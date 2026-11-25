use crate::alerts::outage_text;
use crate::auth::{INIT_DATA_AHEAD_SECONDS, INIT_DATA_MAX_AGE_SECONDS};
use crate::clock::Clock;
use crate::error::ServiceResult;
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_store::Db;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const STATE_KEY: &str = "server.clock";
pub const REPORT_EVERY: Duration = Duration::from_secs(60);
pub const NOTICE_MINUTES: i32 = 15;
pub const DRIFT_SECONDS: i64 = 60;

const STEADY_SECONDS: i64 = 45;
const SAMPLE_FRESH: Duration = Duration::from_secs(10 * 60);
const LOGINS_WINDOW: Duration = Duration::from_secs(15 * 60);
const LOGINS_KEPT: usize = 64;
const LOGINS_SUSPECT: usize = 3;
const DRIFT_LOG_EVERY: Duration = Duration::from_secs(3_600);
const LOGIN_LOG_EVERY: Duration = Duration::from_secs(300);
const FIX: &str = "включите на сервере синхронизацию времени: timedatectl set-ntp true";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    Telegram,
    Login,
    Logins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drift {
    pub seconds: i64,
    pub evidence: Evidence,
}

impl Drift {
    pub fn closes_miniapp(&self) -> bool {
        self.seconds >= INIT_DATA_MAX_AGE_SECONDS || self.seconds < -INIT_DATA_AHEAD_SECONDS
    }

    pub fn span(&self) -> String {
        span(self.seconds)
    }

