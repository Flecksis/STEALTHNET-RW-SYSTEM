use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::inbox::{AcceptResult, RefundCommand, RefundResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sn_domain::events::DomainEvent;
use sn_domain::ids::{AdminId, ClientId, OrderId, PaymentId};
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{
    Payment, PaymentStatus, ProviderId, RefundMethod, RefundSubscriptionAction, providers,
};
use sn_payments::{PaymentRef, ProviderStatus, WebhookOutcome, WebhookReply};
use sn_remnawave::NodeState;
use sn_store::{Db, StoreError};
use uuid::Uuid;

pub const FEED_MAX: u32 = 100;
const MONEY_EVENTS: [&str; 3] = ["payment_paid", "payment_refunded", "balance_changed"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DashboardCounts {
    pub active_paid: i64,
    pub trials: i64,
    pub expiring_soon: i64,
    pub never_connected: i64,
    pub limited: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DashboardRevenue {
    pub today: Money,
    pub week: Money,
    pub month: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeBrief {
    pub uuid: Uuid,
    pub name: String,
    pub country_code: String,
    pub state: NodeState,
    pub users_online: i64,
    pub traffic_alert: bool,
    pub last_status_change: Option<DateTime<Utc>>,
    pub last_status_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemnaOverview {
    pub online_now: i64,
    pub users_total: i64,
    pub status_counts: std::collections::BTreeMap<String, i64>,
    pub nodes_total: usize,
    pub nodes_online: usize,
    pub nodes: Vec<NodeBrief>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DashboardView {
    pub currency: Currency,
    pub subscriptions: Option<DashboardCounts>,
    pub revenue: Option<DashboardRevenue>,
    pub stats_error: Option<String>,
    pub support_waiting: i64,
    pub orders_review: i64,
    pub stars_review: i64,
    pub refund_actions_failed: i64,
    pub remna: Option<RemnaOverview>,
    pub remna_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    #[default]
    All,
    Nodes,
    Clients,
    Payments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct FeedQuery {
    #[serde(default)]
    pub kind: FeedKind,
    #[serde(default)]
    pub remna_before: Option<i64>,
    #[serde(default)]
    pub core_before: Option<i64>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedSource {
    Remnawave,
    Core,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FeedItem {
    pub source: FeedSource,
    pub id: i64,
    pub at: DateTime<Utc>,
    pub kind: String,
    pub title: String,
    pub client_id: Option<ClientId>,
    pub subject: Option<String>,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FeedPage {
    pub items: Vec<FeedItem>,
    pub remna_before: Option<i64>,
    pub core_before: Option<i64>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PaymentFilter {
    #[serde(default)]
    pub status: Option<PaymentStatus>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub client_id: Option<ClientId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MarkPaidRequest {
    #[serde(default)]
    pub provider_txid: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum MarkPaidResult {
    Accepted { order_id: OrderId },
    AlreadyApplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RefundRequest {
    pub amount_minor: Option<i64>,
    pub method: RefundMethod,
    pub subscription_action: RefundSubscriptionAction,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PaymentRow {
    id: i64,
    public_id: Uuid,
    order_id: i64,
    client_id: i64,
    provider: String,
    method: Option<String>,
    amount_minor: i64,
    currency: String,
    status: String,
    provider_txid: Option<String>,
    pay_url: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    paid_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl TryFrom<PaymentRow> for Payment {
    type Error = ServiceError;

    fn try_from(row: PaymentRow) -> Result<Self, Self::Error> {
        Ok(Payment {
            id: PaymentId(row.id),
            public_id: row.public_id,
            order_id: OrderId(row.order_id),
            client_id: ClientId(row.client_id),
            provider: ProviderId(row.provider),
            method: row.method,
            amount: Amount::new(row.amount_minor, PayCurrency::parse(&row.currency)?),
            status: PaymentStatus::parse(&row.status)?,
            provider_txid: row.provider_txid,
            pay_url: row.pay_url,
            expires_at: row.expires_at,
            paid_at: row.paid_at,
            created_at: row.created_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct RemnaFeedRow {
    id: i64,
    scope: String,
    event: String,
    occurred_at: DateTime<Utc>,
    subject_id: Option<String>,
    payload: Value,
}

#[derive(sqlx::FromRow)]
struct CoreFeedRow {
    id: i64,
    occurred_at: DateTime<Utc>,
    client_id: Option<i64>,
    payload: sqlx::types::Json<DomainEvent>,
}

fn text(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    match current {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn pick(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| value.get(key).map(|v| (key.to_string(), v.clone())))
            .collect(),
    )
}

pub fn remna_feed_entry(scope: &str, event: &str, payload: &Value) -> (String, Value) {
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    match scope {
        "node" => {
            let name = text(&data, &["name"]).unwrap_or_else(|| "?".into());
            let title = match event {
                "node.connection_lost" => format!("Нода {name} не отвечает"),
                "node.connection_restored" => format!("Нода {name} снова в сети"),
                "node.traffic_notify" => format!("Нода {name}: трафик подходит к лимиту"),
                "node.created" => format!("Добавлена нода {name}"),
                "node.deleted" => format!("Нода {name} удалена"),
                "node.disabled" => format!("Нода {name} выключена"),
                "node.enabled" => format!("Нода {name} включена"),
                "node.modified" => format!("Нода {name} изменена"),
                other => format!("Нода {name}: {other}"),
            };
            (
                title,
                pick(
                    &data,
                    &[
                        "uuid",
                        "name",
                        "countryCode",
                        "isConnected",
                        "isDisabled",
                        "lastStatusChange",
                        "lastStatusMessage",
                        "usersOnline",
                        "trafficUsedBytes",
                        "trafficLimitBytes",
                    ],
                ),
            )
        }
        "user" | "user_hwid_devices" => {
            let user = data.get("user").cloned().unwrap_or(Value::Null);
            let name = text(&user, &["username"]).unwrap_or_else(|| "?".into());
            let title = match event {
                "user.created" => format!("Создан пользователь {name}"),
                "user.expired" => format!("Подписка {name} истекла"),
                "user.expires_in_72_hours"
                | "user.expires_in_48_hours"
                | "user.expires_in_24_hours"
                | "user.expiration" => {
                    format!("Подписка {name} скоро истекает")
                }
                "user.limited" => format!("У {name} закончился трафик"),
                "user.traffic_reset" => format!("У {name} сброшен трафик"),
                "user.first_connected" => format!("{name} подключился впервые"),
                "user.not_connected" => format!("{name} так и не подключился"),
                "user.disabled" => format!("Пользователь {name} отключён"),
                "user.enabled" => format!("Пользователь {name} включён"),
                "user.revoked" => format!("У {name} перевыпущена ссылка"),
                "user_hwid_devices.added" => format!("{name}: новое устройство"),
                "user_hwid_devices.deleted" => format!("{name}: устройство отключено"),
                other => format!("{name}: {other}"),
            };
            let mut summary = pick(&user, &["id", "username", "status", "expireAt"]);
            if let (Some(device), Value::Object(map)) = (data.get("hwidUserDevice"), &mut summary) {
                map.insert(
                    "device".into(),
                    pick(
                        device,
                        &["platform", "osVersion", "deviceModel", "createdAt"],
                    ),
                );
            }
            (title, summary)
        }
        "crm" => {
            let node = text(&data, &["nodeName"]).unwrap_or_else(|| "?".into());
            let provider = text(&data, &["providerName"]).unwrap_or_default();
            let title = if event.contains("overdue") {
                format!("Оплата сервера {node} ({provider}) просрочена")
            } else {
                format!("Скоро оплата сервера {node} ({provider})")
            };
            (
                title,
                pick(
                    &data,
                    &["nodeName", "providerName", "nextBillingAt", "loginUrl"],
                ),
            )
        }
        _ => (format!("Remnawave: {event}"), Value::Null),
    }
}

pub fn core_feed_title(event: &DomainEvent) -> String {
    match event {
        DomainEvent::ClientRegistered { .. } => "Новый клиент".into(),
        DomainEvent::ClientBlocked { .. } => "Клиент заблокирован".into(),
        DomainEvent::ClientUnblocked { .. } => "Клиент разблокирован".into(),
        DomainEvent::PaymentPaid { amount, .. } => format!("Оплата {amount}"),
        DomainEvent::PaymentRefunded { amount, .. } => format!("Возврат {amount}"),
        DomainEvent::OrderFulfilled { .. } => "Заказ выдан".into(),
        DomainEvent::SubscriptionCreated { .. } => "Новая подписка".into(),
        DomainEvent::SubscriptionRenewed { .. } => "Подписка продлена".into(),
        DomainEvent::TariffChanged { .. } => "Смена тарифа".into(),
        DomainEvent::SubscriptionStatusChanged { to, .. } => {
            format!("Статус подписки: {}", to.as_str())
        }
        DomainEvent::BalanceChanged { delta, .. } => format!("Баланс {delta}"),
        DomainEvent::TicketOpened { .. } => "Новое обращение в поддержку".into(),
        DomainEvent::TicketAnswered { .. } => "Ответ в поддержке".into(),
    }
}

fn remna_scopes(kind: FeedKind) -> Option<Vec<String>> {
    match kind {
        FeedKind::All => None,
        FeedKind::Nodes => Some(vec!["node".into(), "crm".into()]),
        FeedKind::Clients => Some(vec!["user".into(), "user_hwid_devices".into()]),
        FeedKind::Payments => Some(Vec::new()),
    }
}

fn core_kinds(kind: FeedKind, include_money: bool) -> Option<Vec<String>> {
    let all = [
        "client_registered",
        "client_blocked",
        "client_unblocked",
        "payment_paid",
        "payment_refunded",
        "order_fulfilled",
        "subscription_created",
        "subscription_renewed",
        "tariff_changed",
        "subscription_status_changed",
        "balance_changed",
        "ticket_opened",
        "ticket_answered",
    ];
    let allowed = |k: &&&str| include_money || !MONEY_EVENTS.contains(*k);
    match kind {
        FeedKind::All if include_money => None,
        FeedKind::All => Some(all.iter().filter(allowed).map(|k| k.to_string()).collect()),
        FeedKind::Nodes => Some(Vec::new()),
        FeedKind::Clients => Some(
            [
                "client_registered",
                "client_blocked",
                "client_unblocked",
                "subscription_created",
                "subscription_renewed",
                "tariff_changed",
                "subscription_status_changed",
                "ticket_opened",
                "ticket_answered",
            ]
            .iter()
            .map(|k| k.to_string())
            .collect(),
        ),
        FeedKind::Payments => Some(
            [
                "payment_paid",
                "payment_refunded",
                "balance_changed",
                "order_fulfilled",
            ]
            .iter()
            .filter(allowed)
            .map(|k| k.to_string())
            .collect(),
        ),
    }
}

pub fn merge_feed(remna: Vec<FeedItem>, core: Vec<FeedItem>, limit: usize) -> Vec<FeedItem> {
    let mut remna = remna.into_iter().peekable();
    let mut core = core.into_iter().peekable();
    let mut out = Vec::with_capacity(limit);
    while out.len() < limit {
        let take_remna = match (remna.peek(), core.peek()) {
            (Some(r), Some(c)) => r.at >= c.at,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let next = if take_remna {
            remna.next()
        } else {
            core.next()
        };
        match next {
            Some(item) => out.push(item),
            None => break,
        }
    }
    out
}

pub struct AdminOps<'a> {
    services: &'a Services,
}

impl<'a> AdminOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        AdminOps { services }
    }

    pub async fn dashboard(&self, include_money: bool) -> ServiceResult<DashboardView> {
        let currency = self.services.settings().currency;
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        let (subscriptions, revenue, stats_error) =
            match conn.subscriptions().dashboard(now, currency).await {
                Ok(stats) => (
                    Some(DashboardCounts {
                        active_paid: stats.active_paid,
                        trials: stats.trials,
                        expiring_soon: stats.expiring_soon,
                        never_connected: stats.never_connected,
                        limited: stats.limited,
                    }),
                    include_money.then_some(DashboardRevenue {
                        today: Money::new(stats.revenue_today_minor, currency),
                        week: Money::new(stats.revenue_7d_minor, currency),
                        month: Money::new(stats.revenue_30d_minor, currency),
                    }),
                    None,
                ),
                Err(StoreError::NotImplemented(what)) => (None, None, Some(what.to_string())),
                Err(err) => return Err(err.into()),
            };
        let support_waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM tickets WHERE status = 'open' AND last_author = 'client'",
        )
        .fetch_one(conn.conn())
        .await?;
        let orders_review: i64 =
            sqlx::query_scalar("SELECT count(*) FROM orders WHERE status = 'review'")
                .fetch_one(conn.conn())
                .await?;
        let stars_review: i64 =
            sqlx::query_scalar("SELECT count(*) FROM bot_star_events WHERE status = 'review'")
                .fetch_one(conn.conn())
                .await?;
        let refund_actions_failed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM provider_refunds WHERE action_status = 'failed'",
        )
        .fetch_one(conn.conn())
        .await?;
        drop(conn);
        let (remna, remna_error) = match self.remna_overview().await {
            Ok(overview) => (Some(overview), None),
            Err(err) => (None, Some(err.to_string())),
        };
        Ok(DashboardView {
            currency,
            subscriptions,
            revenue,
            stats_error,
            support_waiting,
            orders_review,
            stars_review,
            refund_actions_failed,
            remna,
            remna_error,
        })
    }

    async fn remna_overview(&self) -> ServiceResult<RemnaOverview> {
        let remna = self.services.remna()?;
        let stats = remna.stats().await?;
        let nodes = remna.nodes().await?;
        let briefs: Vec<NodeBrief> = nodes
            .iter()
            .map(|node| NodeBrief {
                uuid: node.uuid,
                name: node.name.clone(),
                country_code: node.country_code.clone(),
                state: node.state(),
                users_online: node.users_online,
                traffic_alert: node.traffic_alert(),
                last_status_change: node.last_status_change,
                last_status_message: node.last_status_message.clone(),
            })
            .collect();
        Ok(RemnaOverview {
            online_now: stats.online_stats.online_now,
            users_total: stats.users.total_users,
            status_counts: stats.users.status_counts.into_iter().collect(),
            nodes_total: briefs.len(),
            nodes_online: briefs
                .iter()
                .filter(|n| n.state == NodeState::Online)
                .count(),
            nodes: briefs,
        })
    }

    pub async fn feed(&self, query: &FeedQuery, include_money: bool) -> ServiceResult<FeedPage> {
        let limit = query.limit.unwrap_or(50).clamp(1, FEED_MAX);
        let fetch = i64::from(limit) + 1;
        let mut conn = self.services.store().conn().await?;
        let scopes = remna_scopes(query.kind);
        let remna_rows: Vec<RemnaFeedRow> = if scopes.as_ref().is_some_and(Vec::is_empty) {
            Vec::new()
        } else {
            sqlx::query_as(
                "SELECT id, scope, event, occurred_at, subject_id, payload FROM remna_events \
                 WHERE ($1::bigint IS NULL OR id < $1) AND ($2::text[] IS NULL OR scope = ANY($2)) \
                 ORDER BY id DESC LIMIT $3",
            )
            .bind(query.remna_before)
            .bind(scopes)
            .bind(fetch)
            .fetch_all(conn.conn())
            .await?
        };
        let kinds = core_kinds(query.kind, include_money);
        let core_rows: Vec<CoreFeedRow> = if kinds.as_ref().is_some_and(Vec::is_empty) {
            Vec::new()
        } else {
            sqlx::query_as(
                "SELECT id, occurred_at, client_id, payload FROM domain_events \
                 WHERE ($1::bigint IS NULL OR id < $1) AND ($2::text[] IS NULL OR kind = ANY($2)) \
                 ORDER BY id DESC LIMIT $3",
            )
            .bind(query.core_before)
            .bind(kinds)
            .bind(fetch)
            .fetch_all(conn.conn())
            .await?
        };
        let fetched = remna_rows.len() + core_rows.len();
        let mut remna_items = Vec::with_capacity(remna_rows.len());
        for row in remna_rows {
            let (title, data) = remna_feed_entry(&row.scope, &row.event, &row.payload);
            remna_items.push(FeedItem {
                source: FeedSource::Remnawave,
                id: row.id,
                at: row.occurred_at,
                kind: row.event,
                title,
                client_id: None,
                subject: row.subject_id,
                data,
            });
        }
        let mut core_items = Vec::with_capacity(core_rows.len());
        for row in core_rows {
            let event = row.payload.0;
            core_items.push(FeedItem {
                source: FeedSource::Core,
                id: row.id,
                at: row.occurred_at,
                kind: event.kind().to_string(),
                title: core_feed_title(&event),
                client_id: row.client_id.map(ClientId),
                subject: None,
                data: serde_json::to_value(&event)
                    .map_err(|e| ServiceError::Internal(e.to_string()))?,
            });
        }
        let items = merge_feed(remna_items, core_items, limit as usize);
        let last = |source: FeedSource| {
            items
                .iter()
                .filter(|i| i.source == source)
                .map(|i| i.id)
                .min()
        };
        Ok(FeedPage {
            remna_before: last(FeedSource::Remnawave).or(query.remna_before),
            core_before: last(FeedSource::Core).or(query.core_before),
            has_more: fetched > items.len(),
            items,
        })
    }

    pub async fn payments(
        &self,
        filter: &PaymentFilter,
        page: PageRequest,
    ) -> ServiceResult<Page<Payment>> {
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<PaymentRow> = sqlx::query_as(
            "SELECT id, public_id, order_id, client_id, provider, method, amount_minor, currency, status, \
             provider_txid, pay_url, expires_at, paid_at, created_at FROM payments \
             WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR provider = $2) \
             AND ($3::bigint IS NULL OR client_id = $3) AND ($4::bigint IS NULL OR id < $4) \
             ORDER BY id DESC LIMIT $5",
        )
        .bind(filter.status.map(PaymentStatus::as_str))
        .bind(filter.provider.as_deref().map(str::trim).filter(|p| !p.is_empty()))
        .bind(filter.client_id.map(|c| c.0))
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(conn.conn())
        .await?;
        let items = rows
            .into_iter()
            .map(Payment::try_from)
            .collect::<ServiceResult<Vec<Payment>>>()?;
        Ok(Page::from_overfetch(items, page, |p| p.id.0))
    }

    pub async fn mark_paid(
        &self,
        id: PaymentId,
        request: &MarkPaidRequest,
        admin: AdminId,
    ) -> ServiceResult<MarkPaidResult> {
        let payment = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().get(id).await?
        };
        match payment.provider.as_str() {
            providers::STARS => {
                return Err(ServiceError::rejected(
                    "payment.manual_stars",
                    "Оплату звёздами нельзя отметить вручную: её подтверждает только Telegram",
                ));
            }
            providers::BALANCE => {
                return Err(ServiceError::rejected(
                    "payment.manual_balance",
                    "Оплата с баланса проводится сразу, отмечать её не нужно",
                ));
            }
            _ => {}
        }
        if payment.amount.as_money().is_none() {
            return Err(ServiceError::rejected(
                "payment.manual_currency",
                "Вручную отмечается только оплата в валюте",
            ));
        }
        if !matches!(
            payment.status,
            PaymentStatus::Pending | PaymentStatus::Failed | PaymentStatus::Canceled
        ) {
            return Err(ServiceError::rejected(
                "payment.manual_status",
                format!("Платёж уже в статусе {}", payment.status.as_str()),
            ));
        }
        let txid = request
            .provider_txid
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.chars().take(200).collect::<String>());
        let comment = request
            .comment
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| c.chars().take(500).collect::<String>());
        let outcome = WebhookOutcome {
            event_key: format!("manual:{}:{}", admin.0, payment.id.0),
            payment: Some(PaymentRef::Payment(payment.id)),
            provider_txid: txid.clone(),
            status: ProviderStatus::Paid,
            amount: Some(payment.amount),
            saved_method: None,
            reply: WebhookReply::ok(),
        };
        let accepted = self
            .services
            .inbox()
            .apply_outcome(payment.provider.as_str(), &outcome)
            .await?;
        let result = match accepted {
            AcceptResult::Accepted { order_id, .. } => MarkPaidResult::Accepted { order_id },
            AcceptResult::AlreadyApplied { .. } => MarkPaidResult::AlreadyApplied,
            AcceptResult::StatusChanged { status, .. } => {
                return Err(ServiceError::rejected(
                    "payment.manual_status",
                    format!("Платёж перешёл в статус {}", status.as_str()),
                ));
            }
            AcceptResult::Ignored { reason } => {
                return Err(ServiceError::rejected("payment.manual_ignored", reason));
            }
        };
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "payment.mark_paid",
                "payment",
                Some(payment.id.0),
                serde_json::json!({
                    "client_id": payment.client_id,
                    "previous_status": payment.status,
                    "provider_txid": txid,
                    "comment": comment,
                }),
            ))
            .await?;
        Ok(result)
    }

    pub async fn refund(
        &self,
        id: PaymentId,
        request: &RefundRequest,
        admin: AdminId,
    ) -> ServiceResult<RefundResult> {
        let payment = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().get(id).await?
        };
        if !matches!(
            payment.status,
            PaymentStatus::Paid | PaymentStatus::PartiallyRefunded
        ) {
            return Err(ServiceError::rejected(
                "payment.refund_status",
                "Вернуть можно только оплаченный платёж",
            ));
        }
        if request
            .amount_minor
            .is_some_and(|amount| amount <= 0 || amount > payment.amount.minor)
        {
            return Err(ServiceError::rejected(
                "payment.refund_amount",
                "Сумма возврата больше суммы платежа",
            ));
        }
        let command = RefundCommand {
            amount: request
                .amount_minor
                .map(|minor| Amount::new(minor, payment.amount.currency)),
            method: request.method,
            subscription_action: request.subscription_action,
            reason: request.reason.clone(),
        };
        self.services.inbox().refund(id, &command, admin).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn лента_не_показывает_ссылку_подписки_из_события() {
        let payload = serde_json::json!({
            "scope": "user", "event": "user.limited", "timestamp": "2026-11-16T14:52:00Z",
            "data": { "user": { "id": 42, "username": "sn_abc", "status": "LIMITED",
                "subscriptionUrl": "https://sub.example.com/secret", "expireAt": "2027-02-14T00:00:00Z" },
                "meta": {} }
        });
        let (title, data) = remna_feed_entry("user", "user.limited", &payload);
        assert_eq!(title, "У sn_abc закончился трафик");
        assert_eq!(data["username"], "sn_abc");
        assert!(!data.to_string().contains("secret"));
    }

    #[test]
    fn заголовки_событий_нод_и_оплаты_серверов() {
        let node = serde_json::json!({ "data": { "name": "NL-2", "address": "198.51.100.22" } });
        assert_eq!(
            remna_feed_entry("node", "node.connection_lost", &node).0,
            "Нода NL-2 не отвечает"
        );
        let crm = serde_json::json!({ "data": { "nodeName": "FI-1", "providerName": "Hetzner",
            "nextBillingAt": "2026-11-18T00:00:00Z" } });
        assert_eq!(
            remna_feed_entry("crm", "crm.infra_billing_node_payment_in_7_days", &crm).0,
            "Скоро оплата сервера FI-1 (Hetzner)"
        );
    }

    #[test]
    fn без_права_на_деньги_денежные_события_не_выбираются() {
        let kinds = core_kinds(FeedKind::All, false).unwrap();
        assert!(!kinds.iter().any(|k| MONEY_EVENTS.contains(&k.as_str())));
        assert!(core_kinds(FeedKind::All, true).is_none());
        assert!(core_kinds(FeedKind::Nodes, true).unwrap().is_empty());
        assert_eq!(
            core_kinds(FeedKind::Payments, false).unwrap(),
            vec!["order_fulfilled".to_string()]
        );
    }
}
