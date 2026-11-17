use super::non_negative_u32;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{JobId, OrderId};
use sn_domain::jobs::{JobKind, JobPayload, JobStatus};
use sqlx::PgConnection;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct JobRecord {
    pub id: JobId,
    pub kind: String,
    pub payload: serde_json::Value,
    pub attempts: u32,
    pub max_attempts: u32,
    pub run_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub lease_token: Option<Uuid>,
}

impl JobRecord {
    pub fn typed(&self) -> StoreResult<JobPayload> {
        JobKind::parse(&self.kind)?;
        Ok(serde_json::from_value(self.payload.clone())?)
    }

    pub const fn exhausted(&self) -> bool {
        self.attempts >= self.max_attempts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuedJob {
    Active(JobId),
    Requeued(JobId),
    Created(JobId),
}

impl QueuedJob {
    pub const fn id(self) -> JobId {
        match self {
            QueuedJob::Active(id) | QueuedJob::Requeued(id) | QueuedJob::Created(id) => id,
        }
    }

    pub const fn state(self) -> &'static str {
        match self {
            QueuedJob::Active(_) => "already_queued",
            QueuedJob::Requeued(_) => "requeued",
            QueuedJob::Created(_) => "created",
        }
    }

    pub const fn armed(self) -> bool {
        !matches!(self, QueuedJob::Active(_))
    }
}

#[derive(sqlx::FromRow)]
struct JobRow {
    id: i64,
    kind: String,
    payload: serde_json::Value,
    attempts: i32,
    max_attempts: i32,
    run_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    lease_token: Option<Uuid>,
}

impl TryFrom<JobRow> for JobRecord {
    type Error = StoreError;

