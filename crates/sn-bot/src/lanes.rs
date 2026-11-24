use crate::telegram::Update;
use sn_services::bot_flood::Lane;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Ready(Lane),
    Busy,
}

#[derive(Default)]
struct Chat {
    high: VecDeque<Update>,
    low: VecDeque<Update>,
    mark: Option<Mark>,
}

impl Chat {
    fn next_lane(&self) -> Option<Lane> {
        if !self.high.is_empty() {
            Some(Lane::High)
        } else if !self.low.is_empty() {
            Some(Lane::Low)
        } else {
            None
        }
    }
}

#[derive(Default)]
struct State {
    chats: HashMap<i64, Chat>,
    ready_high: VecDeque<i64>,
    ready_low: VecDeque<i64>,
    high: usize,
    low: usize,
    busy: usize,
    closed: bool,
}

impl State {
    fn enqueue(&mut self, key: i64, lane: Lane) {
        match lane {
            Lane::High => self.ready_high.push_back(key),
            Lane::Low => self.ready_low.push_back(key),
        }
    }

    fn take(&mut self) -> Option<(i64, Update, Lane)> {
        loop {
            let key = self
                .ready_high
                .pop_front()
                .or_else(|| self.ready_low.pop_front())?;
            let Some(chat) = self.chats.get_mut(&key) else {
                continue;
            };
            if !matches!(chat.mark, Some(Mark::Ready(_))) {
                continue;
            }
            let (update, lane) = match chat.high.pop_front() {
                Some(update) => (update, Lane::High),
                None => match chat.low.pop_front() {
                    Some(update) => (update, Lane::Low),
                    None => continue,
                },
            };
            chat.mark = Some(Mark::Busy);
            match lane {
                Lane::High => self.high -= 1,
                Lane::Low => self.low -= 1,
            }
            self.busy += 1;
            return Some((key, update, lane));
        }
    }
}

pub struct Lanes {
    state: Mutex<State>,
    ready: Notify,
    room: Notify,
    workers: usize,
    high_cap: usize,
    low_cap: usize,
    chat_cap: usize,
    peak: AtomicUsize,
    overflow: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneStats {
    pub queued: usize,
    pub busy: usize,
    pub chats: usize,
    pub peak: usize,
    pub overflow: u64,
}

impl Lanes {
    pub fn new(workers: usize, high_cap: usize, low_cap: usize, chat_cap: usize) -> Lanes {
        Lanes {
            state: Mutex::new(State::default()),
            ready: Notify::new(),
            room: Notify::new(),
            workers: workers.max(1),
            high_cap: high_cap.max(1),
            low_cap: low_cap.max(1),
            chat_cap: chat_cap.max(1),
            peak: AtomicUsize::new(0),
            overflow: AtomicU64::new(0),
        }
    }

