use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_millis(1_050);
const YIELD: Duration = Duration::from_millis(15);

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
            reserve: 5.0,
            max_pause: Duration::from_secs(30),
        }
    }
}

impl SendLimits {
    fn room(&self, class: Class) -> usize {
        let all = self.per_second.max(1.0);
        let share = match class {
            Class::Interactive => all,
            Class::Bulk => (all - self.reserve.max(0.0)).max(1.0),
        };
        share as usize
    }

    fn gap(&self) -> Duration {
        WINDOW.div_f64(self.per_second.max(1.0))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blocked {
    Everyone(Duration),
    Chat(Duration),
}

impl Blocked {
    fn wait(self) -> Duration {
        match self {
            Blocked::Everyone(wait) | Blocked::Chat(wait) => wait,
        }
    }
}

struct State {
    sent: VecDeque<Instant>,
    chats: HashMap<i64, Bucket>,
    paused_until: Option<Instant>,
    careful_until: Option<Instant>,
    replies_waiting: usize,
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

fn left(until: Instant, now: Instant) -> Duration {
    until
        .saturating_duration_since(now)
        .max(Duration::from_millis(1))
}

struct Queued<'a>(&'a Pacer);

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.replies_waiting = state.replies_waiting.saturating_sub(1);
    }
}

