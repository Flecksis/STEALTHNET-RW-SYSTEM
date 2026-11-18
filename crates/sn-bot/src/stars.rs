use crate::telegram::{Message, Update};
use chrono::{DateTime, Utc};
use sn_domain::payment::{Payment, PaymentStatus, providers};
use sn_payments::ProviderConfig;
use sn_payments::providers::{
    STARS_TOKEN_FIELD, StarTransaction, StarsPayment, StarsPaymentEvent, TelegramStars,
};
use sn_services::stars_ledger::{Verdict, finish, later, ours, process};
use sn_services::{ServiceError, Services};
use sn_store::{Db, StoreError, StoreResult};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub use sn_services::stars_ledger::event_name;

pub const OFFSET_PREFIX: &str = "telegram.stars_offset.";
pub const QUICK_RETRIES: u32 = 3;
pub const PAGE: u32 = 100;
const PAGES_PER_RUN: u32 = 10;
const BATCH: i64 = 50;
const BATCHES_PER_TICK: u32 = 10;
const GRACE: Duration = Duration::from_secs(60);
const LEASE: Duration = Duration::from_secs(300);
const MAX_DELAY: Duration = Duration::from_secs(1_800);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Update,
    Reconcile,
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Update => "update",
            Source::Reconcile => "reconcile",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Done,
    Skipped,
    Review,
    Refunded,
    Closed,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Done => "done",
            Status::Skipped => "skipped",
            Status::Review => "review",
            Status::Refunded => "refunded",
            Status::Closed => "closed",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "pending" => Ok(Status::Pending),
            "done" => Ok(Status::Done),
            "skipped" => Ok(Status::Skipped),
            "review" => Ok(Status::Review),
            "refunded" => Ok(Status::Refunded),
            "closed" => Ok(Status::Closed),
            other => Err(StoreError::Decode(format!(
                "статус оплаты звёздами: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Done,
    Repeat,
    Waiting(String),
    Failed(String),
}

enum Prepared {
    Ready,
    Finished(Status),
    Blocked,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retried {
    pub taken: u32,
    pub applied: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reconciled {
    pub read: u32,
    pub recorded: u32,
    pub more: bool,
}

pub fn offset_key(bot_id: i64) -> String {
    format!("{OFFSET_PREFIX}{bot_id}")
}

pub fn retry_delay(base: Duration, attempts: i32) -> Duration {
    let step = u32::try_from(attempts.clamp(1, 7) - 1).unwrap_or(0);
    base.saturating_mul(1 << step).min(MAX_DELAY)
}

pub fn from_message(message: &Message) -> Option<StarsPayment> {
    let payer = message.from.as_ref().map_or(message.chat.id, |u| u.id);
    if let Some(paid) = &message.successful_payment {
        return Some(StarsPayment {
            event: StarsPaymentEvent::SuccessfulPayment,
            invoice_payload: paid.invoice_payload.clone(),
            total_amount: paid.total_amount,
            currency: paid.currency.clone(),
            telegram_payment_charge_id: paid.telegram_payment_charge_id.clone(),
            telegram_user_id: payer,
        });
    }
    message
        .refunded_payment
        .as_ref()
        .map(|refund| StarsPayment {
            event: StarsPaymentEvent::RefundedPayment,
            invoice_payload: refund.invoice_payload.clone(),
            total_amount: refund.total_amount,
            currency: refund.currency.clone(),
            telegram_payment_charge_id: refund.telegram_payment_charge_id.clone(),
            telegram_user_id: payer,
        })
}

pub fn from_updates(updates: &[Update]) -> Vec<StarsPayment> {
    updates
        .iter()
        .filter_map(|update| update.message.as_ref())
        .filter_map(from_message)
        .collect()
}

fn storable(event: &StarsPayment) -> bool {
    !event.telegram_payment_charge_id.trim().is_empty()
}

async fn insert(
    db: &mut impl Db,
    event: &StarsPayment,
    source: Source,
    due: DateTime<Utc>,
) -> StoreResult<bool> {
    let inserted = sqlx::query(
        "INSERT INTO bot_star_events (charge_id, event, payload, source, next_attempt_at) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (charge_id, event) DO NOTHING",
    )
    .bind(&event.telegram_payment_charge_id)
    .bind(event_name(event.event))
    .bind(serde_json::to_value(event)?)
    .bind(source.as_str())
    .bind(due)
    .execute(db.conn())
    .await?;
    Ok(inserted.rows_affected() == 1)
}

pub async fn record(services: &Services, events: &[StarsPayment]) -> StoreResult<u32> {
    let mut added = 0;
    if events.is_empty() {
        return Ok(added);
    }
    let due = later(services, GRACE);
    let mut tx = services.store().begin().await?;
    for event in events {
        if !storable(event) {
            tracing::warn!(
                event = event_name(event.event),
                "оплата звёздами пришла без идентификатора транзакции"
            );
            continue;
        }
        if insert(&mut tx, event, Source::Update, due).await? {
            added += 1;
        }
    }
    tx.commit().await?;
    Ok(added)
}

async fn stored_status(db: &mut impl Db, event: &StarsPayment) -> StoreResult<Option<Status>> {
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT status FROM bot_star_events WHERE charge_id = $1 AND event = $2",
    )
    .bind(&event.telegram_payment_charge_id)
    .bind(event_name(event.event))
    .fetch_optional(db.conn())
    .await?;
    stored.as_deref().map(Status::parse).transpose()
}

pub async fn status(services: &Services, event: &StarsPayment) -> StoreResult<Option<Status>> {
    let mut conn = services.store().conn().await?;
    stored_status(&mut conn, event).await
}

async fn waits_for_payment(db: &mut impl Db, charge: &str) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM bot_star_events \
         WHERE charge_id = $1 AND event = 'successful_payment' AND status = 'pending')",
    )
    .bind(charge)
    .fetch_one(db.conn())
    .await?)
}

