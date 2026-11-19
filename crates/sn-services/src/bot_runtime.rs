use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BotProblem {
    BadToken,
    Unreachable,
    Conflict,
    Duplicate,
    Webhook,
    Telegram,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BotState {
    Off,
    NoToken,
    Starting,
    Running {
        username: Option<String>,
        since: DateTime<Utc>,
        last_ok_at: DateTime<Utc>,
    },
    Failed {
        problem: BotProblem,
        detail: Option<String>,
        at: DateTime<Utc>,
        retry_at: Option<DateTime<Utc>>,
    },
}

impl BotState {
    pub fn problem(&self) -> Option<BotProblem> {
        match self {
            BotState::Failed { problem, .. } => Some(*problem),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct BotRuntime {
    state: Mutex<BotState>,
    wake: Notify,
    takeover: AtomicBool,
    emoji: AtomicU8,
}

impl Default for BotRuntime {
    fn default() -> Self {
        BotRuntime {
            state: Mutex::new(BotState::Off),
            wake: Notify::new(),
            takeover: AtomicBool::new(false),
            emoji: AtomicU8::new(0),
        }
    }
}

impl BotRuntime {
    pub fn state(&self) -> BotState {
        match self.state.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn set(&self, next: BotState) {
        match self.state.lock() {
            Ok(mut guard) => *guard = next,
            Err(poisoned) => *poisoned.into_inner() = next,
        }
    }

    pub fn touch(&self, at: DateTime<Utc>) {
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let BotState::Running { last_ok_at, .. } = &mut *guard {
            *last_ok_at = at;
        }
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub async fn woken(&self) {
        self.wake.notified().await;
    }

    pub fn request_takeover(&self) {
        self.takeover.store(true, Ordering::SeqCst);
        self.wake();
    }

    pub fn take_takeover(&self) -> bool {
        self.takeover.swap(false, Ordering::SeqCst)
    }

    pub fn note_custom_emoji(&self, accepted: bool) -> bool {
        let next = if accepted { 2 } else { 1 };
        self.emoji.swap(next, Ordering::SeqCst) != next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn пробуждение_не_теряется_если_бот_ещё_не_ждёт() {
        let runtime = BotRuntime::default();
        assert_eq!(runtime.state(), BotState::Off);
        runtime.wake();
        tokio::time::timeout(std::time::Duration::from_millis(200), runtime.woken())
            .await
            .expect("пробуждение потерялось");
    }

    #[test]
    fn просьба_забрать_бота_срабатывает_один_раз() {
        let runtime = BotRuntime::default();
        assert!(!runtime.take_takeover());
        runtime.request_takeover();
        assert!(runtime.take_takeover());
        assert!(!runtime.take_takeover());
    }

    #[test]
    fn отметка_связи_меняет_только_работающего_бота() {
        let runtime = BotRuntime::default();
        let at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        runtime.touch(at);
        assert_eq!(runtime.state(), BotState::Off);
        runtime.set(BotState::Running {
            username: Some("demo_bot".into()),
            since: at,
            last_ok_at: at,
        });
        let later = at + chrono::Duration::seconds(30);
        runtime.touch(later);
        assert_eq!(
            runtime.state(),
            BotState::Running {
                username: Some("demo_bot".into()),
                since: at,
                last_ok_at: later,
            }
        );
        assert_eq!(
            serde_json::to_value(runtime.state()).unwrap()["state"],
            "running"
        );
    }

    #[test]
    fn сбой_сериализуется_с_причиной() {
        let at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let failed = BotState::Failed {
            problem: BotProblem::Webhook,
            detail: Some("old.example.com".into()),
            at,
            retry_at: None,
        };
        assert_eq!(failed.problem(), Some(BotProblem::Webhook));
        let value = serde_json::to_value(&failed).unwrap();
        assert_eq!(value["state"], "failed");
        assert_eq!(value["problem"], "webhook");
        assert_eq!(value["detail"], "old.example.com");
        assert_eq!(
            serde_json::to_value(BotState::NoToken).unwrap(),
            serde_json::json!({"state": "no_token"})
        );
    }
}
