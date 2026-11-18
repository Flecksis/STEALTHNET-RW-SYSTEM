use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use crate::stars_ledger::{self, RefundNotice};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sn_domain::client::IdentityKind;
use sn_domain::ids::{AdminId, ClientId, OrderId, PaymentId};
use sn_domain::money::{Amount, PayCurrency, Stars};
use sn_domain::order::OrderStatus;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{Payment, PaymentStatus, providers};
use sn_payments::providers::{STARS_TOKEN_FIELD, StarsPayment, StarsPaymentEvent, TelegramStars};
use sn_payments::{PaymentError, ProviderConfig};
use sn_store::{ActorKind, AuditEntry, Db, StoreError, Tx, audit_entity};
use std::time::Duration;
use uuid::Uuid;

#[cfg(test)]
mod tests;

pub const COMMENT_MAX: usize = 500;
pub const STATUS_REVIEW: &str = "review";
pub const STATUS_REFUNDED: &str = "refunded";
pub const STATUS_CLOSED: &str = "closed";
pub const REFUNDED_ACTION: &str = "payment.stars_review_refunded";
pub const REFUND_FAILED_ACTION: &str = "payment.stars_review_refund_failed";
pub const CLOSED_ACTION: &str = "payment.stars_review_closed";
pub const RETRIED_ACTION: &str = "payment.stars_review_retried";

const SUCCESSFUL_PAYMENT: &str = "successful_payment";
const REFUNDED_PAYMENT: &str = "refunded_payment";
const ALREADY_REFUNDED: &str = "CHARGE_ALREADY_REFUNDED";
const ERROR_MAX: usize = 300;
const RETRY_ATTEMPTS: u32 = 2;
const RETRY_LEASE: Duration = Duration::from_secs(300);
const RETRY_PAUSE: Duration = Duration::from_secs(60);

const LIST_SQL: &str = "SELECT e.id, e.charge_id, e.event, e.payload, e.source, e.attempts, e.last_error, \
     e.created_at, e.updated_at, \
     p.id AS payment_id, p.client_id AS payment_client_id, p.status AS payment_status, \
     p.amount_minor AS payment_minor, p.currency AS payment_currency, p.provider_txid, \
     p.provider AS payment_provider, p.public_id AS payment_public_id, \
     o.id AS order_id, o.status AS order_status, \
     c.id AS payer_client_id, \
     CASE WHEN c.id IS NOT NULL THEN i.meta ->> 'first_name' END AS payer_first_name, \
     CASE WHEN c.id IS NOT NULL THEN i.meta ->> 'last_name' END AS payer_last_name, \
     EXISTS (SELECT 1 FROM bot_star_events r \
         WHERE r.charge_id = e.charge_id AND r.event = 'refunded_payment') AS telegram_refunded, \
     EXISTS (SELECT 1 FROM payments x \
         WHERE x.provider = $3 AND x.provider_txid = e.charge_id) AS credited \
     FROM bot_star_events e \
     LEFT JOIN payments p ON p.id = e.payment_id \
     LEFT JOIN orders o ON o.id = p.order_id \
     LEFT JOIN client_identities i ON i.kind = 'telegram' AND i.value = e.payload ->> 'telegram_user_id' \
     LEFT JOIN clients c ON c.id = i.client_id AND c.deleted_at IS NULL \
     WHERE e.status = 'review' AND ($1::bigint IS NULL OR e.id < $1) \
     ORDER BY e.id DESC LIMIT $2";

const COUNT_SQL: &str = "SELECT count(*) FROM bot_star_events WHERE status = 'review'";

const LOCK_SQL: &str = "SELECT id, charge_id, event, payload, status, last_error, payment_id, \
     resolved_at, resolved_by, resolution_note \
     FROM bot_star_events WHERE id = $1 FOR UPDATE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StarsResolutionKind {
    Refunded,
    Closed,
}