async fn prepare(services: &Services, event: &StarsPayment) -> StoreResult<Prepared> {
    let mut conn = services.store().conn().await?;
    insert(&mut conn, event, Source::Update, later(services, GRACE)).await?;
    let status = stored_status(&mut conn, event)
        .await?
        .unwrap_or(Status::Pending);
    if status != Status::Pending {
        return Ok(Prepared::Finished(status));
    }
    if event.event == StarsPaymentEvent::RefundedPayment
        && waits_for_payment(&mut conn, &event.telegram_payment_charge_id).await?
    {
        return Ok(Prepared::Blocked);
    }
    Ok(Prepared::Ready)
}

pub async fn settle(services: &Services, event: &StarsPayment) -> Settled {
    let kept = storable(event);
    if kept {
        match prepare(services, event).await {
            Ok(Prepared::Ready) => {}
            Ok(Prepared::Finished(Status::Done)) => return Settled::Done,
            Ok(Prepared::Finished(status)) => {
                return Settled::Failed(format!(
                    "оплата уже разобрана со статусом {}",
                    status.as_str()
                ));
            }
            Ok(Prepared::Blocked) => {
                return Settled::Waiting("возврат ждёт, пока проведётся оплата".into());
            }
            Err(err) => {
                tracing::warn!(
                    charge = %event.telegram_payment_charge_id,
                    error = %err,
                    "оплата звёздами не записана перед проведением"
                );
            }
        }
    }
    let verdict = process(services, event, QUICK_RETRIES).await;
    if kept && let Err(err) = finish(services, event, &verdict, Duration::ZERO).await {
        tracing::warn!(
            charge = %event.telegram_payment_charge_id,
            error = %err,
            "итог оплаты звёздами не сохранён, повторим"
        );
    }
    match verdict {
        Verdict::Applied { .. } => Settled::Done,
        Verdict::Repeat(_) => Settled::Repeat,
        Verdict::Retry(error) => Settled::Waiting(error),
        Verdict::Skip(error) | Verdict::Review { error, .. } => Settled::Failed(error),
    }
}

async fn claim(services: &Services) -> StoreResult<Vec<(String, String, serde_json::Value, i32)>> {
    let mut conn = services.store().conn().await?;
    Ok(sqlx::query_as(
        "UPDATE bot_star_events SET next_attempt_at = $2 \
         WHERE (charge_id, event) IN ( \
             SELECT e.charge_id, e.event FROM bot_star_events e \
             WHERE e.status = 'pending' AND e.next_attempt_at <= $1 \
               AND NOT (e.event = 'refunded_payment' AND EXISTS ( \
                   SELECT 1 FROM bot_star_events s \
                   WHERE s.charge_id = e.charge_id AND s.event = 'successful_payment' \
                     AND s.status = 'pending')) \
             ORDER BY e.next_attempt_at \
             LIMIT $3 \
             FOR UPDATE OF e SKIP LOCKED) \
         RETURNING charge_id, event, payload, attempts",
    )
    .bind(services.now())
    .bind(later(services, LEASE))
    .bind(BATCH)
    .fetch_all(conn.conn())
    .await?)
}

async fn quarantine(
    services: &Services,
    charge: &str,
    event: &str,
    error: &str,
) -> StoreResult<()> {
    let mut conn = services.store().conn().await?;
    sqlx::query(
        "UPDATE bot_star_events SET status = 'review', last_error = $3 \
         WHERE charge_id = $1 AND event = $2 AND status = 'pending'",
    )
    .bind(charge)
    .bind(event)
    .bind(error)
    .execute(conn.conn())
    .await?;
    tracing::error!(charge = %charge, event = %event, error = %error, "сохранённая оплата звёздами не разобрана");
    Ok(())
}

