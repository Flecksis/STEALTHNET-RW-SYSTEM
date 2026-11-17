use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Duration, TimeZone, Utc};
use sha2::{Digest, Sha256};
use sn_store::Db;

pub const RATE_LIMITED: &str = "auth.rate_limited";
pub const RETENTION_DAYS: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateRule {
    pub scope: &'static str,
    pub limit: u32,
    pub window_seconds: i64,
}

pub mod rules {
    use super::RateRule;

    pub const ADMIN_LOGIN_IP: RateRule = RateRule {
        scope: "admin_login_ip",
        limit: 30,
        window_seconds: 300,
    };
    pub const ADMIN_LOGIN_ACCOUNT: RateRule = RateRule {
        scope: "admin_login_account",
        limit: 100,
        window_seconds: 900,
    };
    pub const ADMIN_LOGIN_ACCOUNT_IP: RateRule = RateRule {
        scope: "admin_login_account_ip",
        limit: 10,
        window_seconds: 900,
    };
    pub const ADMIN_TOTP_FAILURES: RateRule = RateRule {
        scope: "admin_totp_failures",
        limit: 5,
        window_seconds: 900,
    };
    pub const ADMIN_TOTP_FAILURES_DAY: RateRule = RateRule {
        scope: "admin_totp_failures_day",
        limit: 20,
        window_seconds: 86_400,
    };
    pub const ADMIN_SECURITY: RateRule = RateRule {
        scope: "admin_security",
        limit: 10,
        window_seconds: 900,
    };
    pub const MINIAPP_IP: RateRule = RateRule {
        scope: "miniapp_login_ip",
        limit: 120,
        window_seconds: 60,
    };
    pub const TELEGRAM_START_IP: RateRule = RateRule {
        scope: "telegram_login_ip",
        limit: 10,
        window_seconds: 600,
    };
    pub const TELEGRAM_POLL_IP: RateRule = RateRule {
        scope: "telegram_poll_ip",
        limit: 120,
        window_seconds: 60,
    };
    pub const EMAIL_START_IP: RateRule = RateRule {
        scope: "email_login_ip",
        limit: 5,
        window_seconds: 600,
    };
    pub const EMAIL_START_ADDRESS: RateRule = RateRule {
        scope: "email_login_address",
        limit: 3,
        window_seconds: 900,
    };
    pub const EMAIL_VERIFY_IP: RateRule = RateRule {
        scope: "email_verify_ip",
        limit: 20,
        window_seconds: 900,
    };
    pub const EMAIL_VERIFY_ADDRESS: RateRule = RateRule {
        scope: "email_verify_address",
        limit: 10,
        window_seconds: 900,
    };
}

pub fn bucket(rule: RateRule, identity: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(rule.scope.as_bytes());
    hasher.update(b"\n");
    hasher.update(identity.trim().to_lowercase().as_bytes());
    hasher.finalize().to_vec()
}

pub fn window_start(rule: RateRule, now: DateTime<Utc>) -> DateTime<Utc> {
    let window = rule.window_seconds.max(1);
    let start = now.timestamp().div_euclid(window) * window;
    Utc.timestamp_opt(start, 0).single().unwrap_or(now)
}

fn window(rule: RateRule) -> Duration {
    Duration::seconds(rule.window_seconds.max(1))
}

pub fn wait_phrase(left: Duration) -> String {
    let minutes = (left.num_seconds().max(1) + 59) / 60;
    if minutes <= 90 {
        format!("{minutes} мин.")
    } else {
        format!("{} ч.", (minutes + 59) / 60)
    }
}

pub fn rate_limited(reason: &str, left: Duration) -> ServiceError {
    ServiceError::rejected(
        RATE_LIMITED,
        format!("{reason} Попробуйте снова через {}", wait_phrase(left)),
    )
}

fn too_many(rule: RateRule, start: DateTime<Utc>, now: DateTime<Utc>) -> ServiceError {
    rate_limited("Слишком много попыток.", start + window(rule) - now)
}

