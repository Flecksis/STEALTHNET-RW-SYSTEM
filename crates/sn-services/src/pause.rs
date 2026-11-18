use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Default)]
struct State {
    paused: AtomicBool,
    active: AtomicUsize,
    idle: Notify,
    resumed: Notify,
}

#[derive(Clone, Default)]
pub struct PauseGate {
    state: Arc<State>,
}

pub struct WorkGuard {
    state: Arc<State>,
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        if self.state.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.state.idle.notify_waiters();
        }
    }
}

pub struct Paused {
    state: Arc<State>,
}

impl Drop for Paused {
    fn drop(&mut self) {
        self.state.paused.store(false, Ordering::SeqCst);
        self.state.resumed.notify_waiters();
    }
}

impl PauseGate {
    pub fn is_paused(&self) -> bool {
        self.state.paused.load(Ordering::SeqCst)
    }

    pub fn active(&self) -> usize {
        self.state.active.load(Ordering::SeqCst)
    }

    pub fn try_enter(&self) -> Option<WorkGuard> {
        self.state.active.fetch_add(1, Ordering::SeqCst);
        let guard = WorkGuard {
            state: self.state.clone(),
        };
        if self.state.paused.load(Ordering::SeqCst) {
            drop(guard);
            return None;
        }
        Some(guard)
    }

    pub async fn enter(&self) -> WorkGuard {
        loop {
            let changed = self.state.resumed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(guard) = self.try_enter() {
                return guard;
            }
            changed.await;
        }
    }

    pub async fn cleared(&self) {
        loop {
            let changed = self.state.resumed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !self.is_paused() {
                return;
            }
            changed.await;
        }
    }

    pub async fn pause(&self, drain: Duration) -> Paused {
        self.state.paused.store(true, Ordering::SeqCst);
        let paused = Paused {
            state: self.state.clone(),
        };
        let deadline = tokio::time::Instant::now() + drain;
        loop {
            let changed = self.state.idle.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.active() == 0 {
                break;
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                tracing::warn!(
                    active = self.active(),
                    "не все дела закончились перед паузой, продолжаем без них"
                );
                break;
            }
        }
        paused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn пауза_ждёт_начатые_дела_и_не_пускает_новые() {
        let gate = PauseGate::default();
        let work = gate.try_enter().unwrap();
        let pausing = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.pause(Duration::from_secs(5)).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(gate.is_paused());
        assert!(!pausing.is_finished());
        assert!(gate.try_enter().is_none());
        drop(work);
        let paused = pausing.await.unwrap();
        let waiting = {
            let gate = gate.clone();
            tokio::spawn(async move {
                let _guard = gate.enter().await;
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished());
        drop(paused);
        waiting.await.unwrap();
        assert!(!gate.is_paused());
        assert_eq!(gate.active(), 0);
    }

    #[tokio::test]
    async fn пауза_не_зависает_на_долгом_деле() {
        let gate = PauseGate::default();
        let _work = gate.try_enter().unwrap();
        let started = tokio::time::Instant::now();
        let paused = gate.pause(Duration::from_millis(100)).await;
        assert!(started.elapsed() >= Duration::from_millis(100));
        drop(paused);
        gate.cleared().await;
    }
}