pub async fn retry_due(services: &Services, base: Duration) -> StoreResult<Retried> {
    let mut report = Retried::default();
    for _ in 0..BATCHES_PER_TICK {
        let claimed = claim(services).await?;
        let full = claimed.len() as i64 >= BATCH;
        for (charge, name, payload, attempts) in claimed {
            report.taken += 1;
            let event: StarsPayment = match serde_json::from_value(payload) {
                Ok(event) => event,
                Err(err) => {
                    quarantine(services, &charge, &name, &err.to_string()).await?;
                    continue;
                }
            };
            let verdict = process(services, &event, 0).await;
            if matches!(verdict, Verdict::Applied { .. } | Verdict::Repeat(_)) {
                report.applied += 1;
            }
            let delay = retry_delay(base, attempts.saturating_add(1));
            finish(services, &event, &verdict, delay).await?;
        }
        if !full {
            break;
        }
    }
    Ok(report)
}

async fn paid_with(db: &mut impl Db, charge: &str) -> StoreResult<Option<Payment>> {
    if let Some(payment) = db
        .payments()
        .by_provider_txid(providers::STARS, charge)
        .await?
    {
        return Ok(Some(payment));
    }
    let stored: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM bot_star_events WHERE charge_id = $1 AND event = 'successful_payment'",
    )
    .bind(charge)
    .fetch_optional(db.conn())
    .await?;
    match stored.and_then(|value| serde_json::from_value::<StarsPayment>(value).ok()) {
        Some(paid) => ours(db, &paid.invoice_payload).await,
        None => Ok(None),
    }
}

async fn wanted(
    db: &mut impl Db,
    transaction: &StarTransaction,
) -> StoreResult<Option<StarsPayment>> {
    if let Some(event) = transaction.payment_event() {
        let Some(payment) = ours(db, &event.invoice_payload).await? else {
            return Ok(None);
        };
        let applied = payment.status.is_paid_like()
            && payment.provider_txid.as_deref() == Some(transaction.id.as_str());
        return Ok((!applied).then_some(event));
    }
    if !transaction.is_refund() {
        return Ok(None);
    }
    let named = match transaction.invoice_payload.as_deref() {
        Some(payload) => ours(db, payload).await?,
        None => None,
    };
    let payment = match named {
        Some(payment) => Some(payment),
        None => paid_with(db, &transaction.id).await?,
    };
    Ok(payment
        .filter(|p| p.status != PaymentStatus::Refunded)
        .and_then(|p| transaction.refund_event(&p.public_id.to_string())))
}

pub async fn reconcile(
    services: &Services,
    stars: &TelegramStars,
    config: &ProviderConfig,
    bot_id: i64,
) -> Result<Reconciled, ServiceError> {
    let key = offset_key(bot_id);
    let since = services
        .store()
        .installed_at()
        .await?
        .map_or(0, |at| at.timestamp());
    let stored = {
        let mut conn = services.store().conn().await?;
        conn.bot_state()
            .get(&key)
            .await?
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
    };
    let mut cursor = match stored {
        Some(cursor) => cursor,
        None => {
            let start = stars.first_since(config, since).await?;
            let mut conn = services.store().conn().await?;
            conn.bot_state()
                .set(&key, &serde_json::json!(start))
                .await?;
            if start > 0 {
                tracing::info!(
                    skipped = start,
                    "сверка звёзд начинается с момента установки панели, более ранние транзакции бота не читаются"
                );
            }
            start
        }
    };
    let mut report = Reconciled::default();
    for _ in 0..PAGES_PER_RUN {
        let page = stars.star_transactions(config, cursor, PAGE).await?;
        if page.is_empty() {
            return Ok(report);
        }
        let read = u32::try_from(page.len()).unwrap_or(PAGE);
        let due = services.now();
        let mut tx = services.store().begin().await?;
        for transaction in page.iter().filter(|t| t.date >= since) {
            if let Some(event) = wanted(&mut tx, transaction).await?
                && insert(&mut tx, &event, Source::Reconcile, due).await?
            {
                report.recorded += 1;
            }
        }
        cursor = cursor.saturating_add(read);
        tx.bot_state().set(&key, &serde_json::json!(cursor)).await?;
        tx.commit().await?;
        report.read += read;
        if read < PAGE {
            return Ok(report);
        }
    }
    report.more = true;
    Ok(report)
}

pub struct Keeper {
    services: Services,
    stars: TelegramStars,
    config: ProviderConfig,
    bot_id: i64,
    retry_every: Duration,
    reconcile_every: Duration,
}

