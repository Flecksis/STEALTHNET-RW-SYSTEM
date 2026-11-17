use super::payments::TRANSITION_ATTEMPTS;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{ClientId, OrderId, SubscriptionId};
use sn_domain::order::{NewOrder, Order, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::plan::FulfillmentPlan;
use sn_domain::pricing::Quote;
use sn_domain::tariff::TariffSnapshot;
use sqlx::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct OrderRow {
    id: i64,
    public_id: Uuid,
    client_id: i64,
    subscription_id: Option<i64>,
    kind: String,
    status: String,
    intent: Json<PurchaseIntent>,
    quote: Json<Quote>,
    snapshot: Option<Json<TariffSnapshot>>,
    plan: Option<Json<FulfillmentPlan>>,
    created_at: DateTime<Utc>,
    paid_at: Option<DateTime<Utc>>,
    fulfilled_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id, public_id, client_id, subscription_id, kind, status, intent, quote, snapshot, plan, \
                       created_at, paid_at, fulfilled_at";

impl TryFrom<OrderRow> for Order {
    type Error = StoreError;

    fn try_from(row: OrderRow) -> Result<Self, Self::Error> {
        Ok(Order {
            id: OrderId(row.id),
            public_id: row.public_id,
            client_id: ClientId(row.client_id),
            subscription_id: row.subscription_id.map(SubscriptionId),
            kind: OrderKind::parse(&row.kind)?,
            status: OrderStatus::parse(&row.status)?,
            intent: row.intent.0,
            quote: row.quote.0,
            snapshot: row.snapshot.map(|s| s.0),
            plan: row.plan.map(|p| p.0),
            created_at: row.created_at,
            paid_at: row.paid_at,
            fulfilled_at: row.fulfilled_at,
        })
    }
}

pub struct OrdersRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> OrdersRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        OrdersRepo { conn }
    }

    pub async fn create(&mut self, order: &NewOrder) -> StoreResult<Order> {
        let row: OrderRow = sqlx::query_as(&format!(
            "INSERT INTO orders (client_id, subscription_id, kind, intent, quote, snapshot, currency, total_minor, stars_total) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {COLUMNS}"
        ))
        .bind(order.client_id.0)
        .bind(order.subscription_id.map(|s| s.0))
        .bind(order.kind.as_str())
        .bind(Json(&order.intent))
        .bind(Json(&order.quote))
        .bind(order.snapshot.as_ref().map(Json))
        .bind(order.quote.currency.code())
        .bind(order.quote.total.minor)
        .bind(order.quote.stars.map(|s| s.total.0))
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    async fn find(&mut self, sql: &str, id: i64) -> StoreResult<Order> {
        let row: OrderRow = sqlx::query_as(sql)
            .bind(id)
            .fetch_optional(&mut *self.conn)
            .await?
            .ok_or(StoreError::NotFound("заказ"))?;
        row.try_into()
    }

    pub async fn get(&mut self, id: OrderId) -> StoreResult<Order> {
        self.find(&format!("SELECT {COLUMNS} FROM orders WHERE id = $1"), id.0)
            .await
    }

    pub async fn lock(&mut self, id: OrderId) -> StoreResult<Order> {
        self.find(
            &format!("SELECT {COLUMNS} FROM orders WHERE id = $1 FOR UPDATE"),
            id.0,
        )
        .await
    }

    pub async fn by_public_id(&mut self, public_id: Uuid) -> StoreResult<Order> {
        let row: OrderRow = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM orders WHERE public_id = $1"
        ))
        .bind(public_id)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("заказ"))?;
        row.try_into()
    }

    pub async fn transition(&mut self, id: OrderId, to: OrderStatus) -> StoreResult<(Order, bool)> {
        let mut current = self.lock(id).await?;
        for _ in 0..TRANSITION_ATTEMPTS {
            if !current.status.transition(to)? {
                return Ok((current, false));
            }
            let row: Option<OrderRow> = sqlx::query_as(&format!(
                "UPDATE orders SET status = $2, \
                 paid_at = CASE WHEN $2 = 'paid' THEN COALESCE(paid_at, now()) ELSE paid_at END, \
                 fulfilled_at = CASE WHEN $2 = 'fulfilled' THEN now() ELSE fulfilled_at END, \
                 canceled_at = CASE WHEN $2 = 'canceled' THEN now() ELSE canceled_at END \
                 WHERE id = $1 AND status = $3 RETURNING {COLUMNS}"
            ))
            .bind(id.0)
            .bind(to.as_str())
            .bind(current.status.as_str())
            .fetch_optional(&mut *self.conn)
            .await?;
            match row {
                Some(row) => return Ok((row.try_into()?, true)),
                None => current = self.lock(id).await?,
            }
        }
        Err(StoreError::Conflict {
            constraint: None,
            message: "статус заказа меняется параллельно, повторите".into(),
        })
    }

    pub async fn by_ids(&mut self, ids: &[OrderId]) -> StoreResult<Vec<Order>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<OrderRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM orders WHERE id = ANY($1) ORDER BY id"
        ))
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn save_plan(
        &mut self,
        id: OrderId,
        plan: &FulfillmentPlan,
    ) -> StoreResult<FulfillmentPlan> {
        let stored: Option<Json<FulfillmentPlan>> = sqlx::query_scalar(
            "UPDATE orders SET plan = COALESCE(plan, $2) WHERE id = $1 RETURNING plan",
        )
        .bind(id.0)
        .bind(Json(plan))
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("заказ"))?;
        stored
            .map(|p| p.0)
            .ok_or_else(|| StoreError::Decode("план выдачи не сохранился".into()))
    }

    pub async fn set_subscription(
        &mut self,
        id: OrderId,
        subscription_id: SubscriptionId,
    ) -> StoreResult<()> {
        sqlx::query("UPDATE orders SET subscription_id = $2 WHERE id = $1")
            .bind(id.0)
            .bind(subscription_id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn mark_review(&mut self, id: OrderId, reason: &str) -> StoreResult<()> {
        let (order, _) = self.transition(id, OrderStatus::Review).await?;
        sqlx::query("UPDATE orders SET review_reason = $2 WHERE id = $1")
            .bind(order.id.0)
            .bind(reason)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn for_client(
        &mut self,
        client_id: ClientId,
        page: PageRequest,
    ) -> StoreResult<Page<Order>> {
        let rows: Vec<OrderRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM orders WHERE client_id = $1 AND ($2::bigint IS NULL OR id < $2) ORDER BY id DESC LIMIT $3"
        ))
        .bind(client_id.0)
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        let items = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<StoreResult<Vec<Order>>>()?;
        Ok(Page::from_overfetch(items, page, |o| o.id.0))
    }

    pub async fn needing_attention(&mut self, limit: i64) -> StoreResult<Vec<Order>> {
        let rows: Vec<OrderRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM orders WHERE status IN ('paid', 'review') ORDER BY updated_at LIMIT $1"
        ))
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }
}
