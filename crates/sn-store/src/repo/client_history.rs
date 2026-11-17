use super::audit::{AuditRecord, AuditRepo, audit_entity};
use super::clients::ClientsRepo;
use super::events::{EventsRepo, StoredEvent};
use super::orders::OrdersRepo;
use super::payments::PaymentsRepo;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{ClientId, EventId, OrderId, PaymentId};
use sn_domain::order::Order;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::Payment;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "type", content = "record", rename_all = "snake_case")]
pub enum ClientHistoryEntry {
    Payment(Payment),
    Order(Box<Order>),
    Event(StoredEvent),
    Audit(AuditRecord),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ClientHistoryItem {
    pub at: DateTime<Utc>,
    pub entry: ClientHistoryEntry,
}

const SOURCE_AUDIT: i32 = 1;
const SOURCE_EVENT: i32 = 2;
const SOURCE_PAYMENT: i32 = 3;
const SOURCE_ORDER: i32 = 4;

fn feed_sql() -> String {
    format!(
        "SELECT {SOURCE_AUDIT} AS src, a.id, a.created_at AS at FROM audit_log a \
         WHERE (a.entity_kind, a.entity_id) IN ( \
             SELECT '{client}', $1::bigint \
             UNION ALL SELECT '{subscription}', id FROM subscriptions WHERE client_id = $1 \
             UNION ALL SELECT '{order}', id FROM orders WHERE client_id = $1 \
             UNION ALL SELECT '{payment}', id FROM payments WHERE client_id = $1 \
             UNION ALL SELECT '{ticket}', id FROM tickets WHERE client_id = $1) \
         OR (a.actor_kind = 'client' AND a.actor_id = $1) \
         UNION ALL SELECT {SOURCE_EVENT}, e.id, e.occurred_at FROM domain_events e WHERE e.client_id = $1 \
         UNION ALL SELECT {SOURCE_PAYMENT}, p.id, COALESCE(p.paid_at, p.created_at) FROM payments p WHERE p.client_id = $1 \
         UNION ALL SELECT {SOURCE_ORDER}, o.id, o.created_at FROM orders o WHERE o.client_id = $1",
        client = audit_entity::CLIENT,
        subscription = audit_entity::SUBSCRIPTION,
        order = audit_entity::ORDER,
        payment = audit_entity::PAYMENT,
        ticket = audit_entity::TICKET,
    )
}

#[derive(sqlx::FromRow)]
struct FeedRow {
    src: i32,
    id: i64,
    at: DateTime<Utc>,
    more: bool,
}

impl<'c> ClientsRepo<'c> {
    pub async fn history(
        &mut self,
        id: ClientId,
        page: PageRequest,
    ) -> StoreResult<Page<ClientHistoryItem>> {
        let before = page
            .after
            .map(|micros| {
                DateTime::<Utc>::from_timestamp_micros(micros)
                    .ok_or_else(|| StoreError::Invalid("неверная позиция в истории".into()))
            })
            .transpose()?;
        let rows: Vec<FeedRow> = sqlx::query_as(&format!(
            "WITH feed AS ({feed}), \
             page AS (SELECT src, id, at FROM feed WHERE $2::timestamptz IS NULL OR at < $2), \
             bound AS (SELECT at FROM page ORDER BY at DESC OFFSET $3 LIMIT 1) \
             SELECT p.src, p.id, p.at, EXISTS (SELECT 1 FROM page q, bound b WHERE q.at < b.at) AS more \
             FROM page p WHERE p.at >= COALESCE((SELECT at FROM bound), '-infinity'::timestamptz) \
             ORDER BY p.at DESC, p.src, p.id DESC",
            feed = feed_sql()
        ))
        .bind(id.0)
        .bind(before)
        .bind(i64::from(page.limit.max(1)) - 1)
        .fetch_all(&mut *self.conn)
        .await?;
        let ids = |source: i32| -> Vec<i64> {
            rows.iter()
                .filter(|row| row.src == source)
                .map(|row| row.id)
                .collect()
        };
        let payment_ids: Vec<PaymentId> = ids(SOURCE_PAYMENT).into_iter().map(PaymentId).collect();
        let order_ids: Vec<OrderId> = ids(SOURCE_ORDER).into_iter().map(OrderId).collect();
        let event_ids: Vec<EventId> = ids(SOURCE_EVENT).into_iter().map(EventId).collect();
        let audit_ids = ids(SOURCE_AUDIT);
        let mut payments: HashMap<i64, Payment> = PaymentsRepo::new(&mut *self.conn)
            .by_ids(&payment_ids)
            .await?
            .into_iter()
            .map(|p| (p.id.0, p))
            .collect();
        let mut orders: HashMap<i64, Order> = OrdersRepo::new(&mut *self.conn)
            .by_ids(&order_ids)
            .await?
            .into_iter()
            .map(|o| (o.id.0, o))
            .collect();
        let mut events: HashMap<i64, StoredEvent> = EventsRepo::new(&mut *self.conn)
            .by_ids(&event_ids)
            .await?
            .into_iter()
            .map(|e| (e.id.0, e))
            .collect();
        let mut audit: HashMap<i64, AuditRecord> = AuditRepo::new(&mut *self.conn)
            .by_ids(&audit_ids)
            .await?
            .into_iter()
            .map(|a| (a.id, a))
            .collect();
        let more = rows.first().is_some_and(|row| row.more);
        let next = if more {
            rows.last().map(|row| row.at.timestamp_micros())
        } else {
            None
        };
        let items = rows
            .iter()
            .filter_map(|row| {
                let entry = match row.src {
                    SOURCE_PAYMENT => payments.remove(&row.id).map(ClientHistoryEntry::Payment),
                    SOURCE_ORDER => orders
                        .remove(&row.id)
                        .map(|order| ClientHistoryEntry::Order(Box::new(order))),
                    SOURCE_EVENT => events.remove(&row.id).map(ClientHistoryEntry::Event),
                    _ => audit.remove(&row.id).map(ClientHistoryEntry::Audit),
                }?;
                Some(ClientHistoryItem { at: row.at, entry })
            })
            .collect();
        Ok(Page { items, next })
    }

    pub async fn history_count(&mut self, id: ClientId) -> StoreResult<i64> {
        Ok(
            sqlx::query_scalar(&format!("SELECT count(*) FROM ({}) f", feed_sql()))
                .bind(id.0)
                .fetch_one(&mut *self.conn)
                .await?,
        )
    }
}
