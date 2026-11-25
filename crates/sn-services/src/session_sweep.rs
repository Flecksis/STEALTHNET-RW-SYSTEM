use crate::auth::CLIENT_SESSION_DAYS;
use crate::error::ServiceResult;
use crate::services::Services;
use sn_modules::catalog::antibot::signals::SHARED_SESSIONS_DAYS;
use sn_store::Db;
use std::time::{Duration, Instant};

pub const CLIENT_SESSION_KEEP_DAYS: i64 = 90;
const PORTION: i64 = 200;
const BUDGET: Duration = Duration::from_secs(2);
const REST: Duration = Duration::from_secs(24 * 3_600);

const _: () = assert!(CLIENT_SESSION_KEEP_DAYS > SHARED_SESSIONS_DAYS);
const _: () = assert!(CLIENT_SESSION_KEEP_DAYS > CLIENT_SESSION_DAYS);

#[derive(Debug, Default)]
pub struct SessionSweep {
    after: i64,
    removed: i64,
    finished: Option<Instant>,
}

impl SessionSweep {
    pub async fn run(&mut self, services: &Services) -> ServiceResult<i64> {
        self.run_at(services, Instant::now(), BUDGET).await
    }

    async fn run_at(
        &mut self,
        services: &Services,
        now: Instant,
        budget: Duration,
    ) -> ServiceResult<i64> {
        if self
            .finished
            .is_some_and(|at| now.saturating_duration_since(at) < REST)
        {
            return Ok(0);
        }
        let started = Instant::now();
        let mut removed = 0;
        let mut conn = services.store().conn().await?;
        loop {
            let portion = conn
                .sessions()
                .sweep_client(self.after, PORTION, CLIENT_SESSION_KEEP_DAYS)
                .await?;
            removed += portion.removed;
            self.removed += portion.removed;
            let Some(last) = portion.last else {
                if self.removed > 0 {
                    tracing::info!(sessions = self.removed, "старые сессии клиентов удалены");
                }
                *self = SessionSweep {
                    finished: Some(now),
                    ..SessionSweep::default()
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