    fn phrase(&self) -> String {
        let span = self.span();
        match (self.evidence, self.seconds > 0) {
            (Evidence::Telegram, true) => format!("часы сервера спешат на {span}"),
            (Evidence::Telegram, false) => format!("часы сервера отстают на {span}"),
            (Evidence::Login, _) => format!("часы сервера отстают не меньше чем на {span}"),
            (Evidence::Logins, _) => format!("часы сервера, похоже, спешат на {span}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blame {
    Data,
    Server,
}

fn span(seconds: i64) -> String {
    outage_text(
        chrono::Duration::try_seconds(seconds.saturating_abs()).unwrap_or(chrono::Duration::MAX),
    )
}

fn within(now: Instant, then: Instant, limit: Duration) -> bool {
    now.saturating_duration_since(then) <= limit
}

fn due(logged: &mut Option<Instant>, at: Instant, pause: Duration) -> bool {
    let quiet = logged.is_some_and(|then| within(at, then, pause));
    if !quiet {
        *logged = Some(at);
    }
    !quiet
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    at: Instant,
    seconds: i64,
}

impl Sample {
    fn beyond(&self, limit: i64) -> bool {
        self.seconds.saturating_abs() >= limit
    }
}

#[derive(Debug, Clone, Copy)]
struct OldData {
    at: Instant,
    user: Option<i64>,
    seconds: i64,
}

#[derive(Debug, Default)]
struct State {
    sample: Option<Sample>,
    drift: Option<i64>,
    behind: Option<Sample>,
    old: VecDeque<OldData>,
    admitted: Option<Instant>,
    drift_logged: Option<Instant>,
    login_logged: Option<Instant>,
    doubt_logged: Option<Instant>,
    saved: Option<bool>,
}

impl State {
    fn measured(&self, at: Instant) -> Option<Sample> {
        self.sample.filter(|s| within(at, s.at, SAMPLE_FRESH))
    }

    fn suspected(&self, at: Instant) -> Option<i64> {
        if self
            .admitted
            .is_some_and(|then| within(at, then, LOGINS_WINDOW))
        {
            return None;
        }
        let recent = || self.old.iter().filter(|o| within(at, o.at, LOGINS_WINDOW));
        let users: HashSet<Option<i64>> = recent().map(|o| o.user).collect();
        (users.len() >= LOGINS_SUSPECT)
            .then(|| recent().map(|o| o.seconds).min())
            .flatten()
    }

    fn verdict(&self, at: Instant) -> Option<Drift> {
        let measured = self.measured(at).is_some();
        let found = if let Some(seconds) = self.drift.filter(|_| measured) {
            (seconds, Evidence::Telegram)
        } else if let Some(behind) = self.behind.filter(|s| within(at, s.at, LOGINS_WINDOW)) {
            (behind.seconds, Evidence::Login)
        } else if measured {
            return None;
        } else {
            (self.suspected(at)?, Evidence::Logins)
        };
        Some(Drift {
            seconds: found.0,
            evidence: found.1,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    Save(Drift),
    Forget,
    Nothing,
}

pub struct ServerClock {
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

impl ServerClock {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        ServerClock {
            clock,
            state: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn drift(&self) -> Option<Drift> {
        self.lock().verdict(Instant::now())
    }

    pub fn answered(&self, telegram: DateTime<Utc>) {
        let ahead = self.clock.now() - telegram;
        self.answered_at(Instant::now(), ahead.num_seconds());
    }

    fn answered_at(&self, at: Instant, seconds: i64) {
        let mut state = self.lock();
        let sample = Sample { at, seconds };
        let same_way = |other: i64| other.signum() == seconds.signum();
        let repeated = state
            .measured(at)
            .is_some_and(|last| last.beyond(DRIFT_SECONDS) && same_way(last.seconds));
        let before = state.drift;
        let holds = before.is_some_and(same_way) && sample.beyond(STEADY_SECONDS);
        state.sample = Some(sample);
        state.drift = (holds || (repeated && sample.beyond(DRIFT_SECONDS))).then_some(seconds);
        if !sample.beyond(DRIFT_SECONDS) {
            state.behind = None;
        }
        let Some(seconds) = state.drift else {
            state.drift_logged = None;
            if before.is_some() && !sample.beyond(DRIFT_SECONDS) {
                tracing::info!("часы сервера снова сходятся с Telegram");
            }
            return;
        };
        if !due(&mut state.drift_logged, at, DRIFT_LOG_EVERY) {
            return;
        }
        let drift = Drift {
            seconds,
            evidence: Evidence::Telegram,
        };
        let closed = if drift.closes_miniapp() {
            ", мини-приложение никого не пускает"
        } else {
            ""
        };
        tracing::warn!(
            drift_seconds = seconds,
            "{} по сравнению с Telegram{closed}; {FIX}",
            drift.phrase()
        );
    }

    pub fn admitted(&self) {
        self.admitted_at(Instant::now());
    }

    fn admitted_at(&self, at: Instant) {
        let mut state = self.lock();
        state.admitted = Some(at);
        state.behind = None;
        state.old.clear();
    }

    pub fn refused(&self, user: Option<i64>, age: chrono::Duration) -> Blame {
        let seconds = age.num_milliseconds().div_euclid(1_000);
        self.refused_at(Instant::now(), user, seconds)
    }

    fn refused_at(&self, at: Instant, user: Option<i64>, seconds: i64) -> Blame {
        let mut state = self.lock();
        let measured = state.measured(at).is_some();
        if seconds < 0 {
            let worst = state
                .behind
                .filter(|s| within(at, s.at, LOGINS_WINDOW))
                .map_or(seconds, |s| s.seconds.min(seconds));
            state.behind = Some(Sample { at, seconds: worst });
        } else if !measured {
            while state.old.len() >= LOGINS_KEPT {
                state.old.pop_front();
            }
            state.old.push_back(OldData { at, user, seconds });
        }
        let closed = state.verdict(at).filter(Drift::closes_miniapp);
        if seconds < 0 || closed.is_some() {
            let drift = closed.unwrap_or(Drift {
                seconds,
                evidence: Evidence::Login,
            });
            if due(&mut state.login_logged, at, LOGIN_LOG_EVERY) {
                tracing::warn!(
                    data_age_seconds = seconds,
                    drift_seconds = drift.seconds,
                    "вход в мини-приложение отклонён: {}; {FIX}",
                    drift.phrase()
                );
            }
            return Blame::Server;
        }
        if measured {
            tracing::debug!(
                data_age_seconds = seconds,
                "вход в мини-приложение отклонён: данные от Telegram старше часа"
            );
        } else if due(&mut state.doubt_logged, at, LOGIN_LOG_EVERY) {
            tracing::warn!(
                data_age_seconds = seconds,
                "вход в мини-приложение отклонён: по часам сервера данным от Telegram {}, принимаются не старше часа; если так у всех клиентов, часы сервера спешат, {FIX}",
                span(seconds)
            );
        }
        Blame::Data
    }

    fn report(&self, at: Instant) -> Report {
        let state = self.lock();
        if let Some(drift) = state.verdict(at) {
            return Report::Save(drift);
        }
        let outdated = match state.saved {
            Some(saved) => saved,
            None => state.measured(at).is_some() || state.admitted.is_some(),
        };
        if outdated {
            Report::Forget
        } else {
            Report::Nothing
        }
    }

    fn reported(&self, saved: bool) {
        self.lock().saved = Some(saved);
    }
}

pub struct ServerClockOps<'a> {
    services: &'a Services,
}

impl<'a> ServerClockOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        ServerClockOps { services }
    }

    pub async fn report(&self) -> ServiceResult<()> {
        match self.services.server_clock().report(Instant::now()) {
            Report::Save(drift) => {
                let mut conn = self.services.store().conn().await?;
                conn.bot_state()
                    .set(STATE_KEY, &serde_json::json!(drift))
                    .await?;
                self.services.server_clock().reported(true);
            }
            Report::Forget => {
                let mut conn = self.services.store().conn().await?;
                sqlx::query("DELETE FROM bot_state WHERE key = $1")
                    .bind(STATE_KEY)
                    .execute(conn.conn())
                    .await?;
                self.services.server_clock().reported(false);
            }
            Report::Nothing => {}
        }
        Ok(())
    }

    pub async fn notice(&self) -> ServiceResult<Option<Drift>> {
        let mut conn = self.services.store().conn().await?;
        let saved: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT value FROM bot_state WHERE key = $1 \
             AND updated_at > clock_timestamp() - make_interval(mins => $2) \
             AND updated_at < clock_timestamp() + interval '1 minute'",
        )
        .bind(STATE_KEY)
        .bind(NOTICE_MINUTES)
        .fetch_optional(conn.conn())
        .await?;
        Ok(saved.and_then(|value| serde_json::from_value(value).ok()))
    }
}

#[cfg(test)]
mod tests;
