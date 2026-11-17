use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::PaymentId;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxOutcome {
    Received,
    Accepted,
    Ignored,
    RejectedSignature,
    RejectedPayload,
    PaymentNotFound,
    AmountMismatch,
    Error,
}

impl InboxOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            InboxOutcome::Received => "received",
            InboxOutcome::Accepted => "accepted",
            InboxOutcome::Ignored => "ignored",
            InboxOutcome::RejectedSignature => "rejected_signature",
            InboxOutcome::RejectedPayload => "rejected_payload",
            InboxOutcome::PaymentNotFound => "payment_not_found",
            InboxOutcome::AmountMismatch => "amount_mismatch",
            InboxOutcome::Error => "error",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        Ok(match value {
            "received" => InboxOutcome::Received,
            "accepted" => InboxOutcome::Accepted,
            "ignored" => InboxOutcome::Ignored,
            "rejected_signature" => InboxOutcome::RejectedSignature,
            "rejected_payload" => InboxOutcome::RejectedPayload,
            "payment_not_found" => InboxOutcome::PaymentNotFound,
            "amount_mismatch" => InboxOutcome::AmountMismatch,
            "error" => InboxOutcome::Error,
            other => return Err(StoreError::Decode(format!("исход вебхука: {other}"))),
        })
    }

    pub const fn is_final(self) -> bool {
        matches!(
            self,
            InboxOutcome::Accepted
                | InboxOutcome::Ignored
                | InboxOutcome::PaymentNotFound
                | InboxOutcome::AmountMismatch
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInboundWebhook {
    pub source: String,
    pub dedupe_key: String,
    pub remote_ip: Option<String>,
    pub headers: serde_json::Value,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxRecord {
    pub id: i64,
    pub duplicate: bool,
    pub outcome: InboxOutcome,
}

pub struct WebhooksRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> WebhooksRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        WebhooksRepo { conn }
    }

    pub async fn record(&mut self, hook: &NewInboundWebhook) -> StoreResult<InboxRecord> {
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO webhook_inbox (source, dedupe_key, remote_ip, headers, body) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (source, dedupe_key) DO NOTHING RETURNING id",
        )
        .bind(&hook.source)
        .bind(&hook.dedupe_key)
        .bind(&hook.remote_ip)
        .bind(&hook.headers)
        .bind(&hook.body)
        .fetch_optional(&mut *self.conn)
        .await?;
        if let Some(id) = inserted {
            return Ok(InboxRecord {
                id,
                duplicate: false,
                outcome: InboxOutcome::Received,
            });
        }
        let (id, outcome): (i64, String) = sqlx::query_as(
            "SELECT id, outcome FROM webhook_inbox WHERE source = $1 AND dedupe_key = $2",
        )
        .bind(&hook.source)
        .bind(&hook.dedupe_key)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(InboxRecord {
            id,
            duplicate: true,
            outcome: InboxOutcome::parse(&outcome)?,
        })
    }

    pub async fn finish(
        &mut self,
        id: i64,
        outcome: InboxOutcome,
        payment_id: Option<PaymentId>,
        error: Option<&str>,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE webhook_inbox SET outcome = $2, payment_id = COALESCE($3, payment_id), error = $4, processed_at = now() \
             WHERE id = $1",
        )
        .bind(id)
        .bind(outcome.as_str())
        .bind(payment_id.map(|p| p.0))
        .bind(error)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn record_rejected(
        &mut self,
        hook: &NewInboundWebhook,
        outcome: InboxOutcome,
        error: Option<&str>,
    ) -> StoreResult<bool> {
        if matches!(outcome, InboxOutcome::Received) || outcome.is_final() {
            return Err(StoreError::Invalid(format!(
                "отказ вебхука не может иметь исход {}",
                outcome.as_str()
            )));
        }
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO webhook_inbox (source, dedupe_key, remote_ip, headers, body, outcome, error, processed_at) \
             VALUES ($1, $2, $3, $4, NULL, $5, $6, now()) ON CONFLICT (source, dedupe_key) DO NOTHING RETURNING id",
        )
        .bind(&hook.source)
        .bind(&hook.dedupe_key)
        .bind(&hook.remote_ip)
        .bind(&hook.headers)
        .bind(outcome.as_str())
        .bind(error)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(inserted.is_some())
    }

    pub async fn purge(
        &mut self,
        kept_since: DateTime<Utc>,
        rejected_kept_since: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<u64> {
        let rejected = sqlx::query(
            "DELETE FROM webhook_inbox WHERE id IN (SELECT id FROM webhook_inbox \
             WHERE (outcome IN ('rejected_signature', 'rejected_payload') OR body IS NULL) AND received_at < $1 \
             ORDER BY received_at LIMIT $2)",
        )
        .bind(rejected_kept_since)
        .bind(limit)
        .execute(&mut *self.conn)
        .await?
        .rows_affected();
        let expired = sqlx::query(
            "DELETE FROM webhook_inbox WHERE id IN (SELECT id FROM webhook_inbox \
             WHERE received_at < $1 ORDER BY received_at LIMIT $2)",
        )
        .bind(kept_since)
        .bind(limit)
        .execute(&mut *self.conn)
        .await?
        .rows_affected();
        Ok(rejected + expired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::testing::test_store;
    use chrono::Duration;

    fn hook(key: &str, body: Option<&[u8]>) -> NewInboundWebhook {
        NewInboundWebhook {
            source: "cryptopay".into(),
            dedupe_key: key.into(),
            remote_ip: Some("203.0.113.7".into()),
            headers: serde_json::json!({}),
            body: body.map(<[u8]>::to_vec),
        }
    }

    async fn keys(db: &mut impl Db) -> Vec<String> {
        sqlx::query_scalar("SELECT dedupe_key FROM webhook_inbox ORDER BY id")
            .fetch_all(db.conn())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn отказ_записывается_без_тела_и_один_раз() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let rejected = hook("rejected:aa", None);
        assert!(
            conn.webhooks()
                .record_rejected(&rejected, InboxOutcome::RejectedSignature, Some("подпись"))
                .await
                .unwrap()
        );
        assert!(
            !conn
                .webhooks()
                .record_rejected(&rejected, InboxOutcome::RejectedSignature, Some("подпись"))
                .await
                .unwrap()
        );
        assert!(
            conn.webhooks()
                .record_rejected(&hook("x", None), InboxOutcome::Accepted, None)
                .await
                .is_err()
        );
        let again = conn.webhooks().record(&rejected).await.unwrap();
        assert!(again.duplicate);
        assert_eq!(again.outcome, InboxOutcome::RejectedSignature);
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT body FROM webhook_inbox WHERE dedupe_key = 'rejected:aa'")
                .fetch_one(conn.conn())
                .await
                .unwrap();
        assert_eq!(stored, None);
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn старые_записи_удаляются_пачками_по_сроку() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let now = Utc::now();
        for (key, outcome, body, days) in [
            ("a-old", "accepted", Some(b"{}".as_slice()), 40),
            ("a-new", "accepted", Some(b"{}".as_slice()), 10),
            ("r-old", "rejected_signature", Some(b"junk".as_slice()), 4),
            ("r-new", "rejected_payload", None, 1),
            ("e-old", "error", None, 5),
            ("e-body", "error", Some(b"{}".as_slice()), 5),
        ] {
            sqlx::query(
                "INSERT INTO webhook_inbox (source, dedupe_key, outcome, body, received_at) \
                 VALUES ('cryptopay', $1, $2, $3, $4)",
            )
            .bind(key)
            .bind(outcome)
            .bind(body)
            .bind(now - Duration::days(days))
            .execute(conn.conn())
            .await
            .unwrap();
        }
        let purged = conn
            .webhooks()
            .purge(now - Duration::days(30), now - Duration::days(3), 1)
            .await
            .unwrap();
        assert_eq!(purged, 2);
        let purged = conn
            .webhooks()
            .purge(now - Duration::days(30), now - Duration::days(3), 100)
            .await
            .unwrap();
        assert_eq!(purged, 1);
        assert_eq!(keys(&mut conn).await, ["a-new", "r-new", "e-body"]);
        drop(conn);
        db.cleanup().await;
    }
}