    fn try_from(row: JobRow) -> Result<Self, Self::Error> {
        Ok(JobRecord {
            id: JobId(row.id),
            kind: row.kind,
            payload: row.payload,
            attempts: non_negative_u32(row.attempts, "attempts")?,
            max_attempts: non_negative_u32(row.max_attempts, "max_attempts")?,
            run_at: row.run_at,
            created_at: row.created_at,
            lease_token: row.lease_token,
        })
    }
}

fn fulfill_key_prefix() -> String {
    let probe = JobPayload::FulfillOrder {
        order_id: OrderId(0),
    }
    .dedupe_key()
    .unwrap_or_default();
    probe.strip_suffix('0').unwrap_or(&probe).to_string()
}

pub struct JobsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> JobsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        JobsRepo { conn }
    }

    pub async fn enqueue(
        &mut self,
        payload: &JobPayload,
        run_at: Option<DateTime<Utc>>,
    ) -> StoreResult<Option<JobId>> {
        let kind = payload.kind();
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO jobs (kind, dedupe_key, payload, run_at, max_attempts) VALUES ($1, $2, $3, COALESCE($4, now()), $5) \
             ON CONFLICT (dedupe_key) WHERE status IN ('queued', 'running') DO NOTHING RETURNING id",
        )
        .bind(kind.as_str())
        .bind(payload.dedupe_key())
        .bind(serde_json::to_value(payload)?)
        .bind(run_at)
        .bind(kind.max_attempts() as i32)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(id.map(JobId))
    }

    pub async fn claim(&mut self, limit: i64, lease: Duration) -> StoreResult<Vec<JobRecord>> {
        let rows: Vec<JobRow> = sqlx::query_as(
            "WITH picked AS ( \
                SELECT id FROM jobs \
                WHERE (status = 'queued' AND run_at <= now()) OR (status = 'running' AND locked_until < now()) \
                ORDER BY run_at LIMIT $1 FOR UPDATE SKIP LOCKED \
             ) \
             UPDATE jobs j SET status = 'running', attempts = j.attempts + 1, \
                 locked_until = now() + make_interval(secs => $2), lease_token = gen_random_uuid() \
             FROM picked WHERE j.id = picked.id \
             RETURNING j.id, j.kind, j.payload, j.attempts, j.max_attempts, j.run_at, j.created_at, j.lease_token",
        )
        .bind(limit)
        .bind(lease.as_secs_f64())
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn finish(
        &mut self,
        id: JobId,
        status: JobStatus,
        error: Option<&str>,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE jobs SET status = $2, last_error = COALESCE($3, last_error), locked_until = NULL, lease_token = NULL, \
             done_at = CASE WHEN $2 = 'done' THEN now() ELSE done_at END WHERE id = $1",
        )
        .bind(id.0)
        .bind(status.as_str())
        .bind(error)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn complete(&mut self, id: JobId) -> StoreResult<()> {
        self.finish(id, JobStatus::Done, None).await
    }

    pub async fn review(&mut self, id: JobId, error: &str) -> StoreResult<()> {
        self.finish(id, JobStatus::Review, Some(error)).await
    }

    pub async fn fail(&mut self, id: JobId, error: &str) -> StoreResult<()> {
        self.finish(id, JobStatus::Failed, Some(error)).await
    }

    pub async fn retry(
        &mut self,
        id: JobId,
        error: &str,
        run_at: DateTime<Utc>,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE jobs SET status = 'queued', last_error = $2, run_at = $3, locked_until = NULL, lease_token = NULL \
             WHERE id = $1",
        )
        .bind(id.0)
        .bind(error)
        .bind(run_at)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    async fn finish_leased(
        &mut self,
        job: &JobRecord,
        status: JobStatus,
        error: Option<&str>,
    ) -> StoreResult<bool> {
        let Some(token) = job.lease_token else {
            return Ok(false);
        };
        let done = sqlx::query(
            "UPDATE jobs SET status = $3, last_error = COALESCE($4, last_error), locked_until = NULL, lease_token = NULL, \
             done_at = CASE WHEN $3 = 'done' THEN now() ELSE done_at END \
             WHERE id = $1 AND lease_token = $2 AND status = 'running'",
        )
        .bind(job.id.0)
        .bind(token)
        .bind(status.as_str())
        .bind(error)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn complete_leased(&mut self, job: &JobRecord) -> StoreResult<bool> {
        self.finish_leased(job, JobStatus::Done, None).await
    }

    pub async fn review_leased(&mut self, job: &JobRecord, error: &str) -> StoreResult<bool> {
        self.finish_leased(job, JobStatus::Review, Some(error))
            .await
    }

    pub async fn fail_leased(&mut self, job: &JobRecord, error: &str) -> StoreResult<bool> {
        self.finish_leased(job, JobStatus::Failed, Some(error))
            .await
    }

    pub async fn retry_leased(
        &mut self,
        job: &JobRecord,
        error: &str,
        run_at: DateTime<Utc>,
    ) -> StoreResult<bool> {
        let Some(token) = job.lease_token else {
            return Ok(false);
        };
        let done = sqlx::query(
            "UPDATE jobs SET status = 'queued', last_error = $3, run_at = $4, locked_until = NULL, lease_token = NULL \
             WHERE id = $1 AND lease_token = $2 AND status = 'running'",
        )
        .bind(job.id.0)
        .bind(token)
        .bind(error)
        .bind(run_at)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn extend_lease(&mut self, job: &JobRecord, lease: Duration) -> StoreResult<bool> {
        let Some(token) = job.lease_token else {
            return Ok(false);
        };
        let done = sqlx::query(
            "UPDATE jobs SET locked_until = now() + make_interval(secs => $3) \
             WHERE id = $1 AND lease_token = $2 AND status = 'running'",
        )
        .bind(job.id.0)
        .bind(token)
        .bind(lease.as_secs_f64())
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn requeue(&mut self, id: JobId) -> StoreResult<()> {
        sqlx::query(
            "UPDATE jobs j SET status = 'queued', run_at = now(), locked_until = NULL, lease_token = NULL, attempts = 0, \
             done_at = NULL WHERE j.id = $1 AND j.status IN ('failed', 'review') \
             AND (j.dedupe_key IS NULL OR NOT EXISTS (SELECT 1 FROM jobs a WHERE a.dedupe_key = j.dedupe_key \
                 AND a.status IN ('queued', 'running')))",
        )
        .bind(id.0)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    async fn active(&mut self, key: &str) -> StoreResult<Option<JobId>> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM jobs WHERE dedupe_key = $1 AND status IN ('queued', 'running') LIMIT 1",
        )
        .bind(key)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(id.map(JobId))
    }

    pub async fn ensure_queued(&mut self, payload: &JobPayload) -> StoreResult<QueuedJob> {
        let Some(key) = payload.dedupe_key() else {
            return self
                .enqueue(payload, None)
                .await?
                .map(QueuedJob::Created)
                .ok_or_else(|| StoreError::Invalid("задача не поставлена".into()));
        };
        if let Some(id) = self.active(&key).await? {
            return Ok(QueuedJob::Active(id));
        }
        let requeued: Option<i64> = sqlx::query_scalar(
            "UPDATE jobs SET status = 'queued', run_at = now(), locked_until = NULL, lease_token = NULL, attempts = 0, \
             done_at = NULL WHERE id = (SELECT id FROM jobs WHERE dedupe_key = $1 ORDER BY id DESC LIMIT 1) \
             AND status NOT IN ('queued', 'running') RETURNING id",
        )
        .bind(&key)
        .fetch_optional(&mut *self.conn)
        .await?;
        if let Some(id) = requeued {
            return Ok(QueuedJob::Requeued(JobId(id)));
        }
        if let Some(id) = self.enqueue(payload, None).await? {
            return Ok(QueuedJob::Created(id));
        }
        self.active(&key)
            .await?
            .map(QueuedJob::Active)
            .ok_or_else(|| StoreError::Conflict {
                constraint: None,
                message: "задача меняется параллельно, повторите".into(),
            })
    }

    pub async fn stalled_fulfillments(
        &mut self,
        paid_before: DateTime<Utc>,
        quiet_since: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<OrderId>> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT o.id FROM orders o WHERE o.status = 'paid' AND o.updated_at < $1 \
             AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.dedupe_key = $2 || o.id::text \
                 AND (j.status IN ('queued', 'running', 'failed') OR j.updated_at >= $3)) \
             ORDER BY o.updated_at, o.id LIMIT $4",
        )
        .bind(paid_before)
        .bind(fulfill_key_prefix())
        .bind(quiet_since)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(ids.into_iter().map(OrderId).collect())
    }

    pub async fn counts(&mut self) -> StoreResult<Vec<(String, i64)>> {
        Ok(
            sqlx::query_as("SELECT status, count(*) FROM jobs GROUP BY status ORDER BY status")
                .fetch_all(&mut *self.conn)
                .await?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_store;
    use crate::{Db, NewClient};
    use sn_domain::client::{Locale, RegistrationSource};
    use sn_domain::ids::ClientId;
    use sn_domain::money::{Currency, Money};
    use sn_domain::order::{NewOrder, OrderKind, OrderStatus, PurchaseIntent};
    use sn_domain::pricing::Quote;

    async fn paid_order(db: &mut impl Db) -> OrderId {
        let client: ClientId = db
            .clients()
            .create(&NewClient {
                locale: Locale::Ru,
                registration_source: RegistrationSource::Bot,
                note: None,
            })
            .await
            .unwrap()
            .id;
        let amount = Money::new(299, Currency::Usd);
        let order = db
            .orders()
            .create(&NewOrder {
                client_id: client,
                subscription_id: None,
                kind: OrderKind::TopUp,
                intent: PurchaseIntent::TopUp { amount },
                quote: Quote {
                    currency: Currency::Usd,
                    lines: Vec::new(),
                    subtotal: amount,
                    discount: Money::zero(Currency::Usd),
                    total: amount,
                    stars: None,
                    days: 0,
                    extra_devices: 0,
                },
                snapshot: None,
            })
            .await
            .unwrap();
        db.orders()
            .transition(order.id, OrderStatus::Paid)
            .await
            .unwrap();
        order.id
    }

    async fn status(db: &mut impl Db, id: JobId) -> (String, i32) {
        sqlx::query_as("SELECT status, attempts FROM jobs WHERE id = $1")
            .bind(id.0)
            .fetch_one(db.conn())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn задачу_выдачи_после_ручной_проверки_можно_поставить_снова() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let payload = JobPayload::FulfillOrder {
            order_id: OrderId(7),
        };
        let first = conn.jobs().enqueue(&payload, None).await.unwrap().unwrap();
        let claimed = conn.jobs().claim(1, Duration::from_secs(60)).await.unwrap();
        assert_eq!(claimed[0].id, first);
        conn.jobs()
            .review(first, "попытки кончились")
            .await
            .unwrap();
        let again = conn.jobs().enqueue(&payload, None).await.unwrap();
        assert!(again.is_some_and(|id| id != first));
        assert!(conn.jobs().enqueue(&payload, None).await.unwrap().is_none());
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn итог_по_чужой_аренде_не_записывается() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let id = conn
            .jobs()
            .enqueue(&JobPayload::ProcessRemnaEvent { event_id: 9 }, None)
            .await
            .unwrap()
            .unwrap();
        let first = conn
            .jobs()
            .claim(1, Duration::from_millis(1))
            .await
            .unwrap()
            .remove(0);
        assert!(first.lease_token.is_some());
        tokio::time::sleep(Duration::from_millis(30)).await;
        let second = conn
            .jobs()
            .claim(1, Duration::from_secs(60))
            .await
            .unwrap()
            .remove(0);
        assert_eq!(second.id, id);
        assert_eq!(second.attempts, 2);
        assert_ne!(first.lease_token, second.lease_token);
        assert!(
            !conn
                .jobs()
                .extend_lease(&first, Duration::from_secs(60))
                .await
                .unwrap()
        );
        assert!(!conn.jobs().complete_leased(&first).await.unwrap());
        assert!(!conn.jobs().review_leased(&first, "чужая").await.unwrap());
        assert!(!conn.jobs().fail_leased(&first, "чужая").await.unwrap());
        assert_eq!(status(&mut conn, id).await, ("running".to_string(), 2));
        assert!(
            conn.jobs()
                .extend_lease(&second, Duration::from_secs(60))
                .await
                .unwrap()
        );
        assert!(
            conn.jobs()
                .retry_leased(&second, "повтор", Utc::now())
                .await
                .unwrap()
        );
        assert!(!conn.jobs().complete_leased(&second).await.unwrap());
        assert!(
            !conn
                .jobs()
                .retry_leased(&first, "повтор", Utc::now())
                .await
                .unwrap()
        );
        assert_eq!(status(&mut conn, id).await, ("queued".to_string(), 2));
        let third = conn
            .jobs()
            .claim(1, Duration::from_secs(60))
            .await
            .unwrap()
            .remove(0);
        assert!(conn.jobs().complete_leased(&third).await.unwrap());
        assert_eq!(
            conn.jobs().counts().await.unwrap(),
            vec![("done".to_string(), 1)]
        );
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn задача_ставится_снова_со_сбросом_попыток_без_второй_активной() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let payload = JobPayload::FulfillOrder {
            order_id: OrderId(11),
        };
        let created = conn.jobs().ensure_queued(&payload).await.unwrap();
        assert!(matches!(created, QueuedJob::Created(_)));
        assert!(created.armed());
        assert_eq!(
            conn.jobs().ensure_queued(&payload).await.unwrap(),
            QueuedJob::Active(created.id())
        );
        let claimed = conn
            .jobs()
            .claim(1, Duration::from_secs(60))
            .await
            .unwrap()
            .remove(0);
        let busy = conn.jobs().ensure_queued(&payload).await.unwrap();
        assert_eq!(busy, QueuedJob::Active(created.id()));
        assert!(!busy.armed());
        assert!(
            conn.jobs()
                .review_leased(&claimed, "попытки кончились")
                .await
                .unwrap()
        );
        let requeued = conn.jobs().ensure_queued(&payload).await.unwrap();
        assert_eq!(requeued, QueuedJob::Requeued(created.id()));
        assert_eq!(requeued.state(), "requeued");
        assert_eq!(
            status(&mut conn, created.id()).await,
            ("queued".to_string(), 0)
        );
        assert_eq!(
            conn.jobs().counts().await.unwrap(),
            vec![("queued".to_string(), 1)]
        );
        let broken = JobPayload::FulfillOrder {
            order_id: OrderId(12),
        };
        let other = conn.jobs().enqueue(&broken, None).await.unwrap().unwrap();
        conn.jobs().fail(other, "сломано").await.unwrap();
        let duplicate = conn.jobs().enqueue(&broken, None).await.unwrap().unwrap();
        conn.jobs().requeue(other).await.unwrap();
        assert_eq!(status(&mut conn, other).await.0, "failed");
        conn.jobs().complete(duplicate).await.unwrap();
        conn.jobs().requeue(other).await.unwrap();
        assert_eq!(status(&mut conn, other).await, ("queued".to_string(), 0));
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn оплаченный_заказ_без_активной_задачи_находится_после_затишья() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let order = paid_order(&mut conn).await;
        let fulfilled = paid_order(&mut conn).await;
        conn.orders()
            .transition(fulfilled, OrderStatus::Fulfilled)
            .await
            .unwrap();
        let now = Utc::now();
        let later = now + chrono::Duration::hours(2);
        let quiet = |at: DateTime<Utc>| at - chrono::Duration::minutes(30);
        assert!(
            conn.jobs()
                .stalled_fulfillments(now - chrono::Duration::minutes(1), quiet(now), 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            conn.jobs()
                .stalled_fulfillments(later, quiet(later), 10)
                .await
                .unwrap(),
            vec![order]
        );
        let payload = JobPayload::FulfillOrder { order_id: order };
        conn.jobs().enqueue(&payload, None).await.unwrap().unwrap();
        assert!(
            conn.jobs()
                .stalled_fulfillments(later, quiet(later), 10)
                .await
                .unwrap()
                .is_empty()
        );
        let claimed = conn
            .jobs()
            .claim(1, Duration::from_secs(60))
            .await
            .unwrap()
            .remove(0);
        conn.jobs()
            .review_leased(&claimed, "попытки кончились")
            .await
            .unwrap();
        assert!(
            conn.jobs()
                .stalled_fulfillments(later, quiet(now + chrono::Duration::minutes(10)), 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            conn.jobs()
                .stalled_fulfillments(later, quiet(later), 10)
                .await
                .unwrap(),
            vec![order]
        );
        conn.jobs().fail(claimed.id, "не найдено").await.unwrap();
        assert!(
            conn.jobs()
                .stalled_fulfillments(later, quiet(later), 10)
                .await
                .unwrap()
                .is_empty()
        );
        drop(conn);
        db.cleanup().await;
    }
}
