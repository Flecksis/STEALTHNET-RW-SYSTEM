use crate::telegram::Update;
use sn_services::bot_flood::Lane;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use tokio::sync::Notify;

#[derive(Default)]
struct Queues {
    high: VecDeque<Update>,
    low: VecDeque<Update>,
}

struct Shard {
    queues: Mutex<Queues>,
    ready: Notify,
    room: Notify,
}

pub struct Lanes {
    shards: Vec<Shard>,
    closed: AtomicBool,
    high_cap: usize,
    low_cap: usize,
    queued: AtomicUsize,
    peak: AtomicUsize,
    overflow: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneStats {
    pub queued: usize,
    pub peak: usize,
    pub overflow: u64,
}

pub fn shard_for(chat_key: i64, shards: usize) -> usize {
    let shards = shards.max(1) as i64;
    chat_key.rem_euclid(shards) as usize
}

impl Lanes {
    pub fn new(shards: usize, high_cap: usize, low_cap: usize) -> Lanes {
        Lanes {
            shards: (0..shards.max(1))
                .map(|_| Shard {
                    queues: Mutex::new(Queues::default()),
                    ready: Notify::new(),
                    room: Notify::new(),
                })
                .collect(),
            closed: AtomicBool::new(false),
            high_cap: high_cap.max(1),
            low_cap: low_cap.max(1),
            queued: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            overflow: AtomicU64::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.shards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queued.load(Ordering::SeqCst) == 0
    }

    fn lock(shard: &Shard) -> std::sync::MutexGuard<'_, Queues> {
        match shard.queues.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub async fn push(&self, update: Update, lane: Lane) -> bool {
        let shard = &self.shards[shard_for(update.chat_key(), self.shards.len())];
        let mut update = Some(update);
        loop {
            let room = shard.room.notified();
            tokio::pin!(room);
            room.as_mut().enable();
            {
                let mut queues = Self::lock(shard);
                if self.closed.load(Ordering::SeqCst) {
                    return false;
                }
                let queue = match lane {
                    Lane::High => &mut queues.high,
                    Lane::Low => &mut queues.low,
                };
                let cap = match lane {
                    Lane::High => self.high_cap,
                    Lane::Low => self.low_cap,
                };
                if queue.len() < cap {
                    if let Some(update) = update.take() {
                        queue.push_back(update);
                    }
                    let queued = self.queued.fetch_add(1, Ordering::SeqCst) + 1;
                    self.peak.fetch_max(queued, Ordering::SeqCst);
                    drop(queues);
                    shard.ready.notify_one();
                    return true;
                }
                if lane == Lane::Low {
                    self.overflow.fetch_add(1, Ordering::SeqCst);
                    return false;
                }
            }
            room.await;
        }
    }

    pub async fn next(&self, index: usize) -> Option<Update> {
        let shard = &self.shards[index];
        loop {
            let ready = shard.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            {
                let mut queues = Self::lock(shard);
                let next = queues.high.pop_front().or_else(|| queues.low.pop_front());
                if let Some(update) = next {
                    drop(queues);
                    self.queued.fetch_sub(1, Ordering::SeqCst);
                    shard.room.notify_one();
                    return Some(update);
                }
                if self.closed.load(Ordering::SeqCst) {
                    return None;
                }
            }
            ready.await;
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for shard in &self.shards {
            shard.ready.notify_waiters();
            shard.room.notify_waiters();
        }
    }

    pub fn stats(&self) -> LaneStats {
        LaneStats {
            queued: self.queued.load(Ordering::SeqCst),
            peak: self
                .peak
                .swap(self.queued.load(Ordering::SeqCst), Ordering::SeqCst),
            overflow: self.overflow.swap(0, Ordering::SeqCst),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(id: i64, chat: i64) -> Update {
        serde_json::from_value(serde_json::json!({
            "update_id": id,
            "message": {"message_id": id, "chat": {"id": chat, "type": "private"}, "date": 1, "text": "/start"}
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn важные_обновления_идут_первыми_а_лишние_отбрасываются() {
        let lanes = Lanes::new(1, 4, 2);
        assert!(lanes.push(update(1, 10), Lane::Low).await);
        assert!(lanes.push(update(2, 11), Lane::Low).await);
        assert!(
            !lanes.push(update(3, 12), Lane::Low).await,
            "полоса остального полна"
        );
        assert!(lanes.push(update(4, 13), Lane::High).await);
        let order: Vec<i64> = [
            lanes.next(0).await,
            lanes.next(0).await,
            lanes.next(0).await,
        ]
        .into_iter()
        .map(|u| u.unwrap().update_id)
        .collect();
        assert_eq!(order, [4, 1, 2]);
        let stats = lanes.stats();
        assert_eq!((stats.queued, stats.peak, stats.overflow), (0, 3, 1));
        assert!(lanes.is_empty());
    }

    #[tokio::test]
    async fn полная_полоса_важного_ждёт_места_а_закрытие_отпускает_обработчиков() {
        let lanes = std::sync::Arc::new(Lanes::new(1, 1, 1));
        assert!(lanes.push(update(1, 5), Lane::High).await);
        let waiting = {
            let lanes = lanes.clone();
            tokio::spawn(async move { lanes.push(update(2, 5), Lane::High).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "важное не отбрасывается, а ждёт");
        assert_eq!(lanes.next(0).await.unwrap().update_id, 1);
        assert!(waiting.await.unwrap());
        assert_eq!(lanes.next(0).await.unwrap().update_id, 2);
        let idle = {
            let lanes = lanes.clone();
            tokio::spawn(async move { lanes.next(0).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        lanes.close();
        assert!(idle.await.unwrap().is_none());
        assert!(!lanes.push(update(3, 5), Lane::High).await);
        assert_eq!(shard_for(-7, 4), 1);
    }
}
