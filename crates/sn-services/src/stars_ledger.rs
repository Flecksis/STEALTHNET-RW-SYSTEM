use crate::error::ServiceError;
use crate::inbox::{AcceptResult, BLOCKED_REFUND_REASON, REPEAT_METHOD};
use crate::services::Services;
use chrono::{DateTime, Utc};
use sn_domain::client::{IdentityKind, Locale};
use sn_domain::ids::PaymentId;
use sn_domain::jobs::JobPayload;
use sn_domain::lang;
use sn_domain::payment::{Payment, providers};
use sn_payments::PaymentError;
use sn_payments::providers::{StarsPayment, StarsPaymentEvent};
use sn_store::{ActorKind, AuditEntry, Db, StoreError, StoreResult, Tx, audit_entity};
use std::time::Duration;
use uuid::Uuid;

pub const REFUNDED_TEMPLATE: &str = "payment.stars_refunded";
pub const REPEAT_NOTE: &str = "повторная оплата уже оплаченного счёта ждёт ручной проверки";
pub const UNCREDITED_NOTE: &str =
    "возврат по списанию, которое не проводилось как оплата: платежи не изменены";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Applied {
        payment: Option<PaymentId>,
        note: Option<String>,
    },
    Repeat(PaymentId),
    Retry(String),
    Skip(String),
    Review {
        payment: Option<PaymentId>,
        error: String,
    },
}

pub const fn event_name(event: StarsPaymentEvent) -> &'static str {
    match event {
        StarsPaymentEvent::SuccessfulPayment => "successful_payment",
        StarsPaymentEvent::RefundedPayment => "refunded_payment",
    }
}

pub fn later(services: &Services, delay: Duration) -> DateTime<Utc> {
    services.now()
        + chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::minutes(30))
}

pub async fn ours(db: &mut impl Db, payload: &str) -> StoreResult<Option<Payment>> {
    let Ok(public_id) = Uuid::parse_str(payload.trim()) else {
        return Ok(None);
    };
    match db.payments().by_public_id(public_id).await {
        Ok(payment) if payment.provider.as_str() == providers::STARS => Ok(Some(payment)),
        Ok(_) | Err(StoreError::NotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

async fn credited(services: &Services, charge: &str) -> StoreResult<bool> {
    let mut conn = services.store().conn().await?;
    Ok(conn
        .payments()
        .by_provider_txid(providers::STARS, charge)
        .await?
        .is_some())
}

pub async fn process(services: &Services, event: &StarsPayment, retries: u32) -> Verdict {
    if event.event == StarsPaymentEvent::RefundedPayment {
        match credited(services, &event.telegram_payment_charge_id).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(
                    charge = %event.telegram_payment_charge_id,
                    "возврат звёзд по списанию, которое не проводилось как оплата"
                );
                return Verdict::Applied {
                    payment: None,
                    note: Some(UNCREDITED_NOTE.into()),
                };
            }
            Err(err) => return Verdict::Retry(err.to_string()),
        }
    }
    let mut attempt: u32 = 0;
    loop {
        match services.inbox().stars_payment(event).await {
            Ok(result) => return accepted(services, event, result).await,
            Err(err) if err.is_retryable() && attempt < retries => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(400 * u64::from(attempt))).await;
            }
            Err(err) => return rejected(err),
        }
    }
}

async fn accepted(services: &Services, event: &StarsPayment, result: AcceptResult) -> Verdict {
    match result {
        AcceptResult::Accepted { payment_id, .. }
            if event.event == StarsPaymentEvent::SuccessfulPayment =>
        {
            match is_repeat(services, payment_id).await {
                Ok(true) => Verdict::Repeat(payment_id),
                Ok(false) => Verdict::Applied {
                    payment: Some(payment_id),
                    note: None,
                },
                Err(err) => {
                    tracing::warn!(payment = %payment_id, error = %err, "не удалось проверить, повторная ли это оплата звёздами");
                    Verdict::Applied {
                        payment: Some(payment_id),
                        note: None,
                    }
                }
            }
        }
        AcceptResult::Accepted { payment_id, .. }
        | AcceptResult::StatusChanged { payment_id, .. } => Verdict::Applied {
            payment: Some(payment_id),
            note: None,
        },
        AcceptResult::AlreadyApplied { payment_id }
            if event.event == StarsPaymentEvent::SuccessfulPayment =>
        {
            match recorded_charge(services, payment_id).await {
                Ok(Some(charge)) if charge != event.telegram_payment_charge_id => Verdict::Review {
                    payment: Some(payment_id),
                    error: format!("счёт уже оплачен другой транзакцией Telegram {charge}"),
                },
                Ok(_) => Verdict::Applied {
                    payment: Some(payment_id),
                    note: None,
                },
                Err(err) => Verdict::Retry(err.to_string()),
            }
        }
        AcceptResult::AlreadyApplied { payment_id } => Verdict::Applied {
            payment: Some(payment_id),
            note: None,
        },
        AcceptResult::Ignored { reason } => Verdict::Applied {
            payment: None,
            note: Some(reason),
        },
    }
}

