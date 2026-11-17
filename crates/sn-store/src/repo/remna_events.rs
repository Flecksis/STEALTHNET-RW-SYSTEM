use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq)]
pub struct NewRemnaEvent {
    pub body_sha256: Vec<u8>,
    pub scope: String,
    pub event: String,
    pub occurred_at: DateTime<Utc>,
    pub subject_kind: Option<String>,
    pub subject_id: Option<String>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct RemnaEventRecord {
    pub id: i64,
    pub scope: String,
    pub event: String,
    pub occurred_at: DateTime<Utc>,
    pub subject_kind: Option<String>,
    pub subject_id: Option<String>,
    pub payload: serde_json::Value,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str =
    "id, scope, event, occurred_at, subject_kind, subject_id, payload, received_at, processed_at";

pub struct RemnaEventsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> RemnaEventsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        RemnaEventsRepo { conn }
    }

    pub async fn insert(&mut self, event: &NewRemnaEvent) -> StoreResult<Option<i64>> {
        Ok(sqlx::query_scalar(
            "INSERT INTO remna_events (body_sha256, scope, event, occurred_at, subject_kind, subject_id, payload) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (body_sha256) DO NOTHING RETURNING id",
        )
        .bind(&event.body_sha256)
        .bind(&event.scope)
        .bind(&event.event)
        .bind(event.occurred_at)
        .bind(&event.subject_kind)
        .bind(&event.subject_id)
        .bind(&event.payload)
        .fetch_optional(&mut *self.conn)
        .await?)
    }

    pub async fn get(&mut self, id: i64) -> StoreResult<RemnaEventRecord> {
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM remna_events WHERE id = $1"))
            .bind(id)
            .fetch_optional(&mut *self.conn)
            .await?
            .ok_or(StoreError::NotFound("событие Remnawave"))
    }

    pub async fn mark_processed(&mut self, id: i64) -> StoreResult<()> {
        sqlx::query("UPDATE remna_events SET processed_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn feed(
        &mut self,
        before_id: Option<i64>,
        limit: i64,
    ) -> StoreResult<Vec<RemnaEventRecord>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM remna_events WHERE ($1::bigint IS NULL OR id < $1) ORDER BY id DESC LIMIT $2"
        ))
        .bind(before_id)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?)
    }

    pub async fn last_received_at(&mut self) -> StoreResult<Option<DateTime<Utc>>> {
        Ok(
            sqlx::query_scalar("SELECT max(received_at) FROM remna_events")
                .fetch_one(&mut *self.conn)
                .await?,
        )
    }
}
