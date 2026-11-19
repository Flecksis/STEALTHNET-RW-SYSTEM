use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sn_domain::settings::FloodSettings;
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const REPEAT_WINDOW: Duration = Duration::from_secs(30);
pub const PARAM_MEMORY: Duration = Duration::from_secs(3_600);
pub const IDLE: Duration = Duration::from_secs(3_600);
pub const SWEEP_EVERY: Duration = Duration::from_secs(30);
pub const WINDOW: Duration = Duration::from_secs(60);
pub const MAX_USERS: usize = 200_000;
pub const SIEVE_FACTOR: f64 = 6.0;
pub const SIEVE_BURST: f64 = 60.0;
pub const STATE_KEY: &str = "telegram.flood";
pub const SETTINGS_ACTION: &str = "settings.bot_flood";
pub const BLOCK_ACTION: &str = "settings.bot_flood_blocked";
pub const SIGNUPS_ACTION: &str = "settings.bot_signups_limited";
pub const SAMPLE: usize = 10;
const PARAMS_KEPT: usize = 4;
const PARAM_MAX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Payment,
    Button,
    Message,
    Command,
    Start,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    High,
    Low,
}

impl Kind {
    pub const fn lane(self) -> Lane {
        match self {
            Kind::Payment | Kind::Button | Kind::Message => Lane::High,
            Kind::Command | Kind::Start => Lane::Low,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub user: Option<i64>,
    pub name: Option<String>,
    pub kind: Kind,
    pub start: Option<String>,
}

pub fn command_of(text: &str) -> Option<(String, String)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let (head, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let name = head
        .split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    Some((name, args.trim().chars().take(PARAM_MAX).collect()))
}

impl Probe {
    pub fn message(
        user: Option<i64>,
        name: Option<String>,
        text: Option<&str>,
        attachment: bool,
    ) -> Probe {
        let (kind, start) = match text.and_then(command_of) {
            Some((command, args)) if command == "start" => {
                (Kind::Start, Some(args).filter(|a| !a.is_empty()))
            }
            Some(_) => (Kind::Command, None),
            None if text.is_some() || attachment => (Kind::Message, None),
            None => (Kind::Command, None),
        };
        Probe {
            user,
            name,
            kind,
            start,
        }
    }

    pub fn from_json(update: &Value) -> Probe {
        let from = |node: &Value| {
            let user = node.pointer("/from/id").and_then(Value::as_i64);
            let name = node
                .pointer("/from/username")
                .and_then(Value::as_str)
                .map(str::to_string);
            (user, name)
        };
        let simple = |node: &Value, kind: Kind| {
            let (user, name) = from(node);
            Probe {
                user,
                name,
                kind,
                start: None,
            }
        };
        if let Some(query) = update.get("pre_checkout_query") {
            return simple(query, Kind::Payment);
        }
        if let Some(callback) = update.get("callback_query") {
            return simple(callback, Kind::Button);
        }
        if let Some(query) = update.get("widget_query") {
            return simple(query, Kind::Button);
        }
        let Some(message) = update.get("message") else {
            return Probe {
                user: None,
                name: None,
                kind: Kind::Command,
                start: None,
            };
        };
        if message.get("successful_payment").is_some() || message.get("refunded_payment").is_some()
        {
            return simple(message, Kind::Payment);
        }
        let (user, name) = from(message);
        let user = user.or_else(|| message.pointer("/chat/id").and_then(Value::as_i64));
        let attachment = message.get("photo").is_some() || message.get("document").is_some();
        Probe::message(
            user,
            name,
            message.get("text").and_then(Value::as_str),
            attachment,
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn full(burst: f64, now: Instant) -> Self {
        Bucket {
            tokens: burst,
            at: now,
        }
    }

    fn take(&mut self, per_second: f64, burst: f64, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * per_second).min(burst);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Rates {
    messages: f64,
    messages_burst: f64,
    buttons: f64,
    buttons_burst: f64,
    start_interval: Duration,
}

impl Rates {
    fn of(settings: &FloodSettings) -> Rates {
        let per_minute = f64::from(settings.messages_per_minute.max(1));
        Rates {
            messages: per_minute / 60.0,
            messages_burst: (per_minute / 3.0).max(5.0),
            buttons: per_minute * 2.0 / 60.0,
            buttons_burst: (per_minute * 2.0 / 3.0).max(10.0),
            start_interval: Duration::from_secs(u64::from(settings.start_interval_secs.max(1))),
        }
    }
}

fn param_key(param: Option<&str>) -> u64 {
    let mut hasher = DefaultHasher::new();
    param.unwrap_or_default().hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone)]
struct Entry {
    seen: Instant,
    name: Option<String>,
    start_at: Option<Instant>,
    params: VecDeque<(u64, Instant)>,
    messages: Bucket,
    buttons: Bucket,
    window: Instant,
    total: u32,
    excess: u32,
    blocked_until: Option<Instant>,
}

impl Entry {
    fn new(rates: &Rates, now: Instant) -> Entry {
        Entry {
            seen: now,
            name: None,
            start_at: None,
            params: VecDeque::new(),
            messages: Bucket::full(rates.messages_burst, now),
            buttons: Bucket::full(rates.buttons_burst, now),
            window: now,
            total: 0,
            excess: 0,
            blocked_until: None,
        }
    }

    fn blocked(&self, now: Instant) -> bool {
        self.blocked_until.is_some_and(|until| until > now)
    }

    fn recent_param(&self, key: u64, within: Duration, now: Instant) -> bool {
        self.params
            .iter()
            .any(|(k, at)| *k == key && now.saturating_duration_since(*at) < within)
    }

    fn remember_param(&mut self, key: u64, now: Instant) {
        self.params.retain(|(k, _)| *k != key);
        self.params.push_back((key, now));
        while self.params.len() > PARAMS_KEPT {
            self.params.pop_front();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocked {
    pub user: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub updates: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Blocked,
    JustBlocked(Blocked),
    TooOften,
    Repeat,
    TooMany,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Admit { lane: Lane, fresh: bool },
    Refuse(Refusal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signup {
    Allowed { surge: bool, per_minute: u32 },
    Refused,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Minute {
    admitted: u64,
    dropped: u64,
    signups: u64,
    refused: u64,
    blocked: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub blocked: Vec<Blocked>,
    pub blocked_new: u64,
    pub admitted: u64,
    pub dropped: u64,
    pub signups: u64,
    pub refused: u64,
    pub blocked_now: usize,
    pub users: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloodSnapshot {
    pub at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub admitted: u64,
    #[serde(default)]
    pub dropped: u64,
    #[serde(default)]
    pub blocked_new: u64,
    #[serde(default)]
    pub blocked_now: usize,
    #[serde(default)]
    pub signups: u64,
    #[serde(default)]
    pub refused: u64,
}

impl FloodSnapshot {
    pub fn of(report: &Report, at: DateTime<Utc>) -> FloodSnapshot {
        FloodSnapshot {
            at: Some(at),
            admitted: report.admitted,
            dropped: report.dropped,
            blocked_new: report.blocked_new,
            blocked_now: report.blocked_now,
            signups: report.signups,
            refused: report.refused,
        }
    }
}

#[derive(Debug, Clone)]
struct Ring {
    slots: [u32; 60],
    second: u64,
    origin: Instant,
}

impl Ring {
    fn new(origin: Instant) -> Ring {
        Ring {
            slots: [0; 60],
            second: 0,
            origin,
        }
    }

    fn advance(&mut self, now: Instant) {
        let second = now.saturating_duration_since(self.origin).as_secs();
        if second <= self.second {
            return;
        }
        let gap = (second - self.second).min(60);
        for step in 1..=gap {
            self.slots[((self.second + step) % 60) as usize] = 0;
        }
        self.second = second;
    }

    fn add(&mut self, now: Instant) {
        self.advance(now);
        let slot = &mut self.slots[(self.second % 60) as usize];
        *slot = slot.saturating_add(1);
    }

    fn sum(&mut self, now: Instant) -> u32 {
        self.advance(now);
        self.slots
            .iter()
            .fold(0u32, |acc, n| acc.saturating_add(*n))
    }
}

struct Signups {
    attempts: Ring,
    accepted: Ring,
    bucket: Option<Bucket>,
}

struct State {
    users: HashMap<i64, Entry>,
    signups: Signups,
    swept: Instant,
    minute: Minute,
    blocked: Vec<Blocked>,
}

pub struct FloodGate {
    state: Mutex<State>,
    cap: usize,
}

impl Default for FloodGate {
    fn default() -> Self {
        FloodGate::with_cap(MAX_USERS)
    }
}

fn sweep(users: &mut HashMap<i64, Entry>, idle: Duration, now: Instant) {
    users.retain(|_, entry| entry.blocked(now) || now.saturating_duration_since(entry.seen) < idle);
}

impl FloodGate {
    pub fn with_cap(cap: usize) -> Self {
        FloodGate {
            state: Mutex::new(State {
                users: HashMap::new(),
                signups: Signups {
                    attempts: Ring::new(Instant::now()),
                    accepted: Ring::new(Instant::now()),
                    bucket: None,
                },
                swept: Instant::now(),
                minute: Minute::default(),
                blocked: Vec::new(),
            }),
            cap: cap.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn check(&self, probe: &Probe, settings: &FloodSettings, now: Instant) -> Verdict {
        let lane = probe.kind.lane();
        if !settings.enabled || probe.kind == Kind::Payment {
            return Verdict::Admit { lane, fresh: true };
        }
        let Some(user) = probe.user else {
            return Verdict::Admit { lane, fresh: true };
        };
        let rates = Rates::of(settings);
        let mut state = self.lock();
        if now.saturating_duration_since(state.swept) >= SWEEP_EVERY {
            sweep(&mut state.users, IDLE, now);
            state.swept = now;
        }
        if !state.users.contains_key(&user) && state.users.len() >= self.cap {
            sweep(&mut state.users, WINDOW, now);
            if state.users.len() >= self.cap {
                state.minute.dropped += 1;
                return Verdict::Refuse(Refusal::TooMany);
            }
        }
        let entry = state
            .users
            .entry(user)
            .or_insert_with(|| Entry::new(&rates, now));
        entry.seen = now;
        if probe.name.is_some() {
            entry.name.clone_from(&probe.name);
        }
        if now.saturating_duration_since(entry.window) >= WINDOW {
            entry.window = now;
            entry.total = 0;
            entry.excess = 0;
        }
        entry.total = entry.total.saturating_add(1);
        if entry.blocked(now) {
            state.minute.dropped += 1;
            return Verdict::Refuse(Refusal::Blocked);
        }
        let mut fresh = true;
        let refusal = match probe.kind {
            Kind::Start => {
                let key = param_key(probe.start.as_deref());
                if entry
                    .start_at
                    .is_some_and(|at| now.saturating_duration_since(at) < rates.start_interval)
                {
                    Some(Refusal::TooOften)
                } else if entry.recent_param(key, REPEAT_WINDOW, now) {
                    Some(Refusal::Repeat)
                } else if !entry
                    .messages
                    .take(rates.messages, rates.messages_burst, now)
                {
                    Some(Refusal::TooMany)
                } else {
                    fresh = !entry.recent_param(key, PARAM_MEMORY, now);
                    entry.start_at = Some(now);
                    entry.remember_param(key, now);
                    None
                }
            }
            Kind::Button => (!entry.buttons.take(rates.buttons, rates.buttons_burst, now))
                .then_some(Refusal::TooMany),
            Kind::Message | Kind::Command => {
                (!entry
                    .messages
                    .take(rates.messages, rates.messages_burst, now))
                .then_some(Refusal::TooMany)
            }
            Kind::Payment => None,
        };
        let Some(refusal) = refusal else {
            state.minute.admitted += 1;
            return Verdict::Admit { lane, fresh };
        };
        entry.excess = entry.excess.saturating_add(1);
        let block = settings.block_after > 0 && entry.excess >= settings.block_after;
        if !block {
            state.minute.dropped += 1;
            return Verdict::Refuse(refusal);
        }
        entry.blocked_until =
            Some(now + Duration::from_secs(u64::from(settings.block_minutes.max(1)) * 60));
        let blocked = Blocked {
            user,
            name: entry.name.clone(),
            updates: entry.total,
        };
        state.minute.dropped += 1;
        state.minute.blocked += 1;
        if state.blocked.len() < SAMPLE {
            state.blocked.push(blocked.clone());
        }
        Verdict::Refuse(Refusal::JustBlocked(blocked))
    }

    pub fn signup(&self, settings: &FloodSettings, now: Instant) -> Signup {
        let mut state = self.lock();
        let limit = settings.registrations_per_minute;
        if !settings.enabled || limit == 0 {
            state.minute.signups += 1;
            return Signup::Allowed {
                surge: false,
                per_minute: 0,
            };
        }
        let signups = &mut state.signups;
        signups.attempts.add(now);
        let attempts = signups.attempts.sum(now);
        let accepted = signups.accepted.sum(now);
        let burst = f64::from((limit / 10).clamp(1, 1_000));
        let rate = f64::from(limit) / 60.0;
        let bucket = signups.bucket.get_or_insert(Bucket::full(burst, now));
        if accepted >= limit || !bucket.take(rate, burst, now) {
            state.minute.refused += 1;
            return Signup::Refused;
        }
        signups.accepted.add(now);
        state.minute.signups += 1;
        Signup::Allowed {
            surge: u64::from(attempts) * 2 > u64::from(limit),
            per_minute: attempts,
        }
    }

    pub fn unblock(&self, user: i64) -> bool {
        let mut state = self.lock();
        match state.users.get_mut(&user) {
            Some(entry) if entry.blocked_until.is_some() => {
                entry.blocked_until = None;
                entry.excess = 0;
                true
            }
            _ => false,
        }
    }

    pub fn report(&self, now: Instant) -> Report {
        let mut state = self.lock();
        let minute = std::mem::take(&mut state.minute);
        let blocked = std::mem::take(&mut state.blocked);
        let blocked_now = state.users.values().filter(|e| e.blocked(now)).count();
        Report {
            blocked,
            blocked_new: minute.blocked,
            admitted: minute.admitted,
            dropped: minute.dropped,
            signups: minute.signups,
            refused: minute.refused,
            blocked_now,
            users: state.users.len(),
        }
    }

    pub fn users(&self) -> usize {
        self.lock().users.len()
    }
}

struct SieveEntry {
    bucket: Bucket,
    seen: Instant,
}

pub struct Sieve {
    state: Mutex<(HashMap<i64, SieveEntry>, Instant)>,
    cap: usize,
}

impl Default for Sieve {
    fn default() -> Self {
        Sieve {
            state: Mutex::new((HashMap::new(), Instant::now())),
            cap: MAX_USERS,
        }
    }
}

impl Sieve {
    pub fn pass(&self, probe: &Probe, settings: &FloodSettings, now: Instant) -> bool {
        if !settings.enabled || probe.kind == Kind::Payment {
            return true;
        }
        let Some(user) = probe.user else {
            return true;
        };
        let rate = f64::from(settings.messages_per_minute.max(1)) * SIEVE_FACTOR / 60.0;
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let (users, swept) = &mut *guard;
        if now.saturating_duration_since(*swept) >= SWEEP_EVERY {
            users.retain(|_, entry| now.saturating_duration_since(entry.seen) < WINDOW * 10);
            *swept = now;
        }
        if !users.contains_key(&user) && users.len() >= self.cap {
            return true;
        }
        let entry = users.entry(user).or_insert_with(|| SieveEntry {
            bucket: Bucket::full(SIEVE_BURST, now),
            seen: now,
        });
        entry.seen = now;
        entry.bucket.take(rate, SIEVE_BURST, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(user: i64, param: Option<&str>) -> Probe {
        let text = match param {
            Some(param) => format!("/start {param}"),
            None => "/start".into(),
        };
        Probe::message(Some(user), Some("flooder".into()), Some(&text), false)
    }

    fn text(user: i64) -> Probe {
        Probe::message(Some(user), None, Some("привет"), false)
    }

    fn admitted(verdict: &Verdict) -> bool {
        matches!(verdict, Verdict::Admit { .. })
    }

    #[test]
    fn обновления_раскладываются_по_видам_и_очередям() {
        let start = Probe::from_json(&serde_json::json!({
            "update_id": 1,
            "message": {"message_id": 1, "from": {"id": 7, "username": "anna"},
                "chat": {"id": 7, "type": "private"}, "text": "/start@stealth_bot ref_42"}
        }));
        assert_eq!(start.kind, Kind::Start);
        assert_eq!(start.start.as_deref(), Some("ref_42"));
        assert_eq!(start.user, Some(7));
        assert_eq!(start.name.as_deref(), Some("anna"));
        assert_eq!(start.kind.lane(), Lane::Low);
        let paid = Probe::from_json(&serde_json::json!({
            "update_id": 2,
            "message": {"message_id": 2, "from": {"id": 7}, "chat": {"id": 7, "type": "private"},
                "successful_payment": {"currency": "XTR", "total_amount": 1}}
        }));
        assert_eq!(paid.kind, Kind::Payment);
        let button = Probe::from_json(&serde_json::json!({
            "update_id": 3, "callback_query": {"id": "c", "from": {"id": 8}, "data": "m"}
        }));
        assert_eq!((button.kind, button.user), (Kind::Button, Some(8)));
        assert_eq!(button.kind.lane(), Lane::High);
        let support = Probe::from_json(&serde_json::json!({
            "update_id": 4,
            "message": {"message_id": 4, "from": {"id": 9}, "chat": {"id": 9, "type": "private"},
                "photo": [{"file_id": "f", "width": 1, "height": 1}]}
        }));
        assert_eq!(
            (support.kind, support.kind.lane()),
            (Kind::Message, Lane::High)
        );
        let checkout = Probe::from_json(&serde_json::json!({
            "update_id": 5, "pre_checkout_query": {"id": "q", "from": {"id": 9}}
        }));
        assert_eq!(checkout.kind, Kind::Payment);
        assert_eq!(
            command_of("/language@bot"),
            Some(("language".into(), String::new()))
        );
        assert_eq!(command_of("привет"), None);
        assert_eq!(command_of("/ "), None);
    }

    #[test]
    fn старт_не_чаще_интервала_и_без_повтора_параметра() {
        let gate = FloodGate::default();
        let settings = FloodSettings::default();
        let t0 = Instant::now();
        assert_eq!(
            gate.check(&start(1, Some("ref_1")), &settings, t0),
            Verdict::Admit {
                lane: Lane::Low,
                fresh: true
            }
        );
        assert_eq!(
            gate.check(
                &start(1, Some("ref_1")),
                &settings,
                t0 + Duration::from_secs(1)
            ),
            Verdict::Refuse(Refusal::TooOften)
        );
        assert_eq!(
            gate.check(
                &start(1, Some("ref_1")),
                &settings,
                t0 + Duration::from_secs(5)
            ),
            Verdict::Refuse(Refusal::Repeat)
        );
        assert!(admitted(&gate.check(
            &start(1, Some("utm_x")),
            &settings,
            t0 + Duration::from_secs(10)
        )));
        assert_eq!(
            gate.check(
                &start(1, Some("ref_1")),
                &settings,
                t0 + Duration::from_secs(40)
            ),
            Verdict::Admit {
                lane: Lane::Low,
                fresh: false
            },
            "через полминуты тот же старт снова обрабатывается, но переход не новый"
        );
        assert!(admitted(&gate.check(
            &start(2, Some("ref_1")),
            &settings,
            t0
        )));
    }

    #[test]
    fn поток_сообщений_одного_аккаунта_ведёт_к_временной_блокировке() {
        let gate = FloodGate::default();
        let settings = FloodSettings::default();
        let t0 = Instant::now();
        let mut admitted_count = 0;
        let mut just_blocked = None;
        for i in 0..200 {
            match gate.check(&text(5), &settings, t0 + Duration::from_millis(i)) {
                Verdict::Admit { .. } => admitted_count += 1,
                Verdict::Refuse(Refusal::JustBlocked(blocked)) => just_blocked = Some(blocked),
                Verdict::Refuse(_) => {}
            }
        }
        assert_eq!(admitted_count, 10, "пропускается только запас ведра");
        let blocked = just_blocked.expect("аккаунт заблокирован");
        assert_eq!(blocked.user, 5);
        assert_eq!(blocked.updates, 30);
        assert_eq!(
            gate.check(&text(5), &settings, t0 + Duration::from_secs(9 * 60)),
            Verdict::Refuse(Refusal::Blocked)
        );
        assert!(admitted(&gate.check(
            &text(6),
            &settings,
            t0 + Duration::from_secs(1)
        )));
        let paid = Probe {
            kind: Kind::Payment,
            ..text(5)
        };
        assert!(
            admitted(&gate.check(&paid, &settings, t0 + Duration::from_secs(60))),
            "оплаты проходят и у заблокированного"
        );
        assert!(admitted(&gate.check(
            &text(5),
            &settings,
            t0 + Duration::from_secs(10 * 60 + 1)
        )));
        let report = gate.report(t0 + Duration::from_secs(10 * 60 + 2));
        assert_eq!(report.blocked_new, 1);
        assert_eq!(report.blocked, vec![blocked]);
        assert_eq!(report.blocked_now, 0);
        assert!(report.dropped >= 190);
        assert_eq!(gate.report(t0).blocked_new, 0, "отчёт отдаётся один раз");
    }

    #[test]
    fn кнопки_получают_свой_запас_и_выключенная_защита_пропускает_всё() {
        let gate = FloodGate::default();
        let settings = FloodSettings::default();
        let t0 = Instant::now();
        let button = Probe {
            kind: Kind::Button,
            ..text(3)
        };
        let passed = (0..30)
            .filter(|_| admitted(&gate.check(&button, &settings, t0)))
            .count();
        assert_eq!(passed, 20);
        let off = FloodSettings {
            enabled: false,
            ..settings
        };
        assert!((0..500).all(|_| admitted(&gate.check(&text(4), &off, t0))));
        let quiet = FloodSettings {
            block_after: 0,
            ..settings
        };
        assert!(
            (0..500).all(|_| !matches!(
                gate.check(&text(9), &quiet, t0),
                Verdict::Refuse(Refusal::JustBlocked(_))
            )),
            "без порога блокировки только отбрасываем"
        );
    }

    #[test]
    fn регистрации_сверх_предела_откладываются_а_наплыв_отмечается() {
        let gate = FloodGate::default();
        let settings = FloodSettings {
            registrations_per_minute: 60,
            ..FloodSettings::default()
        };
        let t0 = Instant::now();
        let got: Vec<Signup> = (0..8).map(|_| gate.signup(&settings, t0)).collect();
        assert_eq!(
            got.iter()
                .filter(|s| matches!(s, Signup::Allowed { .. }))
                .count(),
            6,
            "сразу проходит запас — десятая часть предела"
        );
        assert_eq!(
            got[0],
            Signup::Allowed {
                surge: false,
                per_minute: 1
            }
        );
        assert_eq!(got[7], Signup::Refused);
        let crowd: Vec<Signup> = (0..40)
            .map(|i| gate.signup(&settings, t0 + Duration::from_millis(100 * i)))
            .collect();
        let allowed: Vec<&Signup> = crowd
            .iter()
            .filter(|s| matches!(s, Signup::Allowed { .. }))
            .collect();
        assert_eq!(allowed.len(), 3, "дальше по одной в секунду");
        assert_eq!(
            allowed.last(),
            Some(&&Signup::Allowed {
                surge: true,
                per_minute: 40
            }),
            "когда попыток за минуту больше половины предела, принятые отмечаются: {allowed:?}"
        );
        assert!(matches!(allowed[0], Signup::Allowed { surge: false, .. }));
        let later = t0 + Duration::from_secs(5 * 60);
        assert_eq!(
            gate.signup(&settings, later),
            Signup::Allowed {
                surge: false,
                per_minute: 1
            },
            "через минуту наплыв забыт"
        );
        let report = gate.report(later);
        assert_eq!((report.signups, report.refused), (10, 39));
        let unlimited = FloodSettings {
            registrations_per_minute: 0,
            ..settings
        };
        assert!((0..100).all(|_| gate.signup(&unlimited, t0) != Signup::Refused));
        let mut ring = Ring::new(t0);
        ring.add(t0);
        ring.add(t0 + Duration::from_secs(30));
        assert_eq!(ring.sum(t0 + Duration::from_secs(59)), 2);
        assert_eq!(ring.sum(t0 + Duration::from_secs(61)), 1);
        assert_eq!(ring.sum(t0 + Duration::from_secs(3_600)), 0);
    }

    #[test]
    fn память_о_пользователях_ограничена() {
        let gate = FloodGate::with_cap(100);
        let settings = FloodSettings::default();
        let t0 = Instant::now();
        for user in 0..1_000 {
            gate.check(&text(user), &settings, t0);
        }
        assert_eq!(gate.users(), 100);
        let later = t0 + WINDOW + Duration::from_secs(1);
        assert!(admitted(&gate.check(&text(5_000), &settings, later)));
        assert!(gate.users() <= 100);
    }

    #[test]
    fn сито_вебхука_грубее_и_пропускает_оплаты() {
        let sieve = Sieve::default();
        let settings = FloodSettings::default();
        let t0 = Instant::now();
        let passed = (0..1_000)
            .filter(|_| sieve.pass(&text(1), &settings, t0))
            .count();
        assert_eq!(passed, SIEVE_BURST as usize);
        let paid = Probe {
            kind: Kind::Payment,
            ..text(1)
        };
        assert!(sieve.pass(&paid, &settings, t0));
        assert!(sieve.pass(&text(2), &settings, t0));
        assert!(sieve.pass(&text(1), &settings, t0 + Duration::from_secs(2)));
    }
}
