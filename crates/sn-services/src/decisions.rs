use crate::clients::{OrderBrief, order_brief};
use crate::error::{ServiceError, ServiceResult};
use crate::fulfillment::ReviewItem;
use crate::inbox::{PENDING_REFUNDS_MAX, PendingRefundRequest, RefundActionReview};
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::ids::{AdminId, ClientId, OrderId, PaymentId, TariffId};
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::payment::{NoticeKind, Payment};
use sn_store::{ActorKind, AuditEntry, ClientPerson, Db, charge_review_filter, poll_key_prefix};
use std::collections::HashMap;
use uuid::Uuid;

pub const STUCK_REFUND_HOURS: i64 = 24;
pub const CHARGES_LIMIT: i64 = 50;
pub const CHARGES_MAX: i64 = 200;
pub const CREDITS_LIMIT: i64 = 100;
const CREDIT_NOTE_LIMIT: usize = 300;

const ORDERS: &str = "FROM orders o WHERE o.status = 'review'";
const STARS: &str = "FROM bot_star_events e WHERE e.status = 'review'";
const ACTIONS: &str = "FROM provider_refunds r WHERE r.action_status = 'failed'";
const REQUESTS: &str = "FROM provider_refunds r WHERE r.status = 'pending' AND r.created_at <= $1";
const CREDITS: &str = "FROM payment_notices n WHERE n.resolved_at IS NULL \
     AND n.kind IN ('manual_credit', 'refund_reported')";
const SAME_TOTAL: &str = "CASE WHEN count(DISTINCT currency) = 1 THEN sum(amount_minor)::bigint END AS total, \
     min(currency) AS currency";

