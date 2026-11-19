use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendLimits {
    pub per_second: f64,
    pub per_chat: f64,
    pub chat_burst: f64,
    pub reserve: f64,
    pub max_pause: Duration,
}

impl Default for SendLimits {
    fn default() -> Self {
        SendLimits {
            per_second: 30.0,
            per_chat: 1.0,
            chat_burst: 3.0,
            reserve: 10.0,
            max_pause: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Interactive,
    Bulk,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn refill(&mut self, per_second: f64, burst: f64, now: Instant) {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * per_second).min(burst);
        self.at = now;
    }
}

struct State {
    global: Bucket,
    chats: HashMap<i64, Bucket>,
    paused_until: Option<Instant>,
    swept: Instant,
    waits: u64,
    pauses: u64,
}

pub struct Pacer {
    limits: SendLimits,
    state: Mutex<State>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PacerStats {
    pub waits: u64,
    pub pauses: u64,
    pub chats: usize,
}

fn seconds(value: f64) -> Duration {
    Duration::from_secs_f64(value.clamp(0.001, 3_600.0))
}

impl Pacer {
    pub fn new(limits: SendLimits) -> Self {
        let now = Instant::now();
        Pacer {
            limits,
            state: Mutex::new(State {
                global: Bucket {
                    tokens: limits.per_second,
                    at: now,
                },
                chats: HashMap::new(),
                paused_until: None,
                swept: now,
                waits: 0,
                pauses: 0,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn take(&self, chat: Option<i64>, class: Class, now: Instant) -> Result<(), Duration> {
        let limits = self.limits;
        let mut state = self.lock();
        if let Some(until) = state.paused_until {
            if until > now {
                return Err(until - now);
            }
            state.paused_until = None;
        }
        if now.saturating_duration_since(state.swept) >= Duration::from_secs(60) {
            state.chats.retain(|_, bucket| {
                now.saturating_duration_since(bucket.at).as_secs_f64() * limits.per_chat
                    + bucket.tokens
                    < limits.chat_burst
            });
            state.swept = now;
        }
        state
            .global
            .refill(limits.per_second, limits.per_second, now);
        let need = match class {
            Class::Interactive => 1.0,
            Class::Bulk => 1.0 + limits.reserve.min(limits.per_second - 1.0).max(0.0),
        };
        if state.global.tokens < need {
            let wait = (need - state.global.tokens) / limits.per_second;
            return Err(seconds(wait));
        }
        if let Some(chat) = chat {
            let bucket = state.chats.entry(chat).or_insert(Bucket {
                tokens: limits.chat_burst,
                at: now,
            });
            bucket.refill(limits.per_chat, limits.chat_burst, now);
            if bucket.tokens < 1.0 {
                let wait = (1.0 - bucket.tokens) / limits.per_chat;
                return Err(seconds(wait));
            }
            bucket.tokens -= 1.0;
        }
        state.global.tokens -= 1.0;
        Ok(())
    }

    pub async fn acquire(&self, chat: Option<i64>, class: Class) {
        let mut waited = false;
        loop {
            match self.take(chat, class, Instant::now()) {
                Ok(()) => break,
                Err(wait) => {
                    waited = true;
                    tokio::time::sleep(wait).await;
                }
            }
        }
        if waited {
            self.lock().waits += 1;
        }
    }

    pub fn try_acquire(&self, chat: Option<i64>, class: Class) -> bool {
        self.take(chat, class, Instant::now()).is_ok()
    }

    pub fn pause(&self, retry_after: Duration) {
        let until = Instant::now() + retry_after.min(self.limits.max_pause);
        let mut state = self.lock();
        state.pauses += 1;
        if state.paused_until.is_none_or(|current| current < until) {
            state.paused_until = Some(until);
        }
    }

    pub fn max_pause(&self) -> Duration {
        self.limits.max_pause
    }

    pub fn stats(&self) -> PacerStats {
        let state = self.lock();
        PacerStats {
            waits: state.waits,
            pauses: state.pauses,
            chats: state.chats.len(),
        }
    }
}

type Registry = Mutex<HashMap<String, Arc<Pacer>>>;

static PACERS: OnceLock<Registry> = OnceLock::new();

pub fn shared(api_base: &str, token: &str, limits: SendLimits) -> Arc<Pacer> {
    let bot = token.split(':').next().unwrap_or_default();
    let key = format!("{}|{bot}", api_base.trim_end_matches('/'));
    let registry = PACERS.get_or_init(Registry::default);
    let mut map = match registry.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    map.entry(key)
        .or_insert_with(|| Arc::new(Pacer::new(limits)))
        .clone()
}

pub fn sends(method: &str) -> bool {
    method.starts_with("send")
        || method.starts_with("edit")
        || method.starts_with("copy")
        || method.starts_with("forward")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn общий_предел_и_предел_на_чат() {
        let pacer = Pacer::new(SendLimits::default());
        let t0 = Instant::now();
        let mut sent = 0;
        for chat in 0..100 {
            if pacer.take(Some(chat), Class::Interactive, t0).is_ok() {
                sent += 1;
            }
        }
        assert_eq!(sent, 30, "за мгновение — не больше 30 сообщений");
        let wait = pacer.take(Some(500), Class::Interactive, t0).unwrap_err();
        assert!(wait <= Duration::from_millis(40), "{wait:?}");
        let later = t0 + Duration::from_secs(1);
        let one_chat = (0..10)
            .filter(|_| pacer.take(Some(1_000), Class::Interactive, later).is_ok())
            .count();
        assert_eq!(
            one_chat, 3,
            "в один чат — небольшой запас, дальше раз в секунду"
        );
        let wait = pacer
            .take(Some(1_000), Class::Interactive, later)
            .unwrap_err();
        assert!(wait > Duration::from_millis(900), "{wait:?}");
        assert!(
            pacer
                .take(
                    Some(1_000),
                    Class::Interactive,
                    later + Duration::from_secs(1)
                )
                .is_ok()
        );
    }

    #[test]
    fn массовая_отправка_оставляет_запас_ответам() {
        let pacer = Pacer::new(SendLimits::default());
        let t0 = Instant::now();
        let bulk = (0..100)
            .filter(|chat| pacer.take(Some(*chat), Class::Bulk, t0).is_ok())
            .count();
        assert_eq!(bulk, 20, "рассылка не трогает 10 сообщений запаса");
        let replies = (100..200)
            .filter(|chat| pacer.take(Some(*chat), Class::Interactive, t0).is_ok())
            .count();
        assert_eq!(replies, 10);
    }

    #[test]
    fn ответ_429_останавливает_отправку_на_время() {
        let pacer = Pacer::new(SendLimits::default());
        pacer.pause(Duration::from_secs(5));
        let wait = pacer
            .take(Some(1), Class::Interactive, Instant::now())
            .unwrap_err();
        assert!(wait > Duration::from_secs(4), "{wait:?}");
        pacer.pause(Duration::from_secs(600));
        let wait = pacer
            .take(Some(1), Class::Interactive, Instant::now())
            .unwrap_err();
        assert!(
            wait <= Duration::from_secs(30),
            "пауза ограничена: {wait:?}"
        );
        assert_eq!(pacer.stats().pauses, 2);
        assert!(sends("sendMessage") && sends("editMessageText") && sends("copyMessage"));
        assert!(!sends("answerCallbackQuery") && !sends("getUpdates") && !sends("setWebhook"));
    }

    #[test]
    fn один_ограничитель_на_бота() {
        let a = shared("http://127.0.0.1:1/", "777:AAA", SendLimits::default());
        let b = shared("http://127.0.0.1:1", "777:BBB", SendLimits::default());
        let c = shared("http://127.0.0.1:1", "778:AAA", SendLimits::default());
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &c));
    }
}