async fn recorded_charge(services: &Services, payment: PaymentId) -> StoreResult<Option<String>> {
    let mut conn = services.store().conn().await?;
    Ok(conn.payments().get(payment).await?.provider_txid)
}

async fn is_repeat(services: &Services, payment: PaymentId) -> StoreResult<bool> {
    let mut conn = services.store().conn().await?;
    Ok(conn.payments().get(payment).await?.method.as_deref() == Some(REPEAT_METHOD))
}

fn rejected(err: ServiceError) -> Verdict {
    if err.is_retryable() {
        return Verdict::Retry(err.to_string());
    }
    match err {
        ServiceError::Store(StoreError::NotFound(_))
        | ServiceError::Payment(PaymentError::BadPayload(_)) => Verdict::Skip(err.to_string()),
        other => Verdict::Review {
            payment: None,
            error: other.to_string(),
        },
    }
}

async fn mark_done(
    tx: &mut Tx,
    event: &StarsPayment,
    payment: Option<PaymentId>,
    note: Option<&str>,
) -> StoreResult<bool> {
    let done = sqlx::query(
        "UPDATE bot_star_events SET status = 'done', attempts = attempts + 1, last_error = $3, \
         payment_id = COALESCE($4, payment_id) \
         WHERE charge_id = $1 AND event = $2 AND status = 'pending'",
    )
    .bind(&event.telegram_payment_charge_id)
    .bind(event_name(event.event))
    .bind(note)
    .bind(payment.map(|p| p.0))
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn finish(
    services: &Services,
    event: &StarsPayment,
    verdict: &Verdict,
    delay: Duration,
) -> StoreResult<()> {
    let charge = &event.telegram_payment_charge_id;
    let name = event_name(event.event);
    let mut tx = services.store().begin().await?;
    match verdict {
        Verdict::Applied { payment, note } => {
            if mark_done(&mut tx, event, *payment, note.as_deref()).await?
                && event.event == StarsPaymentEvent::RefundedPayment
                && !refunded_by_admin(&mut tx, charge).await?
            {
                refund_notice(
                    &mut tx,
                    &RefundNotice {
                        charge,
                        stars: event.total_amount,
                        payer: Some(event.telegram_user_id),
                        invoice: Some(&event.invoice_payload),
                        payment: None,
                    },
                )
                .await?;
            }
        }
        Verdict::Repeat(payment) => {
            mark_done(&mut tx, event, Some(*payment), Some(REPEAT_NOTE)).await?;
        }
        Verdict::Retry(error) => {
            sqlx::query(
                "UPDATE bot_star_events SET attempts = attempts + 1, last_error = $3, next_attempt_at = $4 \
                 WHERE charge_id = $1 AND event = $2 AND status = 'pending'",
            )
            .bind(charge)
            .bind(name)
            .bind(error)
            .bind(later(services, delay))
            .execute(tx.conn())
            .await?;
        }
        Verdict::Skip(error) => {
            sqlx::query(
                "UPDATE bot_star_events SET status = 'skipped', attempts = attempts + 1, last_error = $3 \
                 WHERE charge_id = $1 AND event = $2 AND status = 'pending'",
            )
            .bind(charge)
            .bind(name)
            .bind(error)
            .execute(tx.conn())
            .await?;
            tracing::warn!(charge = %charge, event = name, error = %error, "оплата звёздами не к нашему счёту");
        }
        Verdict::Review { payment, error } => {
            let payment = match payment {
                Some(id) => Some(*id),
                None => ours(&mut tx, &event.invoice_payload).await?.map(|p| p.id),
            };
            let flagged = sqlx::query(
                "UPDATE bot_star_events SET status = 'review', attempts = attempts + 1, last_error = $3, \
                 payment_id = COALESCE($4, payment_id) \
                 WHERE charge_id = $1 AND event = $2 AND status = 'pending'",
            )
            .bind(charge)
            .bind(name)
            .bind(error)
            .bind(payment.map(|p| p.0))
            .execute(tx.conn())
            .await?;
            if flagged.rows_affected() > 0 {
                tx.audit()
                    .record(&AuditEntry {
                        actor_kind: ActorKind::Bot,
                        actor_id: None,
                        action: "payment.stars_review".into(),
                        entity_kind: payment.map(|_| audit_entity::PAYMENT.to_string()),
                        entity_id: payment.map(|p| p.0),
                        details: serde_json::json!({
                            "event": name,
                            "charge_id": charge,
                            "stars": event.total_amount,
                            "telegram_user_id": event.telegram_user_id,
                            "invoice_payload": event.invoice_payload,
                            "error": error,
                        }),
                        ip: None,
                    })
                    .await?;
                tracing::error!(charge = %charge, event = name, error = %error, "оплата звёздами ждёт ручной проверки");
            }
        }
    }
    tx.commit().await
}

async fn refunded_by_admin(tx: &mut Tx, charge: &str) -> StoreResult<bool> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM bot_star_events \
         WHERE charge_id = $1 AND event = 'successful_payment' FOR SHARE",
    )
    .bind(charge)
    .fetch_optional(tx.conn())
    .await?;
    Ok(status.as_deref() == Some("refunded"))
}