impl StarsResolutionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            StarsResolutionKind::Refunded => STATUS_REFUNDED,
            StarsResolutionKind::Closed => STATUS_CLOSED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StarsReviewPayment {
    pub id: PaymentId,
    pub client_id: ClientId,
    pub status: PaymentStatus,
    pub amount: Amount,
    pub provider_txid: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StarsReviewOrder {
    pub id: OrderId,
    pub status: OrderStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StarsReviewItem {
    pub id: i64,
    pub charge_id: String,
    pub event: StarsPaymentEvent,
    pub stars: Option<Stars>,
    pub telegram_user_id: Option<i64>,
    pub payer_client_id: Option<ClientId>,
    pub payer_name: Option<String>,
    pub invoice_payload: Option<String>,
    pub payment: Option<StarsReviewPayment>,
    pub order: Option<StarsReviewOrder>,
    pub reason: Option<String>,
    pub source: String,
    pub attempts: i32,
    pub refundable: bool,
    pub retryable: bool,
    pub telegram_refunded: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StarsReviewPage {
    pub items: Vec<StarsReviewItem>,
    pub next: Option<i64>,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StarsResolution {
    pub id: i64,
    pub charge_id: String,
    pub status: StarsResolutionKind,
    pub repeated: bool,
    pub already_refunded: bool,
    pub stars: Option<Stars>,
    pub telegram_user_id: Option<i64>,
    pub payment_id: Option<PaymentId>,
    pub resolved_at: DateTime<Utc>,
    pub resolved_by: Option<AdminId>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StarsRetryOutcome {
    Done,
    Review,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StarsRetry {
    pub id: i64,
    pub charge_id: String,
    pub outcome: StarsRetryOutcome,
    pub reason: Option<String>,
    pub payment_id: Option<PaymentId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Payload {
    stars: Option<Stars>,
    payer: Option<i64>,
    invoice: Option<String>,
}

impl Payload {
    fn read(value: &Value) -> Self {
        Payload {
            stars: value.get("total_amount").and_then(Value::as_i64).map(Stars),
            payer: value
                .get("telegram_user_id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0),
            invoice: value
                .get("invoice_payload")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|payload| !payload.is_empty())
                .map(str::to_string),
        }
    }
}

#[derive(sqlx::FromRow)]
struct ItemRow {
    id: i64,
    charge_id: String,
    event: String,
    payload: Value,
    source: String,
    attempts: i32,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    payment_id: Option<i64>,
    payment_client_id: Option<i64>,
    payment_status: Option<String>,
    payment_minor: Option<i64>,
    payment_currency: Option<String>,
    provider_txid: Option<String>,
    payment_provider: Option<String>,
    payment_public_id: Option<Uuid>,
    order_id: Option<i64>,
    order_status: Option<String>,
    payer_client_id: Option<i64>,
    payer_first_name: Option<String>,
    payer_last_name: Option<String>,
    telegram_refunded: bool,
    credited: bool,
}

#[derive(sqlx::FromRow)]
struct EventRow {
    id: i64,
    charge_id: String,
    event: String,
    payload: Value,
    status: String,
    last_error: Option<String>,
    payment_id: Option<i64>,
    resolved_at: Option<DateTime<Utc>>,
    resolved_by: Option<i64>,
    resolution_note: Option<String>,
}

struct Invoice<'a> {
    provider: &'a str,
    public_id: Uuid,
    status: PaymentStatus,
    amount: Amount,
}

impl<'a> From<&'a Payment> for Invoice<'a> {
    fn from(payment: &'a Payment) -> Self {
        Invoice {
            provider: payment.provider.as_str(),
            public_id: payment.public_id,
            status: payment.status,
            amount: payment.amount,
        }
    }
}

fn retryable(
    event: StarsPaymentEvent,
    payload: &Value,
    invoice: Option<Invoice<'_>>,
    credited: bool,
) -> bool {
    let Ok(paid) = serde_json::from_value::<StarsPayment>(payload.clone()) else {
        return false;
    };
    event == StarsPaymentEvent::SuccessfulPayment
        && paid.event == StarsPaymentEvent::SuccessfulPayment
        && paid.telegram_user_id > 0
        && !credited
        && invoice.is_some_and(|invoice| {
            invoice.provider == providers::STARS
                && invoice.amount == Amount::new(paid.total_amount, PayCurrency::Xtr)
                && Uuid::parse_str(paid.invoice_payload.trim()).ok() == Some(invoice.public_id)
                && matches!(
                    invoice.status,
                    PaymentStatus::Pending | PaymentStatus::Failed | PaymentStatus::Canceled
                )
        })
}

fn full_name(first: Option<&str>, last: Option<&str>) -> Option<String> {
    let name = [first, last]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!name.is_empty()).then_some(name)
}

fn event_kind(value: &str) -> ServiceResult<StarsPaymentEvent> {
    match value {
        SUCCESSFUL_PAYMENT => Ok(StarsPaymentEvent::SuccessfulPayment),
        REFUNDED_PAYMENT => Ok(StarsPaymentEvent::RefundedPayment),
        other => Err(StoreError::Decode(format!("событие оплаты звёздами: {other}")).into()),
    }
}

impl TryFrom<ItemRow> for StarsReviewItem {
    type Error = ServiceError;

    fn try_from(row: ItemRow) -> Result<Self, Self::Error> {
        let event = event_kind(&row.event)?;
        let payload = Payload::read(&row.payload);
        let payment = match (
            row.payment_id,
            row.payment_client_id,
            row.payment_status,
            row.payment_minor,
            row.payment_currency,
        ) {
            (Some(id), Some(client), Some(status), Some(minor), Some(currency)) => {
                Some(StarsReviewPayment {
                    id: PaymentId(id),
                    client_id: ClientId(client),
                    status: PaymentStatus::parse(&status)?,
                    amount: Amount::new(minor, PayCurrency::parse(&currency)?),
                    provider_txid: row.provider_txid,
                })
            }
            _ => None,
        };
        let order = match (row.order_id, row.order_status) {
            (Some(id), Some(status)) => Some(StarsReviewOrder {
                id: OrderId(id),
                status: OrderStatus::parse(&status)?,
            }),
            _ => None,
        };
        let invoice = match (
            &payment,
            row.payment_provider.as_deref(),
            row.payment_public_id,
        ) {
            (Some(payment), Some(provider), Some(public_id)) => Some(Invoice {
                provider,
                public_id,
                status: payment.status,
                amount: payment.amount,
            }),
            _ => None,
        };
        let retryable =
            retryable(event, &row.payload, invoice, row.credited) && !row.telegram_refunded;
        Ok(StarsReviewItem {
            id: row.id,
            charge_id: row.charge_id,
            event,
            stars: payload.stars,
            telegram_user_id: payload.payer,
            payer_client_id: row.payer_client_id.map(ClientId),
            payer_name: full_name(
                row.payer_first_name.as_deref(),
                row.payer_last_name.as_deref(),
            ),
            invoice_payload: payload.invoice,
            payment,
            order,
            reason: row.last_error,
            source: row.source,
            attempts: row.attempts,
            refundable: event == StarsPaymentEvent::SuccessfulPayment
                && payload.payer.is_some()
                && !row.credited
                && !row.telegram_refunded,
            retryable,
            telegram_refunded: row.telegram_refunded,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

impl EventRow {
    fn resolution(
        &self,
        payload: &Payload,
        status: StarsResolutionKind,
    ) -> ServiceResult<StarsResolution> {
        let resolved_at = self.resolved_at.ok_or_else(|| {
            StoreError::Decode("у разобранной оплаты звёздами нет времени разбора".into())
        })?;
        Ok(StarsResolution {
            id: self.id,
            charge_id: self.charge_id.clone(),
            status,
            repeated: true,
            already_refunded: false,
            stars: payload.stars,
            telegram_user_id: payload.payer,
            payment_id: self.payment_id.map(PaymentId),
            resolved_at,
            resolved_by: self.resolved_by.map(AdminId),
            comment: self.resolution_note.clone(),
        })
    }
}

struct Decision {
    status: StarsResolutionKind,
    action: &'static str,
    at: DateTime<Utc>,
    admin: AdminId,
    comment: Option<String>,
    already_refunded: bool,
}

fn clean_comment(comment: Option<&str>) -> Option<String> {
    comment
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(COMMENT_MAX).collect())
}

fn not_waiting() -> ServiceError {
    ServiceError::rejected("stars.review_status", "Это событие не ждёт ручной проверки")
}

async fn lock(tx: &mut Tx, id: i64) -> ServiceResult<EventRow> {
    sqlx::query_as(LOCK_SQL)
        .bind(id)
        .fetch_optional(tx.conn())
        .await?
        .ok_or(ServiceError::NotFound("событие оплаты звёздами"))
}

async fn audit(
    tx: &mut Tx,
    admin: AdminId,
    action: &str,
    event: &EventRow,
    payload: &Payload,
    extra: Value,
) -> ServiceResult<()> {
    let payer_client = match (event.payment_id, payload.payer) {
        (None, Some(payer)) => tx
            .clients()
            .find_by_identity(IdentityKind::Telegram, &payer.to_string())
            .await?
            .map(|client| client.id.0),
        _ => None,
    };
    let (entity_kind, entity_id) = match (event.payment_id, payer_client) {
        (Some(payment), _) => (Some(audit_entity::PAYMENT), Some(payment)),
        (None, Some(client)) => (Some(audit_entity::CLIENT), Some(client)),
        (None, None) => (None, None),
    };
    let mut details = serde_json::json!({
        "event_id": event.id,
        "charge_id": event.charge_id,
        "event": event.event,
        "stars": payload.stars,
        "telegram_user_id": payload.payer,
        "reason": event.last_error,
    });
    if let (Some(target), Value::Object(extra)) = (details.as_object_mut(), extra) {
        target.extend(extra);
    }
    tx.audit()
        .record(&AuditEntry {
            actor_kind: ActorKind::Admin,
            actor_id: Some(admin.0),
            action: action.to_string(),
            entity_kind: entity_kind.map(str::to_string),
            entity_id,
            details,
            ip: None,
        })
        .await?;
    Ok(())
}

async fn telegram_told(tx: &mut Tx, charge: &str) -> ServiceResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM bot_star_events \
         WHERE charge_id = $1 AND event = 'refunded_payment' AND status = 'done')",
    )
    .bind(charge)
    .fetch_one(tx.conn())
    .await?)
}

async fn settle(
    mut tx: Tx,
    event: EventRow,
    payload: Payload,
    decision: Decision,
) -> ServiceResult<StarsResolution> {
    let updated = sqlx::query(
        "UPDATE bot_star_events SET status = $2, resolved_at = $3, resolved_by = $4, resolution_note = $5 \
         WHERE id = $1 AND status = 'review'",
    )
    .bind(event.id)
    .bind(decision.status.as_str())
    .bind(decision.at)
    .bind(decision.admin.0)
    .bind(decision.comment.as_deref())
    .execute(tx.conn())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(not_waiting());
    }
    let details = match decision.status {
        StarsResolutionKind::Refunded => serde_json::json!({
            "comment": decision.comment,
            "already_refunded": decision.already_refunded,
        }),
        StarsResolutionKind::Closed => serde_json::json!({ "comment": decision.comment }),
    };
    let told = decision.already_refunded && telegram_told(&mut tx, &event.charge_id).await?;
    if decision.status == StarsResolutionKind::Refunded
        && !told
        && let Some(stars) = payload.stars
    {
        stars_ledger::refund_notice(
            &mut tx,
            &RefundNotice {
                charge: &event.charge_id,
                stars: stars.0,
                payer: payload.payer,
                invoice: payload.invoice.as_deref(),
                payment: event.payment_id.map(PaymentId),
            },
        )
        .await?;
    }
    audit(
        &mut tx,
        decision.admin,
        decision.action,
        &event,
        &payload,
        details,
    )
    .await?;
    tx.commit().await?;
    tracing::info!(
        event = event.id,
        charge = %event.charge_id,
        status = decision.status.as_str(),
        admin = %decision.admin,
        "оплата звёздами разобрана вручную"
    );
    Ok(StarsResolution {
        id: event.id,
        charge_id: event.charge_id,
        status: decision.status,
        repeated: false,
        already_refunded: decision.already_refunded,
        stars: payload.stars,
        telegram_user_id: payload.payer,
        payment_id: event.payment_id.map(PaymentId),
        resolved_at: decision.at,
        resolved_by: Some(decision.admin),
        comment: decision.comment,
    })
}

async fn note_failure(
    mut tx: Tx,
    admin: AdminId,
    event: &EventRow,
    payload: &Payload,
    details: Value,
) -> ServiceResult<()> {
    audit(
        &mut tx,
        admin,
        REFUND_FAILED_ACTION,
        event,
        payload,
        details,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub struct StarsReview<'a> {
    services: &'a Services,
    telegram: Option<&'a TelegramStars>,
}

impl<'a> StarsReview<'a> {
    pub fn new(services: &'a Services) -> Self {
        StarsReview {
            services,
            telegram: None,
        }
    }

    pub fn with_telegram(services: &'a Services, telegram: &'a TelegramStars) -> Self {
        StarsReview {
            services,
            telegram: Some(telegram),
        }
    }

    pub async fn count(&self) -> ServiceResult<i64> {
        let mut conn = self.services.store().conn().await?;
        Ok(sqlx::query_scalar(COUNT_SQL).fetch_one(conn.conn()).await?)
    }

    pub async fn list(&self, page: PageRequest) -> ServiceResult<StarsReviewPage> {
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<ItemRow> = sqlx::query_as(LIST_SQL)
            .bind(page.after)
            .bind(page.fetch_limit())
            .bind(providers::STARS)
            .fetch_all(conn.conn())
            .await?;
        let total: i64 = sqlx::query_scalar(COUNT_SQL).fetch_one(conn.conn()).await?;
        drop(conn);
        let items = rows
            .into_iter()
            .map(StarsReviewItem::try_from)
            .collect::<ServiceResult<Vec<StarsReviewItem>>>()?;
        let Page { items, next } = Page::from_overfetch(items, page, |item| item.id);
        Ok(StarsReviewPage { items, next, total })
    }

    pub async fn close(
        &self,
        id: i64,
        comment: &str,
        admin: AdminId,
    ) -> ServiceResult<StarsResolution> {
        let comment = clean_comment(Some(comment)).ok_or_else(|| {
            ServiceError::rejected(
                "stars.review_comment",
                "Напишите пояснение: почему событие закрывается без возврата звёзд",
            )
        })?;
        let mut tx = self.services.store().begin().await?;
        let event = lock(&mut tx, id).await?;
        let payload = Payload::read(&event.payload);
        match event.status.as_str() {
            STATUS_REVIEW => {}
            STATUS_CLOSED => return event.resolution(&payload, StarsResolutionKind::Closed),
            STATUS_REFUNDED => {
                return Err(ServiceError::rejected(
                    "stars.review_refunded",
                    "Звёзды по этому событию уже возвращены",
                ));
            }
            _ => return Err(not_waiting()),
        }
        let decision = Decision {
            status: StarsResolutionKind::Closed,
            action: CLOSED_ACTION,
            at: self.services.now(),
            admin,
            comment: Some(comment),
            already_refunded: false,
        };
        settle(tx, event, payload, decision).await
    }

    pub async fn refund(
        &self,
        id: i64,
        comment: Option<&str>,
        admin: AdminId,
    ) -> ServiceResult<StarsResolution> {
        let comment = clean_comment(comment);
        let mut tx = self.services.store().begin().await?;
        let event = lock(&mut tx, id).await?;
        let payload = Payload::read(&event.payload);
        match event.status.as_str() {
            STATUS_REVIEW => {}
            STATUS_REFUNDED => return event.resolution(&payload, StarsResolutionKind::Refunded),
            STATUS_CLOSED => {
                return Err(ServiceError::rejected(
                    "stars.review_closed",
                    "Событие уже закрыто без возврата звёзд",
                ));
            }
            _ => return Err(not_waiting()),
        }
        if event.event != SUCCESSFUL_PAYMENT {
            return Err(ServiceError::rejected(
                "stars.review_not_payment",
                "Это уведомление о возврате: звёзды уже у клиента, событие можно только закрыть",
            ));
        }
        let payer = payload.payer.ok_or_else(|| {
            ServiceError::rejected(
                "stars.review_no_payer",
                "В событии нет плательщика: вернуть звёзды нельзя, закройте событие с пояснением",
            )
        })?;
        if let Some(credited) = tx
            .payments()
            .by_provider_txid(providers::STARS, &event.charge_id)
            .await?
        {
            return Err(ServiceError::rejected(
                "stars.review_credited",
                format!(
                    "Это списание уже проведено как оплата платежа {}: оформите возврат этого платежа",
                    credited.id
                ),
            ));
        }
        let token = self.services.bot_token().ok_or_else(|| {
            ServiceError::rejected(
                "stars.bot_token_missing",
                "Не задан токен бота: вернуть звёзды нельзя",
            )
        })?;
        let mut config = ProviderConfig::default();
        config.set(STARS_TOKEN_FIELD, Value::String(token));
        let own;
        let telegram = match self.telegram {
            Some(telegram) => telegram,
            None => {
                own = TelegramStars::with_api_base(self.services.payments().telegram_api());
                &own
            }
        };
        let already_refunded = match telegram
            .refund_star_payment(&config, payer, &event.charge_id)
            .await
        {
            Ok(_) => false,
            Err(PaymentError::Rejected(message)) if message.contains(ALREADY_REFUNDED) => true,
            Err(err) => {
                let error: String = err.to_string().chars().take(ERROR_MAX).collect();
                tracing::warn!(event = event.id, charge = %event.charge_id, error = %error, "звёзды не возвращены");
                let details = serde_json::json!({ "comment": comment, "error": error });
                if let Err(lost) = note_failure(tx, admin, &event, &payload, details).await {
                    tracing::warn!(event = event.id, error = %lost, "неудачный возврат звёзд не записан в аудит");
                }
                return Err(err.into());
            }
        };
        let (event_id, charge) = (event.id, event.charge_id.clone());
        let decision = Decision {
            status: StarsResolutionKind::Refunded,
            action: REFUNDED_ACTION,
            at: self.services.now(),
            admin,
            comment,
            already_refunded,
        };
        settle(tx, event, payload, decision).await.inspect_err(|err| {
            tracing::error!(event = event_id, charge = %charge, error = %err, "звёзды возвращены, но событие не закрыто: повторите возврат");
        })
    }

    pub async fn retry(&self, id: i64, admin: AdminId) -> ServiceResult<StarsRetry> {
        let mut tx = self.services.store().begin().await?;
        let event = lock(&mut tx, id).await?;
        let payload = Payload::read(&event.payload);
        match event.status.as_str() {
            STATUS_REVIEW => {}
            STATUS_REFUNDED => {
                return Err(ServiceError::rejected(
                    "stars.review_refunded",
                    "Звёзды по этому событию уже возвращены",
                ));
            }
            STATUS_CLOSED => {
                return Err(ServiceError::rejected(
                    "stars.review_closed",
                    "Событие уже закрыто без возврата звёзд",
                ));
            }
            _ => return Err(not_waiting()),
        }
        let kind = event_kind(&event.event)?;
        let invoice = match event.payment_id {
            Some(payment) => Some(tx.payments().get(PaymentId(payment)).await?),
            None => None,
        };
        let credited = tx
            .payments()
            .by_provider_txid(providers::STARS, &event.charge_id)
            .await?
            .is_some();
        if kind == StarsPaymentEvent::SuccessfulPayment
            && !credited
            && stars_ledger::returned(&mut tx, &event.charge_id, false).await?
        {
            return Err(ServiceError::rejected(
                "stars.review_returned",
                "Telegram уже вернул звёзды по этому списанию: проводить оплату нельзя, закройте событие",
            ));
        }
        let paid = serde_json::from_value::<StarsPayment>(event.payload.clone())
            .ok()
            .filter(|_| retryable(kind, &event.payload, invoice.as_ref().map(Invoice::from), credited))
            .ok_or_else(|| {
                ServiceError::rejected(
                    "stars.review_not_retryable",
                    "Эту оплату нельзя провести снова: она на проверке из-за расхождения, а не из-за сбоя. Верните звёзды или закройте событие с пояснением",
                )
            })?;
        sqlx::query(
            "UPDATE bot_star_events SET status = 'pending', next_attempt_at = $2 \
             WHERE id = $1 AND status = 'review'",
        )
        .bind(event.id)
        .bind(stars_ledger::later(self.services, RETRY_LEASE))
        .execute(tx.conn())
        .await?;
        audit(
            &mut tx,
            admin,
            RETRIED_ACTION,
            &event,
            &payload,
            serde_json::json!({}),
        )
        .await?;
        tx.commit().await?;
        let verdict = stars_ledger::process(self.services, &paid, RETRY_ATTEMPTS).await;
        stars_ledger::finish(self.services, &paid, &verdict, RETRY_PAUSE).await?;
        let mut conn = self.services.store().conn().await?;
        let (status, reason, payment): (String, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT status, last_error, payment_id FROM bot_star_events WHERE id = $1",
        )
        .bind(event.id)
        .fetch_one(conn.conn())
        .await?;
        let outcome = match status.as_str() {
            "done" => StarsRetryOutcome::Done,
            STATUS_REVIEW => StarsRetryOutcome::Review,
            _ => StarsRetryOutcome::Pending,
        };
        tracing::info!(
            event = event.id,
            charge = %event.charge_id,
            admin = %admin,
            outcome = ?outcome,
            "оплату звёздами провели снова по просьбе администратора"
        );
        Ok(StarsRetry {
            id: event.id,
            charge_id: event.charge_id,
            outcome,
            reason,
            payment_id: payment.map(PaymentId),
        })
    }
}
