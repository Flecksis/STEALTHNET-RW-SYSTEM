use crate::auth::CLIENT_SESSION_DAYS;
use crate::error::ServiceResult;
use crate::services::Services;
use sn_modules::catalog::antibot::signals::SHARED_SESSIONS_DAYS;
use sn_store::Db;
use std::time::{Duration, Instant};

pub const CLIENT_SESSION_KEEP_DAYS: i64 = 90;
pub const START_DELAY: Duration = Duration::from_secs(10 * 60);
const PORTION: i64 = 200;
const BUDGET: Duration = Duration::from_secs(2);
const REST: Duration = Duration::from_secs(24 * 3_600);
const AHEAD_SECONDS: i64 = (CLIENT_SESSION_KEEP_DAYS - SHARED_SESSIONS_DAYS) * 86_400;

const _: () = assert!(CLIENT_SESSION_KEEP_DAYS > SHARED_SESSIONS_DAYS);
const _: () = assert!(CLIENT_SESSION_KEEP_DAYS > CLIENT_SESSION_DAYS);

#[derive(Debug)]
pub struct SessionSweep {
    after: i64,
    removed: i64,
    wake: Instant,
}

impl SessionSweep {
    pub fn starting_in(delay: Duration) -> Self {
        SessionSweep {
            after: 0,
            removed: 0,
            wake: Instant::now() + delay,
        }
    }

    pub async fn run(&mut self, services: &Services) -> ServiceResult<i64> {
        self.run_at(services, Instant::now(), BUDGET).await
    }

    async fn run_at(
        &mut self,
        services: &Services,
        now: Instant,
        budget: Duration,
    ) -> ServiceResult<i64> {
        if now < self.wake {
            return Ok(0);
        }
        let drift = services.server_clock().drift();
        if drift.is_some_and(|drift| drift.seconds >= AHEAD_SECONDS) {
            return Ok(0);
        }
        let started = Instant::now();
        let mut removed = 0;
        loop {
            let mut tx = services.store().begin().await?;
            let portion = tx
                .sessions()
                .sweep_client(self.after, PORTION, CLIENT_SESSION_KEEP_DAYS)
                .await?;
            tx.commit().await?;
            removed += portion.removed;
            self.removed += portion.removed;
            let Some(last) = portion.last else {
                if self.removed > 0 {
                    tracing::info!(sessions = self.removed, "старые сессии клиентов удалены");
                }
                *self = SessionSweep {
                    after: 0,
                    removed: 0,
                    wake: now + REST,
                };
                return Ok(removed);
            };
            self.after = last;
            if started.elapsed() >= budget {
                return Ok(removed);
            }
        }
    }
}

#[cfg(test)]
mod tests;
