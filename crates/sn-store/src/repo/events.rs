use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{ClientId, EventId};
use sn_domain::module::ModuleId;
use sqlx::PgConnection;
use sqlx::types::Json;

const DISPATCH_LOCK: i32 = 0x5e_7e;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Done,
    Skipped,
    Review,
}

impl DeliveryStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            DeliveryStatus::Pending => "pending",
            DeliveryStatus::Done => "done",
            DeliveryStatus::Skipped => "skipped",
            DeliveryStatus::Review => "review",
        }
    }

    fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "pending" => Ok(DeliveryStatus::Pending),
            "done" => Ok(DeliveryStatus::Done),
            "skipped" => Ok(DeliveryStatus::Skipped),
            "review" => Ok(DeliveryStatus::Review),
            other => Err(StoreError::Decode(format!(
                "неизвестный статус доставки события: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Delivery {
    pub event_id: EventId,
    pub module: ModuleId,
    pub status: DeliveryStatus,
    pub attempts: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl Delivery {
    pub fn new(event_id: EventId, module: ModuleId) -> Self {
        Delivery {
            event_id,
            module,
            status: DeliveryStatus::Pending,
            attempts: 0,
            next_attempt_at: None,
            last_error: None,
        }
    }
}

type DeliveryRow = (
    i64,
    String,
    String,
    i32,
    Option<DateTime<Utc>>,
    Option<String>,
);

fn delivery(row: DeliveryRow) -> StoreResult<Option<Delivery>> {
    let (event_id, module, status, attempts, next_attempt_at, last_error) = row;
    let Ok(module) = ModuleId::parse(&module) else {
        return Ok(None);
    };
    Ok(Some(Delivery {
        event_id: EventId(event_id),
        module,
        status: DeliveryStatus::parse(&status)?,
        attempts: u32::try_from(attempts.max(0)).unwrap_or(u32::MAX),
        next_attempt_at,
        last_error,
    }))
}

fn deliveries(rows: Vec<DeliveryRow>) -> StoreResult<Vec<Delivery>> {
    Ok(rows
        .into_iter()
        .map(delivery)
        .collect::<StoreResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect())
}

const DELIVERY_COLUMNS: &str = "event_id, module, status, attempts, next_attempt_at, last_error";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StoredEvent {
    pub id: EventId,
    pub event: DomainEvent,
    pub occurred_at: DateTime<Utc>,
}

pub struct EventsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> EventsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        EventsRepo { conn }
    }

    pub async fn push(&mut self, event: &DomainEvent) -> StoreResult<EventId> {
        let id: i64 = sqlx::query_scalar(
            "WITH e AS (INSERT INTO domain_events (kind, client_id, payload) VALUES ($1, $2, $3) \
             RETURNING id, occurred_at), \
             queued AS (INSERT INTO ext_module_deliveries (module_id, event_id, kind, payload, occurred_at) \
             SELECT m.id, e.id, $1, $3, e.occurred_at FROM ext_modules m, e \
             WHERE m.enabled AND $1 = ANY (m.events)) \
             SELECT id FROM e",
        )
        .bind(event.kind())
        .bind(event.client_id().0)
        .bind(Json(event))
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(EventId(id))
    }

    pub async fn claim_pending(&mut self, limit: i64) -> StoreResult<Vec<StoredEvent>> {
        let rows: Vec<(i64, Json<DomainEvent>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, payload, occurred_at FROM domain_events WHERE dispatched_at IS NULL \
             ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED",
        )
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, event, occurred_at)| StoredEvent {
                id: EventId(id),
                event: event.0,
                occurred_at,
            })
            .collect())
    }

    pub async fn try_lock_dispatch(&mut self) -> StoreResult<bool> {
        let locked: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1, hashtext(current_schema()))")
                .bind(DISPATCH_LOCK)
                .fetch_one(&mut *self.conn)
                .await?;
        Ok(locked)
    }

    pub async fn claim_due(
        &mut self,
        now: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<StoredEvent>> {
        let rows: Vec<(i64, Json<DomainEvent>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, payload, occurred_at FROM domain_events WHERE dispatched_at IS NULL \
             AND (retry_at IS NULL OR retry_at <= $1) \
             ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, event, occurred_at)| StoredEvent {
                id: EventId(id),
                event: event.0,
                occurred_at,
            })
            .collect())
    }

    pub async fn deliveries(&mut self, ids: &[EventId]) -> StoreResult<Vec<Delivery>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<DeliveryRow> = sqlx::query_as(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM event_deliveries WHERE event_id = ANY($1) \
             ORDER BY event_id, module"
        ))
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        deliveries(rows)
    }

    pub async fn save_delivery(&mut self, delivery: &Delivery) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO event_deliveries (event_id, module, status, attempts, next_attempt_at, last_error) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (event_id, module) DO UPDATE SET status = EXCLUDED.status, \
             attempts = EXCLUDED.attempts, next_attempt_at = EXCLUDED.next_attempt_at, \
             last_error = EXCLUDED.last_error, updated_at = now()",
        )
        .bind(delivery.event_id.0)
        .bind(delivery.module.as_str())
        .bind(delivery.status.as_str())
        .bind(i32::try_from(delivery.attempts).unwrap_or(i32::MAX))
        .bind(delivery.next_attempt_at)
        .bind(delivery.last_error.as_deref())
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn waiting_modules(&mut self) -> StoreResult<Vec<(ModuleId, ClientId)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT DISTINCT d.module, e.client_id FROM event_deliveries d \
             JOIN domain_events e ON e.id = d.event_id \
             WHERE d.status = 'pending' AND e.client_id IS NOT NULL",
        )
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(module, client)| {
                ModuleId::parse(&module)
                    .ok()
                    .map(|module| (module, ClientId(client)))
            })
            .collect())
    }

    pub async fn blocked_until(
        &mut self,
        module: ModuleId,
        client_id: ClientId,
        before: EventId,
    ) -> StoreResult<Option<DateTime<Utc>>> {
        let next: Option<Option<DateTime<Utc>>> = sqlx::query_scalar(
            "SELECT d.next_attempt_at FROM event_deliveries d \
             JOIN domain_events e ON e.id = d.event_id \
             WHERE d.module = $1 AND d.status = 'pending' AND e.client_id = $2 AND d.event_id < $3 \
             ORDER BY d.event_id LIMIT 1",
        )
        .bind(module.as_str())
        .bind(client_id.0)
        .bind(before.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(next.flatten())
    }

    pub async fn defer(&mut self, id: EventId, until: DateTime<Utc>) -> StoreResult<()> {
        sqlx::query("UPDATE domain_events SET retry_at = $2 WHERE id = $1")
            .bind(id.0)
            .bind(until)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn in_review(&mut self, limit: i64) -> StoreResult<Vec<Delivery>> {
        let rows: Vec<DeliveryRow> = sqlx::query_as(&format!(
            "SELECT {DELIVERY_COLUMNS} FROM event_deliveries WHERE status = 'review' \
             ORDER BY updated_at DESC, event_id DESC LIMIT $1"
        ))
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        deliveries(rows)
    }

    pub async fn retry_delivery(
        &mut self,
        event_id: EventId,
        module: ModuleId,
        at: DateTime<Utc>,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE event_deliveries SET status = 'pending', attempts = 0, next_attempt_at = $3, \
             updated_at = now() WHERE event_id = $1 AND module = $2 AND status IN ('review', 'skipped')",
        )
        .bind(event_id.0)
        .bind(module.as_str())
        .bind(at)
        .execute(&mut *self.conn)
        .await?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query("UPDATE domain_events SET dispatched_at = NULL, retry_at = $2 WHERE id = $1")
            .bind(event_id.0)
            .bind(at)
            .execute(&mut *self.conn)
            .await?;
        Ok(true)
    }

    pub async fn mark_dispatched(&mut self, ids: &[EventId]) -> StoreResult<()> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        sqlx::query("UPDATE domain_events SET dispatched_at = now() WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn by_ids(&mut self, ids: &[EventId]) -> StoreResult<Vec<StoredEvent>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<(i64, Json<serde_json::Value>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, payload, occurred_at FROM domain_events WHERE id = ANY($1) ORDER BY id",
        )
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, payload, occurred_at)| {
                match serde_json::from_value::<DomainEvent>(payload.0) {
                    Ok(event) => Some(StoredEvent {
                        id: EventId(id),
                        event,
                        occurred_at,
                    }),
                    Err(err) => {
                        tracing::warn!(event = id, error = %err, "событие не разобрано и пропущено");
                        None
                    }
                }
            })
            .collect())
    }

    pub async fn for_client(
        &mut self,
        client_id: ClientId,
        limit: i64,
    ) -> StoreResult<Vec<StoredEvent>> {
        let rows: Vec<(i64, Json<DomainEvent>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, payload, occurred_at FROM domain_events WHERE client_id = $1 ORDER BY id DESC LIMIT $2",
        )
        .bind(client_id.0)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, event, occurred_at)| StoredEvent {
                id: EventId(id),
                event: event.0,
                occurred_at,
            })
            .collect())
    }
}
