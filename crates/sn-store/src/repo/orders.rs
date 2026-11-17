use super::non_negative_u32;
use super::payments::TRANSITION_ATTEMPTS;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{ClientId, JobId, OrderId, SubscriptionId};
use sn_domain::jobs::{JobPayload, JobStatus};
use sn_domain::money::{Currency, Money, Stars};
use sn_domain::order::{NewOrder, Order, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::plan::FulfillmentPlan;
use sn_domain::pricing::Quote;
use sn_domain::tariff::TariffSnapshot;
use sqlx::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStage {
    None,
    Saved,
    Applied,
    Stale,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanState {
    pub saved: bool,
    pub applied_at: Option<DateTime<Utc>>,
    pub stale_at: Option<DateTime<Utc>>,
}

impl PlanState {
    pub const fn stage(&self) -> PlanStage {
        if !self.saved {
            PlanStage::None
        } else if self.applied_at.is_some() {
            PlanStage::Applied
        } else if self.stale_at.is_some() {
            PlanStage::Stale
        } else {
            PlanStage::Saved
        }
    }

    pub const fn applied(&self) -> bool {
        matches!(self.stage(), PlanStage::Applied)
    }

    pub const fn stale(&self) -> bool {
        matches!(self.stage(), PlanStage::Stale)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReset {
    pub plan: Option<FulfillmentPlan>,
    pub state: PlanState,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReviewJob {
    pub id: JobId,
    pub status: JobStatus,
    pub attempts: u32,
    pub error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl ReviewJob {
    pub const fn active(&self) -> bool {
        matches!(self.status, JobStatus::Queued | JobStatus::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReviewOrder {
    pub order_id: OrderId,
    pub public_id: Uuid,
    pub client_id: ClientId,
    pub subscription_id: Option<SubscriptionId>,
    pub kind: OrderKind,
    pub tariff: Option<String>,
    pub total: Money,
    pub stars: Option<Stars>,
    pub reason: Option<String>,
    pub plan: PlanStage,
    pub plan_base_expire_at: Option<DateTime<Utc>>,
    pub plan_target_expire_at: Option<DateTime<Utc>>,
    pub refund_pending: bool,
    pub job: Option<ReviewJob>,
    pub paid_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ReviewRow {
    id: i64,
    public_id: Uuid,
    client_id: i64,
    subscription_id: Option<i64>,
    kind: String,
    tariff: Option<String>,
    currency: String,
    total_minor: i64,
    stars_total: Option<i64>,
    review_reason: Option<String>,
    plan: Option<Json<FulfillmentPlan>>,
    plan_applied_at: Option<DateTime<Utc>>,
    plan_stale_at: Option<DateTime<Utc>>,
    refund_pending: bool,
    job_id: Option<i64>,
    job_status: Option<String>,
    job_attempts: Option<i32>,
    job_error: Option<String>,
    job_updated_at: Option<DateTime<Utc>>,
    paid_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<ReviewRow> for ReviewOrder {
    type Error = StoreError;

    fn try_from(row: ReviewRow) -> Result<Self, Self::Error> {
        let state = PlanState {
            saved: row.plan.is_some(),
            applied_at: row.plan_applied_at,
            stale_at: row.plan_stale_at,
        };
        let job = match (
            row.job_id,
            row.job_status,
            row.job_attempts,
            row.job_updated_at,
        ) {
            (Some(id), Some(status), Some(attempts), Some(updated_at)) => Some(ReviewJob {
                id: JobId(id),
                status: JobStatus::parse(&status)?,
                attempts: non_negative_u32(attempts, "attempts")?,
                error: row.job_error,
                updated_at,
            }),
            _ => None,
        };
        let plan = row.plan.map(|p| p.0);
        Ok(ReviewOrder {
            order_id: OrderId(row.id),
            public_id: row.public_id,
            client_id: ClientId(row.client_id),
            subscription_id: row.subscription_id.map(SubscriptionId),
            kind: OrderKind::parse(&row.kind)?,
            tariff: row.tariff,
            total: Money::new(row.total_minor, Currency::parse(&row.currency)?),
            stars: row.stars_total.map(Stars),
            reason: row.review_reason,
            plan: state.stage(),
            plan_base_expire_at: plan.as_ref().and_then(|p| p.base_expire_at),
            plan_target_expire_at: plan.as_ref().map(|p| p.target.expire_at),
            refund_pending: row.refund_pending,
            job,
            paid_at: row.paid_at,
            updated_at: row.updated_at,
        })
    }
}

pub(crate) const REFUND_PENDING: &str = "EXISTS (SELECT 1 FROM provider_refunds r \
     JOIN payments p ON p.id = r.payment_id WHERE p.order_id = o.id AND r.status = 'pending' \
     AND NOT EXISTS (SELECT 1 FROM payment_repeats x WHERE x.payment_id = p.id))";

const LIVE_REVIEW: &str =
    "(o.plan_applied_at IS NOT NULL OR (o.plan IS NOT NULL AND o.plan_stale_at IS NULL))";

pub(crate) fn fulfill_key_prefix() -> String {
    let probe = JobPayload::FulfillOrder {
        order_id: OrderId(0),
    }
    .dedupe_key()
    .unwrap_or_default();
    probe.strip_suffix('0').unwrap_or(&probe).to_string()
}

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

    pub async fn attach(
        &mut self,
        id: OrderId,
        subscription_id: SubscriptionId,
        kind: OrderKind,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE orders SET subscription_id = $2, kind = $3 WHERE id = $1 AND subscription_id IS NULL",
        )
        .bind(id.0)
        .bind(subscription_id.0)
        .bind(kind.as_str())
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn mark_review(&mut self, id: OrderId, reason: &str) -> StoreResult<()> {
        let (order, _) = self.transition(id, OrderStatus::Review).await?;
        sqlx::query(
            "UPDATE orders SET review_reason = $2, plan_stale_at = CASE \
             WHEN plan IS NOT NULL AND plan_applied_at IS NULL THEN COALESCE(plan_stale_at, now()) \
             ELSE plan_stale_at END WHERE id = $1",
        )
        .bind(order.id.0)
        .bind(reason)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn plan_state(&mut self, id: OrderId) -> StoreResult<PlanState> {
        let (saved, applied_at, stale_at): (bool, Option<DateTime<Utc>>, Option<DateTime<Utc>>) =
            sqlx::query_as(
                "SELECT plan IS NOT NULL, plan_applied_at, plan_stale_at FROM orders WHERE id = $1",
            )
            .bind(id.0)
            .fetch_optional(&mut *self.conn)
            .await?
            .ok_or(StoreError::NotFound("заказ"))?;
        Ok(PlanState {
            saved,
            applied_at,
            stale_at,
        })
    }

    pub async fn review_reason(&mut self, id: OrderId) -> StoreResult<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT review_reason FROM orders WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?
                .flatten(),
        )
    }

    pub async fn mark_plan_applied(&mut self, id: OrderId, at: DateTime<Utc>) -> StoreResult<()> {
        sqlx::query(
            "UPDATE orders SET plan_applied_at = COALESCE(plan_applied_at, $2) WHERE id = $1",
        )
        .bind(id.0)
        .bind(at)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn reset_plan(&mut self, id: OrderId) -> StoreResult<PlanReset> {
        type Row = (
            Option<Json<FulfillmentPlan>>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        );
        let (plan, applied_at, stale_at): Row = sqlx::query_as(
            "WITH old AS (SELECT id, plan, plan_applied_at, plan_stale_at FROM orders WHERE id = $1 FOR UPDATE) \
             UPDATE orders o SET plan = NULL, plan_applied_at = NULL, plan_stale_at = NULL FROM old \
             WHERE o.id = old.id RETURNING old.plan, old.plan_applied_at, old.plan_stale_at",
        )
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("заказ"))?;
        Ok(PlanReset {
            state: PlanState {
                saved: plan.is_some(),
                applied_at,
                stale_at,
            },
            plan: plan.map(|p| p.0),
        })
    }

    pub async fn abandon_plan(&mut self, id: OrderId, reason: &str) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE orders SET plan_stale_at = now(), review_reason = $2 WHERE id = $1 AND status = 'review' \
             AND plan IS NOT NULL AND plan_applied_at IS NULL AND plan_stale_at IS NULL",
        )
        .bind(id.0)
        .bind(reason)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn refund_pending(&mut self, id: OrderId) -> StoreResult<bool> {
        Ok(sqlx::query_scalar(&format!(
            "SELECT {REFUND_PENDING} FROM orders o WHERE o.id = $1"
        ))
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?
        .unwrap_or(false))
    }

    pub async fn refund_pending_settled(&mut self, id: OrderId) -> StoreResult<bool> {
        sqlx::query("SELECT id FROM payments WHERE order_id = $1 ORDER BY id FOR SHARE")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        self.refund_pending(id).await
    }

    pub async fn delivery_blocker(
        &mut self,
        subscription_id: SubscriptionId,
        id: OrderId,
    ) -> StoreResult<Option<Uuid>> {
        Ok(sqlx::query_scalar(&format!(
            "SELECT o.public_id FROM orders o WHERE o.subscription_id = $1 AND o.id <> $2 \
             AND o.kind NOT IN ('topup', 'module') \
             AND ((o.status = 'paid' AND NOT {REFUND_PENDING} AND (o.plan IS NOT NULL OR o.id < $2)) \
                 OR (o.status = 'review' AND {LIVE_REVIEW})) \
             ORDER BY o.id LIMIT 1"
        ))
        .bind(subscription_id.0)
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?)
    }

    pub async fn holds_renewal(&mut self, id: OrderId, renewal_method: &str) -> StoreResult<bool> {
        Ok(sqlx::query_scalar(&format!(
            "SELECT (o.status = 'paid' AND NOT {REFUND_PENDING}) OR (o.status = 'review' AND ({LIVE_REVIEW} \
                 OR EXISTS (SELECT 1 FROM payments p WHERE p.order_id = o.id AND p.method = $2))) \
             FROM orders o WHERE o.id = $1"
        ))
        .bind(id.0)
        .bind(renewal_method)
        .fetch_optional(&mut *self.conn)
        .await?
        .unwrap_or(false))
    }

    pub async fn in_review(&mut self, limit: i64) -> StoreResult<Vec<ReviewOrder>> {
        let rows: Vec<ReviewRow> = sqlx::query_as(&format!(
            "SELECT o.id, o.public_id, o.client_id, o.subscription_id, o.kind, o.snapshot ->> 'name' AS tariff, \
             o.currency, o.total_minor, o.stars_total, o.review_reason, o.plan, o.plan_applied_at, o.plan_stale_at, \
             {REFUND_PENDING} AS refund_pending, j.id AS job_id, j.status AS job_status, j.attempts AS job_attempts, \
             j.last_error AS job_error, j.updated_at AS job_updated_at, o.paid_at, o.updated_at \
             FROM orders o LEFT JOIN LATERAL (SELECT id, status, attempts, last_error, updated_at FROM jobs \
                 WHERE dedupe_key = $1 || o.id::text ORDER BY id DESC LIMIT 1) j ON true \
             WHERE o.status = 'review' ORDER BY o.updated_at DESC, o.id DESC LIMIT $2"
        ))
        .bind(fulfill_key_prefix())
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_store;
    use crate::{Db, NewClient, NewSubscription};
    use sn_domain::client::{Locale, RegistrationSource};
    use sn_domain::ids::{PeriodId, TariffId};
    use sn_domain::money::{Amount, PayCurrency};
    use sn_domain::payment::{NewPayment, ProviderId};
    use sn_domain::subscription::{AutoRenewState, SubscriptionKind};

    fn sample_plan() -> FulfillmentPlan {
        serde_json::from_value(serde_json::json!({
            "kind": "renewal",
            "action": {"type": "update", "user_id": 77},
            "target": {
                "expire_at": "2027-03-16T00:00:00Z",
                "squads": ["00000000-0000-0000-0000-000000000001"],
                "external_squad": null,
                "traffic_limit_bytes": 0,
                "traffic_strategy": "NO_RESET",
                "hwid_device_limit": 3,
                "tag": null
            },
            "reset_traffic": false,
            "activate": false,
            "period_started_at": "2026-11-16T14:52:00Z",
            "period_days": 120,
            "base_expire_at": "2027-02-14T00:00:00Z"
        }))
        .unwrap()
    }

    async fn subscription(db: &mut impl Db) -> (ClientId, SubscriptionId) {
        let client = db
            .clients()
            .create(&NewClient {
                locale: Locale::Ru,
                registration_source: RegistrationSource::Bot,
                note: None,
            })
            .await
            .unwrap()
            .id;
        let subscription = db
            .subscriptions()
            .create(&NewSubscription {
                client_id: client,
                tariff_id: None,
                kind: SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: AutoRenewState::default(),
            })
            .await
            .unwrap()
            .id;
        (client, subscription)
    }

    async fn paid(
        db: &mut impl Db,
        client_id: ClientId,
        subscription_id: SubscriptionId,
        kind: OrderKind,
    ) -> OrderId {
        let amount = Money::new(299, Currency::Usd);
        let intent = match kind {
            OrderKind::TopUp => PurchaseIntent::TopUp { amount },
            _ => PurchaseIntent::Subscription {
                tariff_id: TariffId(1),
                period_id: PeriodId(1),
                extra_devices: 0,
                target: Some(subscription_id),
            },
        };
        let order = db
            .orders()
            .create(&NewOrder {
                client_id,
                subscription_id: Some(subscription_id),
                kind,
                intent,
                quote: Quote {
                    currency: Currency::Usd,
                    lines: Vec::new(),
                    subtotal: amount,
                    discount: Money::zero(Currency::Usd),
                    total: amount,
                    stars: None,
                    days: 30,
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

    async fn stage(db: &mut impl Db, id: OrderId) -> PlanStage {
        db.orders().plan_state(id).await.unwrap().stage()
    }

    #[tokio::test]
    async fn неприменённый_план_заказа_на_проверке_устаревает_и_сбрасывается() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let (client, sub) = subscription(&mut conn).await;
        let order = paid(&mut conn, client, sub, OrderKind::Renewal).await;
        assert_eq!(stage(&mut conn, order).await, PlanStage::None);
        conn.orders()
            .save_plan(order, &sample_plan())
            .await
            .unwrap();
        assert_eq!(stage(&mut conn, order).await, PlanStage::Saved);
        assert!(
            !conn
                .orders()
                .abandon_plan(order, "заказ ещё оплачен")
                .await
                .unwrap()
        );
        conn.orders()
            .mark_review(order, "срок изменился")
            .await
            .unwrap();
        assert_eq!(stage(&mut conn, order).await, PlanStage::Stale);
        assert_eq!(
            conn.orders().review_reason(order).await.unwrap().as_deref(),
            Some("срок изменился")
        );
        let reset = conn.orders().reset_plan(order).await.unwrap();
        assert_eq!(reset.plan, Some(sample_plan()));
        assert_eq!(reset.state.stage(), PlanStage::Stale);
        assert_eq!(stage(&mut conn, order).await, PlanStage::None);
        assert_eq!(conn.orders().get(order).await.unwrap().plan, None);
        let empty = conn.orders().reset_plan(order).await.unwrap();
        assert_eq!(empty.plan, None);
        assert_eq!(empty.state.stage(), PlanStage::None);
        conn.orders()
            .save_plan(order, &sample_plan())
            .await
            .unwrap();
        assert_eq!(stage(&mut conn, order).await, PlanStage::Saved);
        assert!(
            conn.orders()
                .abandon_plan(order, "попытки кончились")
                .await
                .unwrap()
        );
        assert_eq!(stage(&mut conn, order).await, PlanStage::Stale);
        assert_eq!(
            conn.orders().review_reason(order).await.unwrap().as_deref(),
            Some("попытки кончились")
        );
        assert!(!conn.orders().abandon_plan(order, "ещё раз").await.unwrap());
        conn.orders().reset_plan(order).await.unwrap();
        conn.orders()
            .save_plan(order, &sample_plan())
            .await
            .unwrap();
        conn.orders()
            .mark_plan_applied(order, Utc::now())
            .await
            .unwrap();
        conn.orders()
            .mark_review(order, "пользователь удалён после выдачи")
            .await
            .unwrap();
        assert_eq!(stage(&mut conn, order).await, PlanStage::Applied);
        assert!(!conn.orders().abandon_plan(order, "применён").await.unwrap());
        let applied = conn.orders().reset_plan(order).await.unwrap();
        assert!(applied.state.applied());
        assert!(matches!(
            conn.orders().plan_state(OrderId(999_999)).await,
            Err(StoreError::NotFound(_))
        ));
        drop(conn);
        db.cleanup().await;
    }

    async fn blocker(db: &mut impl Db, sub: SubscriptionId, id: OrderId) -> Option<Uuid> {
        db.orders().delivery_blocker(sub, id).await.unwrap()
    }

    #[tokio::test]
    async fn следующие_заказы_подписки_ждут_только_заказы_в_работе() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let (client, sub) = subscription(&mut conn).await;
        let (_, other_sub) = subscription(&mut conn).await;
        let first = paid(&mut conn, client, sub, OrderKind::Renewal).await;
        let second = paid(&mut conn, client, sub, OrderKind::Renewal).await;
        paid(&mut conn, client, sub, OrderKind::TopUp).await;
        paid(&mut conn, client, other_sub, OrderKind::Renewal).await;
        let public = |order: Order| order.public_id;
        let first_public = public(conn.orders().get(first).await.unwrap());
        let second_public = public(conn.orders().get(second).await.unwrap());
        assert_eq!(blocker(&mut conn, sub, first).await, None);
        assert_eq!(blocker(&mut conn, sub, second).await, Some(first_public));
        conn.orders()
            .save_plan(second, &sample_plan())
            .await
            .unwrap();
        assert_eq!(blocker(&mut conn, sub, first).await, Some(second_public));
        conn.orders()
            .mark_review(second, "срок изменился")
            .await
            .unwrap();
        assert_eq!(blocker(&mut conn, sub, first).await, None);
        assert!(
            !conn
                .orders()
                .holds_renewal(second, "autorenew")
                .await
                .unwrap()
        );
        assert!(
            conn.orders()
                .holds_renewal(first, "autorenew")
                .await
                .unwrap()
        );
        conn.orders().reset_plan(second).await.unwrap();
        assert_eq!(blocker(&mut conn, sub, first).await, None);
        conn.orders()
            .save_plan(second, &sample_plan())
            .await
            .unwrap();
        assert_eq!(blocker(&mut conn, sub, first).await, Some(second_public));
        assert!(
            conn.orders()
                .holds_renewal(second, "autorenew")
                .await
                .unwrap()
        );
        conn.orders()
            .abandon_plan(second, "попытки кончились")
            .await
            .unwrap();
        assert_eq!(blocker(&mut conn, sub, first).await, None);
        conn.orders().reset_plan(second).await.unwrap();
        conn.orders()
            .save_plan(second, &sample_plan())
            .await
            .unwrap();
        conn.orders()
            .mark_plan_applied(second, Utc::now())
            .await
            .unwrap();
        conn.orders()
            .mark_review(second, "сбой после применения")
            .await
            .unwrap();
        assert_eq!(blocker(&mut conn, sub, first).await, Some(second_public));
        assert!(
            conn.orders()
                .holds_renewal(second, "autorenew")
                .await
                .unwrap()
        );
        conn.orders()
            .transition(second, OrderStatus::Fulfilled)
            .await
            .unwrap();
        assert_eq!(blocker(&mut conn, sub, first).await, None);
        assert!(
            !conn
                .orders()
                .holds_renewal(second, "autorenew")
                .await
                .unwrap()
        );
        assert!(
            !conn
                .orders()
                .holds_renewal(OrderId(999_999), "autorenew")
                .await
                .unwrap()
        );
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn заказ_на_проверке_оплаченный_автопродлением_держит_новое_списание() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let (client, sub) = subscription(&mut conn).await;
        let manual = paid(&mut conn, client, sub, OrderKind::Renewal).await;
        let auto = paid(&mut conn, client, sub, OrderKind::Renewal).await;
        for (order, method) in [(manual, None), (auto, Some("autorenew"))] {
            conn.payments()
                .create(&NewPayment {
                    order_id: order,
                    client_id: client,
                    provider: ProviderId::from("balance"),
                    method: method.map(str::to_string),
                    amount: Amount::new(299, PayCurrency::Usd),
                    expires_at: None,
                })
                .await
                .unwrap();
            conn.orders()
                .mark_review(order, "сквада нет в панели")
                .await
                .unwrap();
        }
        assert!(
            !conn
                .orders()
                .holds_renewal(manual, "autorenew")
                .await
                .unwrap()
        );
        assert!(
            conn.orders()
                .holds_renewal(auto, "autorenew")
                .await
                .unwrap()
        );
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn ожидающий_возврат_виден_у_заказа_и_в_списке_проверки() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let (client, sub) = subscription(&mut conn).await;
        let order = paid(&mut conn, client, sub, OrderKind::Renewal).await;
        let payment = conn
            .payments()
            .create(&NewPayment {
                order_id: order,
                client_id: client,
                provider: ProviderId::from("platega"),
                method: None,
                amount: Amount::new(299, PayCurrency::Usd),
                expires_at: None,
            })
            .await
            .unwrap();
        assert!(!conn.orders().refund_pending(order).await.unwrap());
        assert!(
            !conn
                .orders()
                .refund_pending(OrderId(999_999))
                .await
                .unwrap()
        );
        let request: i64 = sqlx::query_scalar(
            "INSERT INTO provider_refunds (payment_id, amount_minor, currency, subscription_action) \
             VALUES ($1, 299, 'USD', 'keep') RETURNING id",
        )
        .bind(payment.id.0)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        assert!(conn.orders().refund_pending(order).await.unwrap());
        assert!(conn.orders().in_review(10).await.unwrap().is_empty());
        conn.orders()
            .save_plan(order, &sample_plan())
            .await
            .unwrap();
        conn.orders()
            .mark_review(order, "срок изменился")
            .await
            .unwrap();
        let listed = conn.orders().in_review(10).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].order_id, order);
        assert_eq!(listed[0].reason.as_deref(), Some("срок изменился"));
        assert_eq!(listed[0].plan, PlanStage::Stale);
        assert_eq!(
            listed[0].plan_target_expire_at,
            Some(sample_plan().target.expire_at)
        );
        assert_eq!(listed[0].plan_base_expire_at, sample_plan().base_expire_at);
        assert!(listed[0].refund_pending);
        assert_eq!(listed[0].job, None);
        assert_eq!(listed[0].total, Money::new(299, Currency::Usd));
        let job = conn
            .jobs()
            .enqueue(&JobPayload::FulfillOrder { order_id: order }, None)
            .await
            .unwrap()
            .unwrap();
        conn.jobs().review(job, "попытки кончились").await.unwrap();
        let listed = conn.orders().in_review(10).await.unwrap();
        let seen = listed[0].job.clone().unwrap();
        assert_eq!(seen.id, job);
        assert_eq!(seen.status, JobStatus::Review);
        assert_eq!(seen.error.as_deref(), Some("попытки кончились"));
        assert!(!seen.active());
        sqlx::query(
            "UPDATE provider_refunds SET status = 'declined', resolved_at = now() WHERE id = $1",
        )
        .bind(request)
        .execute(conn.conn())
        .await
        .unwrap();
        assert!(!conn.orders().refund_pending(order).await.unwrap());
        assert!(!conn.orders().in_review(10).await.unwrap()[0].refund_pending);
        let repeat = conn
            .payments()
            .create(&NewPayment {
                order_id: order,
                client_id: client,
                provider: ProviderId::from("platega"),
                method: Some("repeat".into()),
                amount: Amount::new(299, PayCurrency::Usd),
                expires_at: None,
            })
            .await
            .unwrap();
        conn.payments()
            .mark_repeat(repeat.id, Some(payment.id), crate::RepeatAction::Balance)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO provider_refunds (payment_id, amount_minor, currency, subscription_action) \
             VALUES ($1, 299, 'USD', 'keep')",
        )
        .bind(repeat.id.0)
        .execute(conn.conn())
        .await
        .unwrap();
        assert!(!conn.orders().refund_pending(order).await.unwrap());
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn оплаченный_заказ_с_ожидающим_возвратом_не_держит_следующие() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let (client, sub) = subscription(&mut conn).await;
        let refunded = paid(&mut conn, client, sub, OrderKind::Renewal).await;
        let next = paid(&mut conn, client, sub, OrderKind::Renewal).await;
        let refunded_public = conn.orders().get(refunded).await.unwrap().public_id;
        let payment = conn
            .payments()
            .create(&NewPayment {
                order_id: refunded,
                client_id: client,
                provider: ProviderId::from("platega"),
                method: None,
                amount: Amount::new(299, PayCurrency::Usd),
                expires_at: None,
            })
            .await
            .unwrap();
        assert_eq!(blocker(&mut conn, sub, next).await, Some(refunded_public));
        let request: i64 = sqlx::query_scalar(
            "INSERT INTO provider_refunds (payment_id, amount_minor, currency, subscription_action) \
             VALUES ($1, 299, 'USD', 'keep') RETURNING id",
        )
        .bind(payment.id.0)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        assert_eq!(blocker(&mut conn, sub, next).await, None);
        conn.orders()
            .save_plan(refunded, &sample_plan())
            .await
            .unwrap();
        assert_eq!(blocker(&mut conn, sub, next).await, None);
        sqlx::query(
            "UPDATE provider_refunds SET status = 'declined', resolved_at = now() WHERE id = $1",
        )
        .bind(request)
        .execute(conn.conn())
        .await
        .unwrap();
        assert_eq!(blocker(&mut conn, sub, next).await, Some(refunded_public));
        conn.orders().reset_plan(refunded).await.unwrap();
        assert_eq!(blocker(&mut conn, sub, next).await, Some(refunded_public));
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn проверка_перед_выдачей_дожидается_возврата_который_оформляется_прямо_сейчас() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let (client, sub) = subscription(&mut conn).await;
        let order = paid(&mut conn, client, sub, OrderKind::Renewal).await;
        let payment = conn
            .payments()
            .create(&NewPayment {
                order_id: order,
                client_id: client,
                provider: ProviderId::from("platega"),
                method: None,
                amount: Amount::new(299, PayCurrency::Usd),
                expires_at: None,
            })
            .await
            .unwrap();
        let mut refund = db.store.begin().await.unwrap();
        refund.payments().lock(payment.id).await.unwrap();
        sqlx::query(
            "INSERT INTO provider_refunds (payment_id, amount_minor, currency, subscription_action) \
             VALUES ($1, 299, 'USD', 'keep')",
        )
        .bind(payment.id.0)
        .execute(refund.conn())
        .await
        .unwrap();
        assert!(!conn.orders().refund_pending(order).await.unwrap());
        let requested = async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            refund.commit().await.unwrap();
        };
        let (pending, ()) = tokio::join!(
            async { conn.orders().refund_pending_settled(order).await.unwrap() },
            requested
        );
        assert!(pending);
        drop(conn);
        db.cleanup().await;
    }
}