impl Pacer {
    pub fn new(limits: SendLimits) -> Self {
        let now = Instant::now();
        Pacer {
            limits,
            state: Mutex::new(State {
                sent: VecDeque::new(),
                chats: HashMap::new(),
                paused_until: None,
                careful_until: None,
                replies_waiting: 0,
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

    fn admit(&self, chat: Option<i64>, class: Class, now: Instant) -> Result<(), Blocked> {
        let limits = self.limits;
        let mut state = self.lock();
        let now = state.sent.back().map_or(now, |last| now.max(*last));
        if let Some(until) = state.paused_until {
            if until > now {
                return Err(Blocked::Everyone(until - now));
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
        while state
            .sent
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= WINDOW)
        {
            state.sent.pop_front();
        }
        let room = limits.room(class);
        if state.sent.len() >= room {
            let frees = state.sent[state.sent.len() - room] + WINDOW;
            return Err(Blocked::Everyone(left(frees, now)));
        }
        if class == Class::Bulk && state.replies_waiting > 0 {
            return Err(Blocked::Everyone(YIELD));
        }
        let careful = state.careful_until.is_some_and(|until| until > now);
        if (class == Class::Bulk || careful)
            && let Some(last) = state.sent.back()
        {
            let next = *last + limits.gap();
            if next > now {
                return Err(Blocked::Everyone(next - now));
            }
        }
        if let Some(chat) = chat {
            let bucket = state.chats.entry(chat).or_insert(Bucket {
                tokens: limits.chat_burst,
                at: now,
            });
            bucket.refill(limits.per_chat, limits.chat_burst, now);
            if bucket.tokens < 1.0 {
                let wait = (1.0 - bucket.tokens) / limits.per_chat;
                return Err(Blocked::Chat(seconds(wait)));
            }
            bucket.tokens -= 1.0;
        }
        state.sent.push_back(now);
        Ok(())
    }

    pub fn take(&self, chat: Option<i64>, class: Class, now: Instant) -> Result<(), Duration> {
        self.admit(chat, class, now).map_err(Blocked::wait)
    }

    pub async fn acquire(&self, chat: Option<i64>, class: Class) {
        let mut waited = false;
        let mut queued: Option<Queued<'_>> = None;
        loop {
            let blocked = match self.admit(chat, class, Instant::now()) {
                Ok(()) => break,
                Err(blocked) => blocked,
            };
            waited = true;
            match blocked {
                Blocked::Everyone(_) if class == Class::Interactive && queued.is_none() => {
                    self.lock().replies_waiting += 1;
                    queued = Some(Queued(self));
                }
                Blocked::Chat(_) => queued = None,
                Blocked::Everyone(_) => {}
            }
            tokio::time::sleep(blocked.wait()).await;
        }
        drop(queued);
        if waited {
            self.lock().waits += 1;
        }
    }

    pub fn try_acquire(&self, chat: Option<i64>, class: Class) -> bool {
        self.admit(chat, class, Instant::now()).is_ok()
    }

    pub fn pause(&self, retry_after: Duration) {
        let until = Instant::now() + retry_after.min(self.limits.max_pause);
        let mut state = self.lock();
        state.pauses += 1;
        if state.paused_until.is_none_or(|current| current < until) {
            state.paused_until = Some(until);
            state.careful_until = Some(until + WINDOW);
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

    const SECOND: Duration = Duration::from_secs(1);

    fn busiest_second(sent: &[Instant]) -> usize {
        let mut most = 0;
        let mut from = 0;
        for (i, at) in sent.iter().enumerate() {
            while at.saturating_duration_since(sent[from]) >= SECOND {
                from += 1;
            }
            most = most.max(i - from + 1);
        }
        most
    }

    fn evenly(sent: &[Instant], gap: Duration) -> bool {
        sent.windows(2).all(|pair| pair[1] - pair[0] >= gap)
    }

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
        assert_eq!(wait, WINDOW, "следующее — когда первое выйдет из окна");
        assert!(
            pacer
                .take(Some(500), Class::Interactive, t0 + SECOND)
                .is_err(),
            "ровно через секунду ещё рано: запас на разброс в сети"
        );
        assert!(
            pacer
                .take(Some(500), Class::Interactive, t0 + WINDOW)
                .is_ok()
        );
        let later = t0 + Duration::from_secs(5);
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
                .take(Some(1_000), Class::Interactive, later + SECOND)
                .is_ok()
        );
    }

    #[test]
    fn за_любую_секунду_уходит_не_больше_предела() {
        let pacer = Pacer::new(SendLimits::default());
        let t0 = Instant::now();
        let mut sent = Vec::new();
        for ms in 0..10_000u64 {
            let now = t0 + Duration::from_millis(ms);
            let class = if ms % 2 == 0 {
                Class::Interactive
            } else {
                Class::Bulk
            };
            for _ in 0..3 {
                let chat = sent.len() as i64;
                if pacer.take(Some(chat), class, now).is_ok() {
                    sent.push(now);
                }
            }
        }
        assert!(busiest_second(&sent) <= 30, "{}", busiest_second(&sent));
        assert!(sent.len() >= 280, "почти весь предел занят: {}", sent.len());
    }

    #[test]
    fn рассылка_идёт_ровно_и_оставляет_место_ответам() {
        let pacer = Pacer::new(SendLimits::default());
        let t0 = Instant::now();
        let mut bulk = Vec::new();
        for ms in 0..5_000u64 {
            let now = t0 + Duration::from_millis(ms);
            if pacer.take(Some(ms as i64), Class::Bulk, now).is_ok() {
                bulk.push(now);
            }
        }
        assert!(busiest_second(&bulk) <= 25, "{}", busiest_second(&bulk));
        assert!(evenly(&bulk, Duration::from_millis(35)));
        assert!(bulk.len() >= 115, "рассылка не простаивает: {}", bulk.len());
        let now = t0 + Duration::from_millis(5_000);
        let recent = bulk.iter().filter(|at| now - **at < WINDOW).count();
        let replies = (10_000..10_100)
            .filter(|chat| pacer.take(Some(*chat), Class::Interactive, now).is_ok())
            .count();
        assert_eq!(recent + replies, 30);
        assert!(replies >= 5, "ответам остаётся запас: {replies}");
        assert!(
            pacer.take(Some(20_000), Class::Bulk, now).is_err(),
            "пока окно занято ответами, рассылка ждёт"
        );
    }

    #[test]
    fn ответы_не_ждут_а_рассылка_идёт_остатком() {
        let pacer = Pacer::new(SendLimits::default());
        let t0 = Instant::now();
        let mut all = Vec::new();
        let mut bulk = 0;
        for ms in 0..10_000u64 {
            let now = t0 + Duration::from_millis(ms);
            if ms % 77 == 0 {
                let chat = 1_000_000 + ms as i64;
                assert!(
                    pacer.take(Some(chat), Class::Interactive, now).is_ok(),
                    "ответ ждёт на {ms} мс"
                );
                all.push(now);
            }
            if pacer.take(Some(ms as i64), Class::Bulk, now).is_ok() {
                all.push(now);
                bulk += 1;
            }
        }
        assert!(busiest_second(&all) <= 30, "{}", busiest_second(&all));
        assert!(bulk >= 100, "рассылка идёт остатком предела: {bulk}");
    }

    #[test]
    fn рассылка_уступает_ответу_который_ждёт() {
        let pacer = Pacer::new(SendLimits::default());
        let t0 = Instant::now();
        assert!(pacer.take(Some(1), Class::Bulk, t0).is_ok());
        pacer.lock().replies_waiting = 1;
        let later = t0 + Duration::from_millis(100);
        assert_eq!(pacer.take(Some(2), Class::Bulk, later), Err(YIELD));
        assert!(pacer.take(Some(3), Class::Interactive, later).is_ok());
        pacer.lock().replies_waiting = 0;
        assert!(
            pacer
                .take(Some(2), Class::Bulk, later + Duration::from_millis(100))
                .is_ok()
        );
    }

    #[tokio::test]
    async fn ждущий_ответ_проходит_раньше_рассылки() {
        let pacer = Arc::new(Pacer::new(SendLimits {
            per_second: 4.0,
            reserve: 1.0,
            ..SendLimits::default()
        }));
        let t0 = Instant::now();
        for chat in 0..4 {
            assert!(pacer.take(Some(chat), Class::Interactive, t0).is_ok());
        }
        let abandoned = tokio::spawn({
            let pacer = pacer.clone();
            async move { pacer.acquire(Some(50), Class::Interactive).await }
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(pacer.lock().replies_waiting, 1);
        abandoned.abort();
        assert!(abandoned.await.is_err());
        assert_eq!(
            pacer.lock().replies_waiting,
            0,
            "брошенный ответ не держит рассылку"
        );
        let order = Arc::new(Mutex::new(Vec::new()));
        let bulk = tokio::spawn({
            let (pacer, order) = (pacer.clone(), order.clone());
            async move {
                pacer.acquire(Some(100), Class::Bulk).await;
                order.lock().unwrap().push("рассылка");
            }
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        let reply = tokio::spawn({
            let (pacer, order) = (pacer.clone(), order.clone());
            async move {
                pacer.acquire(Some(200), Class::Interactive).await;
                order.lock().unwrap().push("ответ");
            }
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(pacer.lock().replies_waiting, 1);
        reply.await.unwrap();
        bulk.await.unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["ответ", "рассылка"]);
        assert_eq!(pacer.lock().replies_waiting, 0);
        assert!(t0.elapsed() >= WINDOW);
        assert_eq!(pacer.stats().waits, 2);
    }

    #[test]
    fn после_паузы_по_429_отправка_идёт_ровно() {
        let pacer = Pacer::new(SendLimits::default());
        pacer.pause(Duration::from_secs(5));
        let wait = pacer
            .take(Some(1), Class::Interactive, Instant::now())
            .unwrap_err();
        assert!(wait > Duration::from_secs(4), "{wait:?}");
        let resumed = Instant::now() + Duration::from_secs(5);
        let mut sent = Vec::new();
        for ms in 0..1_000u64 {
            let now = resumed + Duration::from_millis(ms);
            for chat in 0..3 {
                let chat = 1_000 + (ms as i64) * 3 + chat;
                if pacer.take(Some(chat), Class::Interactive, now).is_ok() {
                    sent.push(now);
                }
            }
        }
        assert_eq!(sent.len(), 29, "первая секунда после паузы — без рывка");
        assert!(evenly(&sent, Duration::from_millis(35)));
        let burst = resumed + Duration::from_secs(3);
        let at_once = (0..100)
            .filter(|chat| {
                pacer
                    .take(Some(50_000 + chat), Class::Interactive, burst)
                    .is_ok()
            })
            .count();
        assert_eq!(at_once, 30, "дальше ответы снова не ждут друг друга");
    }

    #[test]
    fn ответ_429_останавливает_отправку_на_время() {
        let pacer = Pacer::new(SendLimits::default());
        pacer.pause(Duration::from_secs(600));
        let wait = pacer
            .take(Some(1), Class::Interactive, Instant::now())
            .unwrap_err();
        assert!(
            wait > Duration::from_secs(29) && wait <= Duration::from_secs(30),
            "пауза ограничена: {wait:?}"
        );
        pacer.pause(Duration::from_secs(1));
        let still = pacer
            .take(Some(1), Class::Bulk, Instant::now())
            .unwrap_err();
        assert!(
            still > Duration::from_secs(29),
            "короткая пауза не сокращает длинную: {still:?}"
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