pub fn unlock_at(rule: RateRule, start: DateTime<Utc>, current: u32) -> DateTime<Utc> {
    if current >= rule.limit {
        start + window(rule) + window(rule)
    } else {
        start + window(rule)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Strike {
    bucket: Vec<u8>,
    window_start: DateTime<Utc>,
    pub rule: RateRule,
    pub failures: u32,
}

impl Strike {
    pub const fn reached_limit(&self) -> bool {
        self.failures == self.rule.limit
    }
}

pub struct RateLimits<'a> {
    services: &'a Services,
}

impl<'a> RateLimits<'a> {
    pub fn new(services: &'a Services) -> Self {
        RateLimits { services }
    }

    async fn sweep(&self, conn: &mut sn_store::Conn, now: DateTime<Utc>) -> ServiceResult<()> {
        sqlx::query("DELETE FROM auth_rate_limits WHERE window_start < $1")
            .bind(now - Duration::days(RETENTION_DAYS))
            .execute(conn.conn())
            .await?;
        Ok(())
    }

    pub async fn hit(&self, rule: RateRule, identity: &str) -> ServiceResult<u32> {
        let now = self.services.now();
        let start = window_start(rule, now);
        let mut conn = self.services.store().conn().await?;
        let hits: i32 = sqlx::query_scalar(
            "INSERT INTO auth_rate_limits (bucket, window_start, hits) VALUES ($1, $2, 1) \
             ON CONFLICT (bucket, window_start) DO UPDATE SET hits = LEAST(auth_rate_limits.hits + 1, 1000000) \
             RETURNING hits",
        )
        .bind(bucket(rule, identity))
        .bind(start)
        .fetch_one(conn.conn())
        .await?;
        if hits == 1 {
            self.sweep(&mut conn, now).await?;
        }
        let hits = u32::try_from(hits).unwrap_or(u32::MAX);
        if hits > rule.limit {
            return Err(too_many(rule, start, now));
        }
        Ok(hits)
    }

    pub async fn hit_ip(&self, rule: RateRule, ip: Option<&str>) -> ServiceResult<u32> {
        self.hit(rule, ip.unwrap_or("unknown")).await
    }

    pub async fn strike(
        &self,
        rule: RateRule,
        identity: &str,
        reason: &str,
    ) -> ServiceResult<Strike> {
        let now = self.services.now();
        let start = window_start(rule, now);
        let strike_bucket = bucket(rule, identity);
        let mut conn = self.services.store().conn().await?;
        let (current, previous): (i32, i32) = sqlx::query_as(
            "WITH cur AS ( \
                 INSERT INTO auth_rate_limits (bucket, window_start, hits) VALUES ($1, $2, 1) \
                 ON CONFLICT (bucket, window_start) DO UPDATE SET hits = LEAST(auth_rate_limits.hits + 1, 1000000) \
                 RETURNING hits) \
             SELECT (SELECT hits FROM cur), \
                    COALESCE((SELECT hits FROM auth_rate_limits WHERE bucket = $1 AND window_start = $3), 0)",
        )
        .bind(&strike_bucket)
        .bind(start)
        .bind(start - window(rule))
        .fetch_one(conn.conn())
        .await?;
        if current == 1 {
            self.sweep(&mut conn, now).await?;
        }
        let current = u32::try_from(current).unwrap_or(u32::MAX);
        let previous = u32::try_from(previous).unwrap_or(u32::MAX);
        let strike = Strike {
            bucket: strike_bucket,
            window_start: start,
            rule,
            failures: current.saturating_add(previous),
        };
        if strike.failures > rule.limit {
            self.release(&mut conn, &strike).await?;
            let left = unlock_at(rule, start, current.saturating_sub(1)) - now;
            return Err(rate_limited(reason, left));
        }
        Ok(strike)
    }

    async fn release(&self, conn: &mut sn_store::Conn, strike: &Strike) -> ServiceResult<()> {
        sqlx::query(
            "WITH gone AS ( \
                 DELETE FROM auth_rate_limits WHERE bucket = $1 AND window_start = $2 AND hits <= 1 RETURNING 1) \
             UPDATE auth_rate_limits SET hits = hits - 1 WHERE bucket = $1 AND window_start = $2 AND hits > 1",
        )
        .bind(&strike.bucket)
        .bind(strike.window_start)
        .execute(conn.conn())
        .await?;
        Ok(())
    }

    pub async fn forgive(&self, strike: &Strike) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        self.release(&mut conn, strike).await
    }

    pub async fn forget(&self, rule: RateRule, identity: &str) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        sqlx::query("DELETE FROM auth_rate_limits WHERE bucket = $1")
            .bind(bucket(rule, identity))
            .execute(conn.conn())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn окно_выравнивается_по_длине() {
        let rule = rules::ADMIN_LOGIN_IP;
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 31).unwrap();
        assert_eq!(
            window_start(rule, at),
            Utc.with_ymd_and_hms(2026, 11, 16, 14, 50, 0).unwrap()
        );
        assert_eq!(bucket(rule, " 203.0.113.7 "), bucket(rule, "203.0.113.7"));
        assert_ne!(
            bucket(rule, "203.0.113.7"),
            bucket(rules::TELEGRAM_POLL_IP, "203.0.113.7")
        );
        let err = too_many(rule, window_start(rule, at), at);
        assert_eq!(err.code(), RATE_LIMITED);
        assert!(err.to_string().contains("3 мин"));
    }

    #[test]
    fn блокировка_после_неудач_держится_до_конца_следующего_окна() {
        let rule = rules::ADMIN_TOTP_FAILURES;
        let start = Utc.with_ymd_and_hms(2026, 11, 16, 14, 45, 0).unwrap();
        assert_eq!(
            unlock_at(rule, start, 2),
            Utc.with_ymd_and_hms(2026, 11, 16, 15, 0, 0).unwrap()
        );
        assert_eq!(
            unlock_at(rule, start, rule.limit),
            Utc.with_ymd_and_hms(2026, 11, 16, 15, 15, 0).unwrap()
        );
        assert_eq!(wait_phrase(Duration::seconds(30)), "1 мин.");
        assert_eq!(wait_phrase(Duration::minutes(90)), "90 мин.");
        assert_eq!(wait_phrase(Duration::hours(30)), "30 ч.");
        assert!(
            rate_limited("Слишком много попыток.", Duration::minutes(20))
                .to_string()
                .ends_with("через 20 мин.")
        );
    }
}