    pub fn workers(&self) -> usize {
        self.workers
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub async fn push(&self, update: Update, lane: Lane) -> bool {
        let key = update.chat_key();
        let mut update = Some(update);
        loop {
            let room = self.room.notified();
            tokio::pin!(room);
            room.as_mut().enable();
            {
                let mut state = self.lock();
                if state.closed {
                    return false;
                }
                let full = match lane {
                    Lane::High => state.high >= self.high_cap,
                    Lane::Low => state.low >= self.low_cap,
                };
                if !full {
                    let chat = state.chats.entry(key).or_default();
                    let queue = match lane {
                        Lane::High => &mut chat.high,
                        Lane::Low => &mut chat.low,
                    };
                    if queue.len() >= self.chat_cap {
                        self.overflow.fetch_add(1, Ordering::SeqCst);
                        return false;
                    }
                    if let Some(update) = update.take() {
                        queue.push_back(update);
                    }
                    let wake = match chat.mark {
                        None => {
                            chat.mark = Some(Mark::Ready(lane));
                            true
                        }
                        Some(Mark::Ready(Lane::Low)) if lane == Lane::High => {
                            chat.mark = Some(Mark::Ready(Lane::High));
                            true
                        }
                        _ => false,
                    };
                    match lane {
                        Lane::High => state.high += 1,
                        Lane::Low => state.low += 1,
                    }
                    self.peak
                        .fetch_max(state.high + state.low, Ordering::SeqCst);
                    if wake {
                        state.enqueue(key, lane);
                        drop(state);
                        self.ready.notify_one();
                    }
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

    pub async fn next(&self) -> Option<(i64, Update)> {
        loop {
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            {
                let mut state = self.lock();
                if let Some((key, update, _)) = state.take() {
                    let more = !state.ready_high.is_empty() || !state.ready_low.is_empty();
                    drop(state);
                    self.room.notify_one();
                    if more {
                        self.ready.notify_one();
                    }
                    return Some((key, update));
                }
                if state.closed {
                    return None;
                }
            }
            ready.await;
        }
    }

    pub fn done(&self, key: i64) {
        let mut state = self.lock();
        state.busy = state.busy.saturating_sub(1);
        let Some(chat) = state.chats.get_mut(&key) else {
            return;
        };
        match chat.next_lane() {
            None => {
                state.chats.remove(&key);
                let closed = state.closed;
                drop(state);
                if closed {
                    self.ready.notify_waiters();
                }
            }
            Some(lane) => {
                chat.mark = Some(Mark::Ready(lane));
                state.enqueue(key, lane);
                drop(state);
                self.ready.notify_one();
            }
        }
    }

    pub fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_waiters();
        self.room.notify_waiters();
    }

    pub fn stats(&self) -> LaneStats {
        let state = self.lock();
        let queued = state.high + state.low;
        LaneStats {
            queued,
            busy: state.busy,
            chats: state.chats.len(),
            peak: self.peak.swap(queued, Ordering::SeqCst),
            overflow: self.overflow.swap(0, Ordering::SeqCst),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn update(id: i64, chat: i64) -> Update {
        serde_json::from_value(serde_json::json!({
            "update_id": id,
            "message": {"message_id": id, "chat": {"id": chat, "type": "private"}, "date": 1, "text": "/start"}
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn важное_идёт_первым_а_лишнее_отбрасывается() {
        let lanes = Lanes::new(1, 4, 2, 8);
        assert!(lanes.push(update(1, 10), Lane::Low).await);
        assert!(lanes.push(update(2, 11), Lane::Low).await);
        assert!(
            !lanes.push(update(3, 12), Lane::Low).await,
            "общая полоса остального полна"
        );
        assert!(lanes.push(update(4, 13), Lane::High).await);
        let mut order = Vec::new();
        for _ in 0..3 {
            let (key, update) = lanes.next().await.unwrap();
            order.push(update.update_id);
            lanes.done(key);
        }
        assert_eq!(order, [4, 1, 2]);
        let stats = lanes.stats();
        assert_eq!(
            (
                stats.queued,
                stats.busy,
                stats.chats,
                stats.peak,
                stats.overflow
            ),
            (0, 0, 0, 3, 1)
        );
    }

    #[tokio::test]
    async fn один_чат_идёт_по_порядку_и_по_одному() {
        let lanes = Lanes::new(4, 64, 64, 3);
        for id in 1..=3 {
            assert!(lanes.push(update(id, 5), Lane::High).await);
        }
        assert!(
            !lanes.push(update(4, 5), Lane::High).await,
            "у чата своя полка"
        );
        assert!(lanes.push(update(5, 6), Lane::Low).await);
        let (first, one) = lanes.next().await.unwrap();
        assert_eq!((first, one.update_id), (5, 1));
        let (second, other) = lanes.next().await.unwrap();
        assert_eq!(
            (second, other.update_id),
            (6, 5),
            "пока первое обновление чата в работе, второе не выдаётся"
        );
        let waiting = tokio::time::timeout(Duration::from_millis(50), lanes.next()).await;
        assert!(waiting.is_err(), "чат занят — следующего нет");
        lanes.done(5);
        let (again, next) = lanes.next().await.unwrap();
        assert_eq!((again, next.update_id), (5, 2));
        lanes.done(6);
        lanes.done(5);
        let (_, last) = lanes.next().await.unwrap();
        assert_eq!(last.update_id, 3);
        lanes.done(5);
        let stats = lanes.stats();
        assert_eq!((stats.chats, stats.overflow), (0, 1));
    }

    #[tokio::test]
    async fn медленный_чат_не_держит_остальные() {
        let lanes = Arc::new(Lanes::new(4, 1024, 1024, 64));
        let handled = Arc::new(Mutex::new(Vec::new()));
        let mut workers = Vec::new();
        for _ in 0..lanes.workers() {
            let lanes = lanes.clone();
            let handled = handled.clone();
            workers.push(tokio::spawn(async move {
                while let Some((key, update)) = lanes.next().await {
                    if key == 1 {
                        tokio::time::sleep(Duration::from_millis(400)).await;
                    }
                    handled.lock().unwrap().push((key, update.update_id));
                    lanes.done(key);
                }
            }));
        }
        let started = std::time::Instant::now();
        assert!(lanes.push(update(1, 1), Lane::High).await);
        assert!(lanes.push(update(2, 1), Lane::High).await);
        for id in 0..100 {
            assert!(lanes.push(update(100 + id, 100 + id), Lane::High).await);
        }
        loop {
            if handled.lock().unwrap().len() >= 100 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let fast = started.elapsed();
        assert!(
            fast < Duration::from_millis(300),
            "сто других чатов обработаны за {fast:?}, пока медленный чат занят"
        );
        lanes.close();
        for worker in workers {
            worker.await.unwrap();
        }
        let handled = handled.lock().unwrap();
        let slow: Vec<i64> = handled
            .iter()
            .filter(|(key, _)| *key == 1)
            .map(|(_, id)| *id)
            .collect();
        assert_eq!(
            slow,
            [1, 2],
            "медленный чат сохранил порядок и дошёл до конца"
        );
        assert_eq!(handled.len(), 102);
    }

    #[tokio::test]
    async fn полная_полоса_важного_ждёт_места_а_закрытие_отпускает_обработчиков() {
        let lanes = Arc::new(Lanes::new(1, 1, 1, 8));
        assert!(lanes.push(update(1, 5), Lane::High).await);
        let waiting = {
            let lanes = lanes.clone();
            tokio::spawn(async move { lanes.push(update(2, 7), Lane::High).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "важное не отбрасывается, а ждёт");
        let (key, first) = lanes.next().await.unwrap();
        assert_eq!(first.update_id, 1);
        assert!(waiting.await.unwrap());
        lanes.done(key);
        let (key, second) = lanes.next().await.unwrap();
        assert_eq!(second.update_id, 2);
        lanes.done(key);
        let idle = {
            let lanes = lanes.clone();
            tokio::spawn(async move { lanes.next().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        lanes.close();
        assert!(idle.await.unwrap().is_none());
        assert!(!lanes.push(update(3, 5), Lane::High).await);
    }
}