impl Keeper {
    pub fn new(
        services: Services,
        api_base: &str,
        token: &str,
        bot_id: i64,
        retry_every: Duration,
        reconcile_every: Duration,
    ) -> Self {
        let mut config = ProviderConfig::default();
        config.set(
            STARS_TOKEN_FIELD,
            serde_json::Value::String(token.to_string()),
        );
        Keeper {
            services,
            stars: TelegramStars::with_api_base(api_base),
            config,
            bot_id,
            retry_every: retry_every.max(Duration::from_millis(10)),
            reconcile_every,
        }
    }

    async fn reconcile_once(&self) -> bool {
        match reconcile(&self.services, &self.stars, &self.config, self.bot_id).await {
            Ok(report) => {
                if report.recorded > 0 {
                    tracing::info!(
                        read = report.read,
                        recorded = report.recorded,
                        "сверка звёзд нашла непроведённые оплаты или возвраты"
                    );
                }
                report.more
            }
            Err(err) => {
                tracing::warn!(error = %err, "сверка транзакций звёзд не удалась");
                false
            }
        }
    }

    pub async fn run(self, cancel: CancellationToken) {
        let mut next_reconcile = tokio::time::Instant::now();
        loop {
            if cancel.is_cancelled() {
                return;
            }
            if tokio::time::Instant::now() >= next_reconcile {
                let more = self.reconcile_once().await;
                next_reconcile = tokio::time::Instant::now()
                    + if more {
                        Duration::ZERO
                    } else {
                        self.reconcile_every
                    };
            }
            match retry_due(&self.services, self.retry_every).await {
                Ok(report) if report.taken > 0 => tracing::info!(
                    taken = report.taken,
                    applied = report.applied,
                    "оплаты звёздами проведены повторно"
                ),
                Ok(_) => {}
                Err(err) => tracing::warn!(error = %err, "повтор оплат звёздами не удался"),
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(self.retry_every) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn пауза_между_повторами_растёт_до_получаса() {
        let base = Duration::from_secs(30);
        assert_eq!(retry_delay(base, 0), base);
        assert_eq!(retry_delay(base, 1), base);
        assert_eq!(retry_delay(base, 2), Duration::from_secs(60));
        assert_eq!(retry_delay(base, 6), Duration::from_secs(960));
        assert_eq!(retry_delay(base, 7), MAX_DELAY);
        assert_eq!(retry_delay(base, i32::MAX), MAX_DELAY);
        assert_eq!(
            retry_delay(Duration::from_millis(100), 3),
            Duration::from_millis(400)
        );
    }

    #[test]
    fn оплаты_и_возвраты_звёзд_извлекаются_из_апдейтов() {
        let updates: Vec<Update> = serde_json::from_value(serde_json::json!([
            {"update_id": 1, "message": {
                "message_id": 1, "from": {"id": 42, "first_name": "А"},
                "chat": {"id": 42, "type": "private"}, "date": 1,
                "successful_payment": {"currency": "XTR", "total_amount": 350,
                    "invoice_payload": "p-1", "telegram_payment_charge_id": "c-1"}}},
            {"update_id": 2, "message": {
                "message_id": 2, "chat": {"id": 43, "type": "private"}, "date": 1,
                "refunded_payment": {"currency": "XTR", "total_amount": 150,
                    "invoice_payload": "p-2", "telegram_payment_charge_id": "c-2"}}},
            {"update_id": 3, "message": {
                "message_id": 3, "from": {"id": 44, "first_name": "Б"},
                "chat": {"id": 44, "type": "private"}, "date": 1, "text": "/start"}},
            {"update_id": 4, "callback_query": {
                "id": "cb", "from": {"id": 45, "first_name": "В"}, "data": "m"}}
        ]))
        .unwrap();
        let events = from_updates(&updates);
        assert_eq!(
            events,
            vec![
                StarsPayment {
                    event: StarsPaymentEvent::SuccessfulPayment,
                    invoice_payload: "p-1".into(),
                    total_amount: 350,
                    currency: "XTR".into(),
                    telegram_payment_charge_id: "c-1".into(),
                    telegram_user_id: 42,
                },
                StarsPayment {
                    event: StarsPaymentEvent::RefundedPayment,
                    invoice_payload: "p-2".into(),
                    total_amount: 150,
                    currency: "XTR".into(),
                    telegram_payment_charge_id: "c-2".into(),
                    telegram_user_id: 43,
                },
            ]
        );
        assert!(events.iter().all(storable));
        assert_eq!(offset_key(7_001), "telegram.stars_offset.7001");
        assert_eq!(event_name(events[1].event), "refunded_payment");
        for status in [
            Status::Pending,
            Status::Done,
            Status::Skipped,
            Status::Review,
            Status::Refunded,
            Status::Closed,
        ] {
            assert_eq!(Status::parse(status.as_str()).unwrap(), status);
        }
        assert!(Status::parse("lost").is_err());
    }
}