fn charges_from() -> String {
    format!("FROM payments p WHERE {}", charge_review_filter("p"))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scope {
    pub money: bool,
    pub refunds: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Brief {
    pub client_id: Option<ClientId>,
    pub client: Option<ClientPerson>,
    pub amount: Option<Amount>,
    pub reason: Option<String>,
    pub action: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Group {
    pub count: i64,
    pub since: Option<DateTime<Utc>>,
    pub total: Option<Amount>,
    pub oldest: Option<Brief>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Decisions {
    pub orders: Option<Group>,
    pub refund_actions: Option<Group>,
    pub refund_requests: Option<Group>,
    pub charges: Option<Group>,
    pub stars: Option<Group>,
    pub credits: Option<Group>,
}

impl Decisions {
    pub fn total(&self) -> i64 {
        self.groups().map(|group| group.count).sum()
    }

    fn groups(&self) -> impl Iterator<Item = &Group> {
        [
            &self.orders,
            &self.refund_actions,
            &self.refund_requests,
            &self.charges,
            &self.stars,
            &self.credits,
        ]
        .into_iter()
        .flatten()
    }

    fn groups_mut(&mut self) -> impl Iterator<Item = &mut Group> {
        [
            &mut self.orders,
            &mut self.refund_actions,
            &mut self.refund_requests,
            &mut self.charges,
            &mut self.stars,
            &mut self.credits,
        ]
        .into_iter()
        .flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderReview {
    #[serde(flatten)]
    pub item: ReviewItem,
    pub client: Option<ClientPerson>,
    pub order: Option<OrderBrief>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefundActionView {
    #[serde(flatten)]
    pub item: RefundActionReview,
    pub client: Option<ClientPerson>,
    pub order: Option<OrderBrief>,
    pub provider: Option<String>,
    pub provider_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefundRequestView {
    #[serde(flatten)]
    pub item: PendingRefundRequest,
    pub client_id: Option<ClientId>,
    pub client: Option<ClientPerson>,
    pub order: Option<OrderBrief>,
    pub provider: Option<String>,
    pub provider_title: Option<String>,
    pub stuck: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChargeReviewItem {
    pub payment_id: PaymentId,
    pub public_id: Uuid,
    pub order_id: OrderId,
    pub client_id: ClientId,
    pub client: Option<ClientPerson>,
    pub order: Option<OrderBrief>,
    pub provider: String,
    pub provider_title: String,
    pub amount: Amount,
    pub reason: Option<String>,
    pub charged_at: DateTime<Utc>,
    pub review_since: Option<DateTime<Utc>>,
    pub closes_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreditView {
    pub id: i64,
    pub provider: String,
    pub provider_title: String,
    pub kind: NoticeKind,
    pub title: &'static str,
    pub amount: Option<Amount>,
    pub client_id: Option<ClientId>,
    pub client: Option<ClientPerson>,
    pub payment_id: Option<PaymentId>,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct CountRow {
    count: i64,
    since: Option<DateTime<Utc>>,
    total: Option<i64>,
    currency: Option<String>,
}

#[derive(sqlx::FromRow)]
struct OldestRow {
    client_id: Option<i64>,
    minor: Option<i64>,
    currency: Option<String>,
    reason: Option<String>,
    action: Option<String>,
}

fn amount(minor: Option<i64>, currency: Option<&str>) -> Option<Amount> {
    let currency = PayCurrency::parse(currency?).ok()?;
    Some(Amount::new(minor?, currency))
}

#[derive(Default)]
struct Found {
    people: HashMap<ClientId, ClientPerson>,
    orders: HashMap<OrderId, OrderBrief>,
    payments: HashMap<PaymentId, Payment>,
}

pub struct DecisionsOps<'a> {
    services: &'a Services,
}

impl<'a> DecisionsOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        DecisionsOps { services }
    }

    pub fn stuck_before(&self) -> DateTime<Utc> {
        self.services.now() - Duration::hours(STUCK_REFUND_HOURS)
    }

    pub async fn waiting(&self, scope: Scope) -> ServiceResult<i64> {
        if !scope.money && !scope.refunds {
            return Ok(0);
        }
        let mut conn = self.services.store().conn().await?;
        let (orders, charges, stars, actions, requests, credits): (i64, i64, i64, i64, i64, i64) =
            sqlx::query_as(&format!(
                "SELECT (SELECT count(*) {ORDERS}), (SELECT count(*) {}), (SELECT count(*) {STARS}), \
                 (SELECT count(*) {ACTIONS}), (SELECT count(*) {REQUESTS}), (SELECT count(*) {CREDITS})",
                charges_from()
            ))
            .bind(self.stuck_before())
            .fetch_one(conn.conn())
            .await?;
        let money = if scope.money {
            orders + charges + stars + credits
        } else {
            0
        };
        let refunds = if scope.refunds { actions + requests } else { 0 };
        Ok(money + refunds)
    }

    pub async fn summary(&self, scope: Scope) -> ServiceResult<Decisions> {
        let cutoff = self.stuck_before();
        let mut out = Decisions::default();
        if scope.money {
            out.orders = Some(
                self.group(
                    &format!("SELECT count(*) AS count, min(o.updated_at) AS since, NULL::bigint AS total, NULL::text AS currency {ORDERS}"),
                    &format!(
                        "SELECT o.client_id, CASE WHEN o.stars_total > 0 THEN o.stars_total ELSE o.total_minor END AS minor, \
                         CASE WHEN o.stars_total > 0 THEN 'XTR' ELSE o.currency END AS currency, \
                         o.review_reason AS reason, NULL::text AS action {ORDERS} ORDER BY o.updated_at, o.id LIMIT 1"
                    ),
                    None,
                )
                .await?,
            );
            out.charges = Some(
                self.group(
                    &format!(
                        "SELECT count(*) AS count, min(p.created_at) AS since, {SAME_TOTAL} {}",
                        charges_from()
                    ),
                    &format!(
                        "SELECT p.client_id, p.amount_minor AS minor, p.currency, \
                         (SELECT j.last_error FROM jobs j WHERE j.dedupe_key = '{}' || p.id::text \
                             ORDER BY j.id DESC LIMIT 1) AS reason, NULL::text AS action {} \
                         ORDER BY p.created_at, p.id LIMIT 1",
                        poll_key_prefix(),
                        charges_from()
                    ),
                    None,
                )
                .await?,
            );
            out.stars = Some(
                self.group(
                    &format!(
                        "SELECT count(*) AS count, min(e.created_at) AS since, \
                         sum(CASE WHEN jsonb_typeof(e.payload -> 'total_amount') = 'number' \
                             THEN (e.payload ->> 'total_amount')::bigint END)::bigint AS total, \
                         'XTR' AS currency {STARS}"
                    ),
                    &format!(
                        "SELECT COALESCE(( \
                             SELECT c.id FROM client_identities i JOIN clients c ON c.id = i.client_id \
                             WHERE i.kind = 'telegram' AND i.value = e.payload ->> 'telegram_user_id' \
                               AND c.deleted_at IS NULL LIMIT 1), \
                             (SELECT p.client_id FROM payments p WHERE p.id = e.payment_id)) AS client_id, \
                         CASE WHEN jsonb_typeof(e.payload -> 'total_amount') = 'number' \
                             THEN (e.payload ->> 'total_amount')::bigint END AS minor, \
                         'XTR' AS currency, e.last_error AS reason, NULL::text AS action {STARS} \
                         ORDER BY e.created_at, e.id LIMIT 1"
                    ),
                    None,
                )
                .await?,
            );
            out.credits = Some(
                self.group(
                    &format!(
                        "SELECT count(*) AS count, min(n.created_at) AS since, {SAME_TOTAL} {CREDITS}"
                    ),
                    &format!(
                        "SELECT n.client_id, n.amount_minor AS minor, n.currency, n.text AS reason, \
                         n.kind AS action {CREDITS} ORDER BY n.created_at, n.id LIMIT 1"
                    ),
                    None,
                )
                .await?,
            );
        }
        if scope.refunds {
            out.refund_actions = Some(
                self.group(
                    &format!(
                        "SELECT count(*) AS count, min(COALESCE(r.resolved_at, r.created_at)) AS since, \
                         {SAME_TOTAL} {ACTIONS}"
                    ),
                    &format!(
                        "SELECT (SELECT p.client_id FROM payments p WHERE p.id = r.payment_id) AS client_id, \
                         r.amount_minor AS minor, r.currency, r.action_error AS reason, \
                         r.subscription_action AS action {ACTIONS} ORDER BY r.id LIMIT 1"
                    ),
                    None,
                )
                .await?,
            );
            out.refund_requests = Some(
                self.group(
                    &format!(
                        "SELECT count(*) AS count, min(r.created_at) AS since, {SAME_TOTAL} {REQUESTS}"
                    ),
                    &format!(
                        "SELECT (SELECT p.client_id FROM payments p WHERE p.id = r.payment_id) AS client_id, \
                         r.amount_minor AS minor, r.currency, r.reason, r.subscription_action AS action \
                         {REQUESTS} ORDER BY r.created_at, r.id LIMIT 1"
                    ),
                    Some(cutoff),
                )
                .await?,
            );
        }
        let ids: Vec<ClientId> = out
            .groups()
            .filter_map(|group| group.oldest.as_ref().and_then(|brief| brief.client_id))
            .collect();
        let people = {
            let mut conn = self.services.store().conn().await?;
            conn.clients().people(&ids).await?
        };
        for group in out.groups_mut() {
            if let Some(brief) = group.oldest.as_mut() {
                brief.client = brief.client_id.and_then(|id| people.get(&id).cloned());
            }
        }
        Ok(out)
    }

    async fn group(
        &self,
        count_sql: &str,
        oldest_sql: &str,
        cutoff: Option<DateTime<Utc>>,
    ) -> ServiceResult<Group> {
        let mut conn = self.services.store().conn().await?;
        let mut counted = sqlx::query_as::<_, CountRow>(count_sql);
        if let Some(cutoff) = cutoff {
            counted = counted.bind(cutoff);
        }
        let counted = counted.fetch_one(conn.conn()).await?;
        if counted.count == 0 {
            return Ok(Group::default());
        }
        let mut oldest = sqlx::query_as::<_, OldestRow>(oldest_sql);
        if let Some(cutoff) = cutoff {
            oldest = oldest.bind(cutoff);
        }
        let oldest = oldest.fetch_optional(conn.conn()).await?;
        Ok(Group {
            count: counted.count,
            since: counted.since,
            total: amount(counted.total, counted.currency.as_deref()),
            oldest: oldest.map(|row| Brief {
                client_id: row.client_id.map(ClientId),
                client: None,
                amount: amount(row.minor, row.currency.as_deref()),
                reason: row.reason.filter(|reason| !reason.trim().is_empty()),
                action: row.action,
            }),
        })
    }

    async fn found(
        &self,
        clients: &[ClientId],
        orders: &[OrderId],
        payments: &[PaymentId],
    ) -> ServiceResult<Found> {
        let mut conn = self.services.store().conn().await?;
        let payments: HashMap<PaymentId, Payment> = if payments.is_empty() {
            HashMap::new()
        } else {
            conn.payments()
                .by_ids(payments)
                .await?
                .into_iter()
                .map(|payment| (payment.id, payment))
                .collect()
        };
        let mut clients = clients.to_vec();
        clients.extend(payments.values().map(|payment| payment.client_id));
        let mut orders = orders.to_vec();
        orders.extend(payments.values().map(|payment| payment.order_id));
        let people = conn.clients().people(&clients).await?;
        let mut briefs = HashMap::new();
        if !orders.is_empty() {
            let tariffs: HashMap<TariffId, String> = conn
                .tariffs()
                .list(true)
                .await?
                .into_iter()
                .map(|tariff| (tariff.id, tariff.name))
                .collect();
            for order in conn.orders().by_ids(&orders).await? {
                briefs.insert(order.id, order_brief(&order, &tariffs));
            }
        }
        Ok(Found {
            people,
            orders: briefs,
            payments,
        })
    }

    fn provider_title(&self, provider: &str) -> String {
        self.services
            .payments()
            .get(provider)
            .map_or_else(|| provider.to_string(), |p| p.title().to_string())
    }

    pub async fn orders(&self, limit: i64) -> ServiceResult<Vec<OrderReview>> {
        let items = self.services.fulfillment().review_orders(limit).await?;
        let clients: Vec<ClientId> = items.iter().map(|item| item.order.client_id).collect();
        let orders: Vec<OrderId> = items.iter().map(|item| item.order.order_id).collect();
        let found = self.found(&clients, &orders, &[]).await?;
        Ok(items
            .into_iter()
            .map(|item| OrderReview {
                client: found.people.get(&item.order.client_id).cloned(),
                order: found.orders.get(&item.order.order_id).cloned(),
                item,
            })
            .collect())
    }

    pub async fn refund_actions(&self, limit: i64) -> ServiceResult<Vec<RefundActionView>> {
        let items = self.services.inbox().refund_actions_review(limit).await?;
        let payments: Vec<PaymentId> = items.iter().map(|item| item.payment_id).collect();
        let found = self.found(&[], &[], &payments).await?;
        Ok(items
            .into_iter()
            .map(|item| {
                let provider = found
                    .payments
                    .get(&item.payment_id)
                    .map(|payment| payment.provider.as_str().to_string());
                RefundActionView {
                    client: found.people.get(&item.client_id).cloned(),
                    order: found.orders.get(&item.order_id).cloned(),
                    provider_title: provider.as_deref().map(|p| self.provider_title(p)),
                    provider,
                    item,
                }
            })
            .collect())
    }

    pub async fn refund_requests(
        &self,
        limit: i64,
        stuck_only: bool,
    ) -> ServiceResult<Vec<RefundRequestView>> {
        let cutoff = self.stuck_before();
        let mut items: Vec<PendingRefundRequest> = {
            let mut conn = self.services.store().conn().await?;
            conn.payments()
                .pending_provider_refunds(
                    if stuck_only {
                        cutoff
                    } else {
                        self.services.now()
                    },
                    limit.clamp(1, PENDING_REFUNDS_MAX),
                )
                .await?
                .into_iter()
                .map(PendingRefundRequest::from)
                .collect()
        };
        items.sort_by_key(|item| (item.requested_at, item.request_id));
        let payments: Vec<PaymentId> = items.iter().map(|item| item.payment_id).collect();
        let found = self.found(&[], &[], &payments).await?;
        Ok(items
            .into_iter()
            .map(|item| {
                let payment = found.payments.get(&item.payment_id);
                let provider = payment.map(|p| p.provider.as_str().to_string());
                RefundRequestView {
                    client_id: payment.map(|p| p.client_id),
                    client: payment.and_then(|p| found.people.get(&p.client_id).cloned()),
                    order: payment.and_then(|p| found.orders.get(&p.order_id).cloned()),
                    provider_title: provider.as_deref().map(|p| self.provider_title(p)),
                    provider,
                    stuck: item.requested_at <= cutoff,
                    item,
                }
            })
            .collect())
    }

    pub async fn credits(&self, limit: i64) -> ServiceResult<Vec<CreditView>> {
        let notices = {
            let mut conn = self.services.store().conn().await?;
            conn.payment_notices()
                .waiting(limit.clamp(1, CREDITS_LIMIT))
                .await?
        };
        let clients: Vec<ClientId> = notices.iter().filter_map(|n| n.client_id).collect();
        let found = self.found(&clients, &[], &[]).await?;
        Ok(notices
            .into_iter()
            .map(|notice| CreditView {
                provider_title: self.provider_title(&notice.provider),
                title: notice.kind.title_ru(),
                client: notice
                    .client_id
                    .and_then(|id| found.people.get(&id).cloned()),
                id: notice.id,
                provider: notice.provider,
                kind: notice.kind,
                amount: notice.amount,
                client_id: notice.client_id,
                payment_id: notice.payment_id,
                text: notice.text,
                created_at: notice.created_at,
            })
            .collect())
    }

    pub async fn close_credit(&self, id: i64, admin: AdminId, note: &str) -> ServiceResult<()> {
        let note: String = note.trim().chars().take(CREDIT_NOTE_LIMIT).collect();
        if note.is_empty() {
            return Err(ServiceError::rejected(
                "notice.close_note",
                "Напишите, что сделано с деньгами",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let notice = tx.payment_notices().get(id).await?;
        if !tx
            .payment_notices()
            .close(id, admin, &note, self.services.now())
            .await?
        {
            return Err(ServiceError::rejected(
                "notice.closed",
                "Это оповещение уже закрыто",
            ));
        }
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::Admin,
                actor_id: Some(admin.0),
                action: "payment.notice_closed".into(),
                entity_kind: Some("payment_notice".into()),
                entity_id: Some(id),
                details: serde_json::json!({
                    "provider": notice.provider,
                    "kind": notice.kind.as_str(),
                    "payment_id": notice.payment_id,
                    "client_id": notice.client_id,
                    "amount": notice.amount,
                    "note": note,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn charges(&self, limit: i64) -> ServiceResult<Vec<ChargeReviewItem>> {
        let charges = {
            let mut conn = self.services.store().conn().await?;
            conn.payments()
                .charges_review_list(limit.clamp(1, CHARGES_MAX))
                .await?
        };
        let clients: Vec<ClientId> = charges.iter().map(|c| c.payment.client_id).collect();
        let orders: Vec<OrderId> = charges.iter().map(|c| c.payment.order_id).collect();
        let found = self.found(&clients, &orders, &[]).await?;
        Ok(charges
            .into_iter()
            .map(|charge| {
                let payment = charge.payment;
                ChargeReviewItem {
                    payment_id: payment.id,
                    public_id: payment.public_id,
                    order_id: payment.order_id,
                    client_id: payment.client_id,
                    client: found.people.get(&payment.client_id).cloned(),
                    order: found.orders.get(&payment.order_id).cloned(),
                    provider_title: self.provider_title(payment.provider.as_str()),
                    provider: payment.provider.as_str().to_string(),
                    amount: payment.amount,
                    reason: charge.reason.filter(|reason| !reason.trim().is_empty()),
                    charged_at: payment.created_at,
                    review_since: charge.review_since,
                    closes_at: payment.expires_at,
                }
            })
            .collect())
    }
}