async fn refunded_for_blocked(tx: &mut Tx, payment: PaymentId) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM refunds WHERE payment_id = $1 AND reason = $2)",
    )
    .bind(payment.0)
    .bind(BLOCKED_REFUND_REASON)
    .fetch_one(tx.conn())
    .await?)
}

pub(crate) struct RefundNotice<'a> {
    pub charge: &'a str,
    pub stars: i64,
    pub payer: Option<i64>,
    pub invoice: Option<&'a str>,
    pub payment: Option<PaymentId>,
}

pub(crate) async fn refund_notice(tx: &mut Tx, notice: &RefundNotice<'_>) -> StoreResult<bool> {
    let payment = match notice.payment {
        Some(id) => Some(tx.payments().get(id).await?),
        None => match tx
            .payments()
            .by_provider_txid(providers::STARS, notice.charge)
            .await?
        {
            Some(payment) => Some(payment),
            None => match notice.invoice {
                Some(invoice) => ours(tx, invoice).await?,
                None => None,
            },
        },
    };
    if let Some(payment) = &payment
        && refunded_for_blocked(tx, payment.id).await?
    {
        return Ok(false);
    }
    let payer = match notice.payer.filter(|id| *id > 0) {
        Some(payer) => tx
            .clients()
            .find_by_identity(IdentityKind::Telegram, &payer.to_string())
            .await?
            .map(|client| client.id),
        None => None,
    };
    let client = payer.or(payment.as_ref().map(|payment| payment.client_id));
    let Some(client) = client else {
        tracing::info!(charge = %notice.charge, "звёзды вернули, но клиента для сообщения не нашли");
        return Ok(false);
    };
    let line = match &payment {
        Some(payment) => tx
            .orders()
            .get(payment.order_id)
            .await?
            .quote
            .lines
            .into_iter()
            .next(),
        None => None,
    };
    let label = line.as_ref().map(|line| line.label.clone());
    let label_en = line
        .as_ref()
        .map(|line| lang::sync_scope(Locale::En, || line.shown_label()));
    tx.jobs()
        .enqueue(
            &JobPayload::Notify {
                client_id: client,
                template: REFUNDED_TEMPLATE.into(),
                params: serde_json::json!({
                    "stars": notice.stars,
                    "label": label,
                    "label_en": label_en,
                    "charge_id": notice.charge,
                    "payment_id": payment.as_ref().map(|p| p.id),
                }),
            },
            None,
        )
        .await?;
    Ok(true)
}
