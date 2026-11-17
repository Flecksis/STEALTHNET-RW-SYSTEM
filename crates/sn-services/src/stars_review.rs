use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sn_domain::client::IdentityKind;
use sn_domain::ids::{AdminId, ClientId, OrderId, PaymentId};
use sn_domain::money::{Amount, PayCurrency, Stars};
use sn_domain::order::OrderStatus;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{PaymentStatus, providers};
use sn_payments::providers::{STARS_TOKEN_FIELD, StarsPaymentEvent, TelegramStars};
use sn_payments::{PaymentError, ProviderConfig};
use sn_store::{ActorKind, AuditEntry, Db, StoreError, Tx, audit_entity};

#[cfg(test)]
mod tests;

pub const COMMENT_MAX: usize = 500;
pub const STATUS_REVIEW: &str = "review";
pub const STATUS_REFUNDED: &str = "refunded";
pub const STATUS_CLOSED: &str = "closed";
pub const REFUNDED_ACTION: &str = "payment.stars_review_refunded";
pub const REFUND_FAILED_ACTION: &str = "payment.stars_review_refund_failed";
pub const CLOSED_ACTION: &str = "payment.stars_review_closed";

const SUCCESSFUL_PAYMENT: &str = "successful_payment";
const REFUNDED_PAYMENT: &str = "refunded_payment";
const ALREADY_REFUNDED: &str = "CHARGE_ALREADY_REFUNDED";
const ERROR_MAX: usize = 300;

const LIST_SQL: &str = "SELECT e.id, e.charge_id, e.event, e.payload, e.source, e.attempts, e.last_error, \
     e.created_at, e.updated_at, \
     p.id AS payment_id, p.client_id AS payment_client_id, p.status AS payment_status, \
     p.amount_minor AS payment_minor, p.currency AS payment_currency, p.provider_txid, \
     o.id AS order_id, o.status AS order_status, \
     c.id AS payer_client_id, \
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
    pub invoice_payload: Option<String>,
    pub payment: Option<StarsReviewPayment>,
    pub order: Option<StarsReviewOrder>,
    pub reason: Option<String>,
    pub source: String,
    pub attempts: i32,
    pub refundable: bool,
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
    order_id: Option<i64>,
    order_status: Option<String>,
    payer_client_id: Option<i64>,
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
        Ok(StarsReviewItem {
            id: row.id,
            charge_id: row.charge_id,
            event,
            stars: payload.stars,
            telegram_user_id: payload.payer,
            payer_client_id: row.payer_client_id.map(ClientId),
            invoice_payload: payload.invoice,
            payment,
            order,
            reason: row.last_error,
            source: row.source,
            attempts: row.attempts,
            refundable: event == StarsPaymentEvent::SuccessfulPayment
                && payload.payer.is_some()
                && !row.credited,
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
                own = TelegramStars::new();
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
}
