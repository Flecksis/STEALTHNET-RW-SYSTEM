mod routes;
mod store;
#[cfg(test)]
mod tests;

use crate::catalog::web::ClientAddr;
use crate::catalog::{db_now, installation_currency};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{BotEntry, Module, QuoteHook, QuoteOrigin, ScheduledTask};
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::client::Locale;
use sn_domain::events::DomainEvent;
use sn_domain::ids::{AdminId, ClientId, OrderId, SubscriptionId, TariffId};
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money, PayCurrency, parse_units};
use sn_domain::order::{Order, PurchaseIntent};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{Payment, PaymentStatus};
use sn_domain::pricing::{AdjustmentKind, AdjustmentSource, PriceAdjustment, Quote, QuoteLineKind};
use sn_domain::{format, lang};
use sn_store::{Db, LockSpace, StoreError, Tx};
use std::future::Future;
use std::net::IpAddr;
use tracing::Instrument;

pub use store::ListFilter;

pub const BOT_ENTRY: &str = "promo";
pub const SETTLE_TASK: &str = "settle";
pub const ACTIVATION_HOLD_MINUTES: i64 = 15;
pub const MAX_DAYS: u32 = 3_650;
pub const FAILURE_LIMIT: u32 = 10;
pub const ADDRESS_FAILURE_LIMIT: u32 = 30;
pub const FAILURE_WINDOW_MINUTES: i64 = 15;

const UNKNOWN_CODE: &str = "promo.not_found";
const UNKNOWN_MESSAGE: &str = "Такого промокода нет или он не подходит";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromoKind {
    Percent { percent: u8 },
    Fixed { amount: Money },
    Days { days: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromoCode {
    pub id: i64,
    pub code: String,
    pub kind: PromoKind,
    pub max_uses: Option<u32>,
    pub per_client: u32,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub first_purchase_only: bool,
    pub tariff_ids: Vec<TariffId>,
    pub active: bool,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl PromoCode {
    pub fn shown_label(&self) -> String {
        let locale = lang::current();
        match (locale, self.kind) {
            (Locale::Ru, _) => self.label(),
            (_, PromoKind::Percent { percent }) => {
                format!("Promo code {}, −{percent}%", self.code)
            }
            (_, PromoKind::Fixed { amount }) => format!(
                "Promo code {}, −{}",
                self.code,
                format::money_in(amount, locale)
            ),
            (_, PromoKind::Days { days }) => format!(
                "Promo code {}, +{}",
                self.code,
                format::days_in(days, locale)
            ),
        }
    }

    pub fn label(&self) -> String {
        match self.kind {
            PromoKind::Percent { percent } => format!("Промокод {}, −{percent}%", self.code),
            PromoKind::Fixed { amount } => {
                format!("Промокод {}, −{}", self.code, format::money(amount))
            }
            PromoKind::Days { days } => format!("Промокод {}, +{}", self.code, format::days(days)),
        }
    }

    pub fn adjustment(&self) -> Option<PriceAdjustment> {
        let kind = match self.kind {
            PromoKind::Percent { percent } => AdjustmentKind::Percent { percent },
            PromoKind::Fixed { amount } => AdjustmentKind::Fixed { amount },
            PromoKind::Days { .. } => return None,
        };
        Some(PriceAdjustment {
            source: AdjustmentSource::Promo {
                code: self.code.clone(),
                promo_id: Some(self.id),
            },
            kind,
            label: self.label(),
        })
    }

    pub const fn is_discount(&self) -> bool {
        !matches!(self.kind, PromoKind::Days { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromoListItem {
    #[serde(flatten)]
    pub code: PromoCode,
    pub used: u32,
    pub reserved: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromoRejection {
    NotFound,
    Disabled,
    NotStarted,
    Expired,
    Exhausted,
    AlreadyUsed,
    FirstPurchaseOnly,
    WrongTariff,
    WrongCurrency,
    DaysCode,
    DiscountCode,
    NoSubscription,
    Reserved,
    TooManyAttempts,
    Protected,
}

impl PromoRejection {
    pub const fn counts_as_failure(self) -> bool {
        matches!(
            self,
            PromoRejection::NotFound
                | PromoRejection::Disabled
                | PromoRejection::NotStarted
                | PromoRejection::Expired
                | PromoRejection::Exhausted
                | PromoRejection::FirstPurchaseOnly
                | PromoRejection::WrongTariff
                | PromoRejection::WrongCurrency
        )
    }

    pub const fn code(self) -> &'static str {
        match self {
            PromoRejection::AlreadyUsed => "promo.already_used",
            PromoRejection::DaysCode => "promo.days_code",
            PromoRejection::DiscountCode => "promo.discount_code",
            PromoRejection::NoSubscription => "promo.no_subscription",
            PromoRejection::Reserved => "promo.reserved",
            PromoRejection::TooManyAttempts => "promo.rate_limited",
            PromoRejection::Protected => "promo.protected",
            PromoRejection::NotFound
            | PromoRejection::Disabled
            | PromoRejection::NotStarted
            | PromoRejection::Expired
            | PromoRejection::Exhausted
            | PromoRejection::FirstPurchaseOnly
            | PromoRejection::WrongTariff
            | PromoRejection::WrongCurrency => UNKNOWN_CODE,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            PromoRejection::AlreadyUsed => lang::pick(
                "Вы уже использовали этот промокод",
                "You've already used this promo code",
            ),
            PromoRejection::DaysCode => lang::pick(
                "Этот промокод добавляет дни к подписке: активируйте его в разделе «Промокод»",
                "This promo code adds days to a subscription. Activate it under “Promo code”",
            ),
            PromoRejection::DiscountCode => lang::pick(
                "Это промокод на скидку: введите его при оплате",
                "This is a discount code. Enter it at checkout",
            ),
            PromoRejection::NoSubscription => lang::pick(
                "Промокод добавляет дни к подписке, а подходящей подписки нет",
                "This promo code adds days to a subscription, but you don't have a suitable one",
            ),
            PromoRejection::Reserved => lang::pick(
                "Промокод уже применён к неоплаченному счёту. Оплатите его или подождите, пока счёт истечёт",
                "This promo code is already on an unpaid invoice. Pay it or wait until it expires",
            ),
            PromoRejection::TooManyAttempts => lang::pick(
                "Слишком много неверных промокодов. Попробуйте снова через 15 минут",
                "Too many wrong promo codes. Try again in 15 minutes",
            ),
            PromoRejection::Protected => lang::pick(
                "Промокоды на дни недоступны для этого аккаунта. Если это ошибка, напишите в поддержку",
                "Promo codes for days aren't available for this account. If this is a mistake, contact support",
            ),
            PromoRejection::NotFound
            | PromoRejection::Disabled
            | PromoRejection::NotStarted
            | PromoRejection::Expired
            | PromoRejection::Exhausted
            | PromoRejection::FirstPurchaseOnly
            | PromoRejection::WrongTariff
            | PromoRejection::WrongCurrency => lang::pick(
                UNKNOWN_MESSAGE,
                "This promo code doesn't exist or doesn't apply here",
            ),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PromoError {
    #[error("{}", .0.message())]
    Rejected(PromoRejection),
    #[error(transparent)]
    Module(#[from] ModuleError),
}

impl From<StoreError> for PromoError {
    fn from(err: StoreError) -> Self {
        PromoError::Module(ModuleError::Store(err))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Checkout {
        tariff: TariffId,
        currency: Currency,
    },
    Activate,
    Check {
        tariff: Option<TariffId>,
        currency: Currency,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub active_total: u32,
    pub client_active: u32,
    pub client_held: u32,
    pub client_has_purchases: bool,
}

pub fn normalize_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    let valid = (3..=32).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    valid.then_some(code)
}

fn validity(code: &PromoCode, now: DateTime<Utc>) -> Result<(), PromoRejection> {
    if !code.active {
        return Err(PromoRejection::Disabled);
    }
    if code.valid_from.is_some_and(|from| from > now) {
        return Err(PromoRejection::NotStarted);
    }
    if code.valid_until.is_some_and(|until| until <= now) {
        return Err(PromoRejection::Expired);
    }
    Ok(())
}

fn applicability(code: &PromoCode, purpose: Purpose) -> Result<(), PromoRejection> {
    let (tariff, currency) = match purpose {
        Purpose::Checkout { tariff, currency } => (Some(tariff), Some(currency)),
        Purpose::Check { tariff, currency } => (tariff, Some(currency)),
        Purpose::Activate => (None, None),
    };
    if let Some(tariff) = tariff
        && !code.tariff_ids.is_empty()
        && !code.tariff_ids.contains(&tariff)
    {
        return Err(PromoRejection::WrongTariff);
    }
    if let (PromoKind::Fixed { amount }, Some(currency)) = (code.kind, currency)
        && amount.currency != currency
    {
        return Err(PromoRejection::WrongCurrency);
    }
    Ok(())
}

pub fn limits(code: &PromoCode, usage: &Usage) -> Result<(), PromoRejection> {
    let own = if usage.client_held > 0 {
        PromoRejection::Reserved
    } else {
        PromoRejection::AlreadyUsed
    };
    if usage.client_active >= code.per_client {
        return Err(own);
    }
    if code.first_purchase_only && usage.client_has_purchases {
        return Err(if usage.client_held > 0 {
            own
        } else {
            PromoRejection::FirstPurchaseOnly
        });
    }
    if code.max_uses.is_some_and(|max| usage.active_total >= max) {
        return Err(PromoRejection::Exhausted);
    }
    Ok(())
}

fn kind_fits(code: &PromoCode, purpose: Purpose) -> Result<(), PromoRejection> {
    match (purpose, code.kind) {
        (Purpose::Checkout { .. }, PromoKind::Days { .. }) => Err(PromoRejection::DaysCode),
        (Purpose::Activate, PromoKind::Percent { .. } | PromoKind::Fixed { .. }) => {
            Err(PromoRejection::DiscountCode)
        }
        _ => Ok(()),
    }
}

pub fn evaluate(
    code: &PromoCode,
    usage: &Usage,
    now: DateTime<Utc>,
    purpose: Purpose,
) -> Result<(), PromoRejection> {
    validity(code, now)?;
    applicability(code, purpose)?;
    limits(code, usage)?;
    kind_fits(code, purpose)
}

fn attempt_window(now: DateTime<Utc>) -> DateTime<Utc> {
    let seconds = FAILURE_WINDOW_MINUTES * 60;
    let start = now.timestamp().div_euclid(seconds) * seconds;
    DateTime::from_timestamp(start, 0).unwrap_or(now)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Attempt {
    client_id: ClientId,
    network: Option<String>,
    window: DateTime<Utc>,
}

async fn admit(
    db: &mut impl Db,
    client_id: ClientId,
    address: Option<IpAddr>,
    now: DateTime<Utc>,
) -> Result<Attempt, PromoError> {
    let network = match ClientAddr(address).limit_network() {
        Some(network) if !store::paid_before(db, client_id).await? => Some(network),
        _ => None,
    };
    let attempt = Attempt {
        client_id,
        network,
        window: attempt_window(now),
    };
    let forget_before = now - Duration::days(1);
    if store::strike_client(db, client_id, attempt.window, forget_before).await? > FAILURE_LIMIT {
        store::forgive_client(db, client_id, attempt.window).await?;
        return Err(PromoError::Rejected(PromoRejection::TooManyAttempts));
    }
    if let Some(network) = &attempt.network
        && store::strike_address(db, network, attempt.window, forget_before).await?
            > ADDRESS_FAILURE_LIMIT
    {
        store::forgive_address(db, network, attempt.window).await?;
        store::forgive_client(db, client_id, attempt.window).await?;
        return Err(PromoError::Rejected(PromoRejection::TooManyAttempts));
    }
    Ok(attempt)
}

async fn forgive(db: &mut impl Db, attempt: &Attempt) -> Result<(), StoreError> {
    store::forgive_client(db, attempt.client_id, attempt.window).await?;
    if let Some(network) = &attempt.network {
        store::forgive_address(db, network, attempt.window).await?;
    }
    Ok(())
}

fn counted<T>(result: &Result<T, PromoError>) -> bool {
    matches!(result, Err(PromoError::Rejected(rejection)) if rejection.counts_as_failure())
}

fn left_counted(attempt: &Attempt, err: &StoreError) {
    tracing::warn!(
        client = %attempt.client_id,
        error = %err,
        "попытка ввода промокода осталась засчитанной"
    );
}

async fn detached<T, F>(work: F) -> Result<T, PromoError>
where
    F: Future<Output = Result<T, PromoError>> + Send + 'static,
    T: Send + 'static,
{
    match tokio::spawn(work.in_current_span()).await {
        Ok(result) => result,
        Err(err) => match err.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            Err(err) => Err(PromoError::Module(ModuleError::Core(format!(
                "операция с промокодом прервана: {err}"
            )))),
        },
    }
}

fn rejected(rejection: PromoRejection) -> ModuleError {
    ModuleError::Invalid(rejection.message().to_string())
}

pub fn promo_in_quote(quote: &Quote) -> Option<(i64, String)> {
    quote.lines.iter().find_map(|line| match &line.kind {
        QuoteLineKind::Adjustment {
            source:
                AdjustmentSource::Promo {
                    code,
                    promo_id: Some(id),
                },
        } => Some((*id, code.clone())),
        _ => None,
    })
}

fn order_tariff(order: &Order) -> Option<TariffId> {
    match &order.intent {
        PurchaseIntent::Subscription { tariff_id, .. } => Some(*tariff_id),
        _ => None,
    }
}

const fn moves_to_new_invoice(purpose: Purpose) -> bool {
    matches!(purpose, Purpose::Checkout { .. } | Purpose::Check { .. })
}

async fn lookup(
    db: &mut impl Db,
    client_id: ClientId,
    code: &str,
    purpose: Purpose,
    now: DateTime<Utc>,
) -> Result<PromoCode, PromoError> {
    let promo = store::by_code(db, code, false)
        .await?
        .ok_or(PromoError::Rejected(PromoRejection::NotFound))?;
    let usage = store::usage(db, promo.id, client_id, now, &[]).await?;
    let Err(rejection) = evaluate(&promo, &usage, now, purpose) else {
        return Ok(promo);
    };
    let unpaid = if moves_to_new_invoice(purpose) {
        store::unpaid_holds(db, promo.id, client_id, now, None).await?
    } else {
        Vec::new()
    };
    if unpaid.is_empty() {
        return Err(PromoError::Rejected(rejection));
    }
    let usage = store::usage(db, promo.id, client_id, now, &unpaid).await?;
    evaluate(&promo, &usage, now, purpose).map_err(PromoError::Rejected)?;
    Ok(promo)
}

pub async fn superseded(tx: &mut Tx, order: &Order) -> Result<Vec<OrderId>, ModuleError> {
    let Some((promo_id, _)) = promo_in_quote(&order.quote) else {
        return Ok(Vec::new());
    };
    tx.lock(LockSpace::Client, order.client_id.0).await?;
    let Some(promo) = store::lock(tx, promo_id).await? else {
        return Ok(Vec::new());
    };
    if store::redemption_for_order(tx, order.id).await?.is_some() {
        return Ok(Vec::new());
    }
    let now = db_now(tx).await?;
    let purpose = order_purpose(order);
    let mut skipped = vec![order.id];
    let usage = store::usage(tx, promo.id, order.client_id, now, &skipped).await?;
    if evaluate(&promo, &usage, now, purpose).is_ok() {
        return Ok(Vec::new());
    }
    for unpaid in store::unpaid_holds(tx, promo.id, order.client_id, now, Some(order.id)).await? {
        skipped.push(unpaid);
        let usage = store::usage(tx, promo.id, order.client_id, now, &skipped).await?;
        if evaluate(&promo, &usage, now, purpose).is_ok() {
            return Ok(skipped.split_off(1));
        }
    }
    Ok(Vec::new())
}

pub async fn check(
    ctx: &ModuleContext,
    client_id: ClientId,
    raw: &str,
    purpose: Purpose,
) -> Result<PromoCode, PromoError> {
    check_from(ctx, client_id, None, raw, purpose).await
}

pub async fn check_from(
    ctx: &ModuleContext,
    client_id: ClientId,
    address: Option<IpAddr>,
    raw: &str,
    purpose: Purpose,
) -> Result<PromoCode, PromoError> {
    let code = normalize_code(raw).ok_or(PromoError::Rejected(PromoRejection::NotFound))?;
    let ctx = ctx.clone();
    detached(async move { check_now(&ctx, client_id, address, &code, purpose).await }).await
}

async fn check_now(
    ctx: &ModuleContext,
    client_id: ClientId,
    address: Option<IpAddr>,
    code: &str,
    purpose: Purpose,
) -> Result<PromoCode, PromoError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    let attempt = admit(&mut conn, client_id, address, now).await?;
    let result = lookup(&mut conn, client_id, code, purpose, now).await;
    if !counted(&result)
        && let Err(err) = forgive(&mut conn, &attempt).await
    {
        left_counted(&attempt, &err);
    }
    result
}

pub async fn settle(ctx: &ModuleContext, limit: i64) -> Result<usize, ModuleError> {
    let orders = {
        let mut conn = ctx.store.conn().await?;
        store::paid_orders_without_use(&mut conn, limit).await?
    };
    for order_id in &orders {
        on_paid(ctx, *order_id).await?;
    }
    Ok(orders.len())
}

fn order_purpose(order: &Order) -> Purpose {
    match order_tariff(order) {
        Some(tariff) => Purpose::Checkout {
            tariff,
            currency: order.quote.currency,
        },
        None => Purpose::Check {
            tariff: None,
            currency: order.quote.currency,
        },
    }
}

async fn hold(
    tx: &mut Tx,
    order: &Order,
    until: Option<DateTime<Utc>>,
) -> Result<Option<i64>, ModuleError> {
    let Some((promo_id, _)) = promo_in_quote(&order.quote) else {
        return Ok(None);
    };
    tx.lock(LockSpace::Client, order.client_id.0).await?;
    let promo = store::lock(tx, promo_id)
        .await?
        .ok_or(ModuleError::NotFound("промокод"))?;
    if let Some(existing) = store::redemption_for_order(tx, order.id).await? {
        return Ok(Some(existing));
    }
    let now = db_now(tx).await?;
    let usage = store::usage(tx, promo.id, order.client_id, now, &[order.id]).await?;
    evaluate(&promo, &usage, now, order_purpose(order)).map_err(rejected)?;
    Ok(store::insert_redemption(
        tx,
        &store::NewRedemption {
            promo_id: promo.id,
            client_id: order.client_id,
            order_id: Some(order.id),
            subscription_id: None,
            status: store::RedemptionStatus::Reserved,
            reserved_until: until,
        },
    )
    .await?)
}

async fn confirm(tx: &mut Tx, order: &Order, payment: &Payment) -> Result<(), ModuleError> {
    let Some((promo_id, _)) = promo_in_quote(&order.quote) else {
        return Ok(());
    };
    tx.lock(LockSpace::Client, order.client_id.0).await?;
    let promo = store::lock(tx, promo_id)
        .await?
        .ok_or(ModuleError::NotFound("промокод"))?;
    let existing = store::redemption_state(tx, order.id).await?;
    let now = db_now(tx).await?;
    if let Some(held) = &existing {
        if held.status == store::RedemptionStatus::Used {
            return Ok(());
        }
        let live = held.status == store::RedemptionStatus::Reserved
            && (held.reserved_until.is_none_or(|until| until > now)
                || payment.expires_at.is_none_or(|expires| expires > now));
        if live {
            store::set_status(tx, held.id, store::RedemptionStatus::Used).await?;
            return Ok(());
        }
    }
    tracing::info!(
        order = %order.id,
        code = %promo.code,
        "оплата пришла после брони промокода: заказ выдаётся по цене счёта"
    );
    match existing {
        Some(held) => store::set_status(tx, held.id, store::RedemptionStatus::Used).await?,
        None => {
            store::insert_redemption(
                tx,
                &store::NewRedemption {
                    promo_id: promo.id,
                    client_id: order.client_id,
                    order_id: Some(order.id),
                    subscription_id: None,
                    status: store::RedemptionStatus::Used,
                    reserved_until: None,
                },
            )
            .await?;
        }
    }
    Ok(())
}

pub async fn reserve(
    ctx: &ModuleContext,
    order: &Order,
    until: DateTime<Utc>,
) -> Result<Option<i64>, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let id = hold(&mut tx, order, Some(until)).await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn release(ctx: &ModuleContext, order_id: OrderId) -> Result<bool, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    Ok(store::release_order(&mut conn, order_id, store::RedemptionStatus::Reserved).await? > 0)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Activation {
    pub code: String,
    pub days: u32,
    pub subscription_id: SubscriptionId,
}

async fn hold_days(
    ctx: &ModuleContext,
    client_id: ClientId,
    code: &str,
    subscription_id: SubscriptionId,
) -> Result<(PromoCode, u32, i64), PromoError> {
    let mut tx = ctx.store.begin().await?;
    let subscription = match tx.subscriptions().get(subscription_id).await {
        Ok(subscription) => subscription,
        Err(StoreError::NotFound(_)) => {
            return Err(PromoError::Rejected(PromoRejection::NoSubscription));
        }
        Err(err) => return Err(err.into()),
    };
    if subscription.client_id != client_id
        || subscription.ended_at.is_some()
        || subscription.remna.is_none()
    {
        return Err(PromoError::Rejected(PromoRejection::NoSubscription));
    }
    let promo = store::by_code(&mut tx, code, true)
        .await?
        .ok_or(PromoError::Rejected(PromoRejection::NotFound))?;
    let now = db_now(&mut tx).await?;
    let usage = store::usage(&mut tx, promo.id, client_id, now, &[]).await?;
    evaluate(&promo, &usage, now, Purpose::Activate).map_err(PromoError::Rejected)?;
    let PromoKind::Days { days } = promo.kind else {
        return Err(PromoError::Rejected(PromoRejection::DiscountCode));
    };
    let redemption = store::insert_redemption(
        &mut tx,
        &store::NewRedemption {
            promo_id: promo.id,
            client_id,
            order_id: None,
            subscription_id: Some(subscription_id),
            status: store::RedemptionStatus::Reserved,
            reserved_until: Some(now + Duration::minutes(ACTIVATION_HOLD_MINUTES)),
        },
    )
    .await?
    .ok_or(ModuleError::Core("промокод не зарезервирован".into()))?;
    tx.commit().await?;
    Ok((promo, days, redemption))
}

pub async fn activate(
    ctx: &ModuleContext,
    client_id: ClientId,
    raw: &str,
    subscription_id: SubscriptionId,
) -> Result<Activation, PromoError> {
    activate_from(ctx, client_id, None, raw, subscription_id).await
}

pub async fn activate_from(
    ctx: &ModuleContext,
    client_id: ClientId,
    address: Option<IpAddr>,
    raw: &str,
    subscription_id: SubscriptionId,
) -> Result<Activation, PromoError> {
    let code = normalize_code(raw).ok_or(PromoError::Rejected(PromoRejection::NotFound))?;
    let ctx = ctx.clone();
    detached(async move { activate_now(&ctx, client_id, address, &code, subscription_id).await })
        .await
}

async fn activate_now(
    ctx: &ModuleContext,
    client_id: ClientId,
    address: Option<IpAddr>,
    code: &str,
    subscription_id: SubscriptionId,
) -> Result<Activation, PromoError> {
    if !crate::catalog::antibot::promo_allowed(ctx, client_id).await? {
        return Err(PromoError::Rejected(PromoRejection::Protected));
    }
    let attempt = {
        let mut conn = ctx.store.conn().await?;
        let now = db_now(&mut conn).await?;
        admit(&mut conn, client_id, address, now).await?
    };
    let held = hold_days(ctx, client_id, code, subscription_id).await;
    if !counted(&held) {
        let forgiven = match ctx.store.conn().await {
            Ok(mut conn) => forgive(&mut conn, &attempt).await,
            Err(err) => Err(err),
        };
        if let Err(err) = forgiven {
            left_counted(&attempt, &err);
        }
    }
    let (promo, days, redemption) = held?;
    let applied = ctx
        .core
        .add_days(subscription_id, days, &format!("промокод {}", promo.code))
        .await;
    let mut conn = ctx.store.conn().await?;
    match applied {
        Ok(()) => {
            store::set_status(&mut conn, redemption, store::RedemptionStatus::Used).await?;
            Ok(Activation {
                code: promo.code,
                days,
                subscription_id,
            })
        }
        Err(err) => {
            store::set_status(&mut conn, redemption, store::RedemptionStatus::Released).await?;
            Err(PromoError::Module(err))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NewPromoValue {
    Percent { percent: u8 },
    Fixed { amount: String },
    Days { days: u32 },
}

fn default_per_client() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NewPromo {
    pub code: String,
    #[serde(flatten)]
    pub value: NewPromoValue,
    #[serde(default)]
    pub max_uses: Option<u32>,
    #[serde(default = "default_per_client")]
    pub per_client: u32,
    #[serde(default)]
    pub valid_from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub valid_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub first_purchase_only: bool,
    #[serde(default)]
    pub tariff_ids: Vec<TariffId>,
    #[serde(default)]
    pub note: Option<String>,
}

fn invalid(message: impl Into<String>) -> ModuleError {
    ModuleError::Invalid(message.into())
}

pub fn validate_new(
    new: &NewPromo,
    currency: Currency,
) -> Result<(String, PromoKind), ModuleError> {
    let code = normalize_code(&new.code)
        .ok_or_else(|| invalid("код — от 3 до 32 латинских букв, цифр, «-» или «_»"))?;
    let kind = match &new.value {
        NewPromoValue::Percent { percent } if (1..=100).contains(percent) => {
            PromoKind::Percent { percent: *percent }
        }
        NewPromoValue::Percent { .. } => return Err(invalid("скидка — от 1 до 100 %")),
        NewPromoValue::Fixed { amount } => {
            let minor = parse_units(amount.trim(), PayCurrency::from(currency))
                .filter(|m| *m > 0)
                .ok_or_else(|| invalid("сумма скидки указана неверно"))?;
            if minor % currency.rounding_step() != 0 {
                return Err(invalid("в рублях скидка задаётся целыми рублями"));
            }
            PromoKind::Fixed {
                amount: Money::new(minor, currency),
            }
        }
        NewPromoValue::Days { days } if (1..=MAX_DAYS).contains(days) => {
            PromoKind::Days { days: *days }
        }
        NewPromoValue::Days { .. } => return Err(invalid("дней — от 1 до 3650")),
    };
    if new.max_uses == Some(0) {
        return Err(invalid("лимит использований должен быть больше нуля"));
    }
    if new.per_client == 0 {
        return Err(invalid("лимит на клиента должен быть больше нуля"));
    }
    if let (Some(from), Some(until)) = (new.valid_from, new.valid_until)
        && until <= from
    {
        return Err(invalid("срок окончания раньше начала"));
    }
    if new.note.as_deref().is_some_and(|n| n.chars().count() > 500) {
        return Err(invalid("заметка длиннее 500 символов"));
    }
    if matches!(kind, PromoKind::Days { .. }) && !new.tariff_ids.is_empty() {
        return Err(invalid("промокод на дни не привязывается к тарифам"));
    }
    Ok((code, kind))
}

pub async fn create(
    ctx: &ModuleContext,
    new: &NewPromo,
    by: Option<AdminId>,
) -> Result<PromoCode, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let currency = installation_currency(&mut tx).await?;
    let (code, kind) = validate_new(new, currency)?;
    if store::by_code(&mut tx, &code, false).await?.is_some() {
        return Err(invalid(format!("промокод {code} уже есть")));
    }
    if !store::tariffs_exist(&mut tx, &new.tariff_ids).await? {
        return Err(invalid("среди тарифов есть несуществующий"));
    }
    let created = store::create(&mut tx, &code, kind, new, by).await?;
    tx.commit().await?;
    Ok(created)
}

const PERSONAL_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn personal_code_text() -> String {
    use rand::Rng;
    let mut rng = rand::rngs::OsRng;
    let tail: String = (0..8)
        .map(|_| char::from(PERSONAL_ALPHABET[rng.gen_range(0..PERSONAL_ALPHABET.len())]))
        .collect();
    format!("WIN-{tail}")
}

pub async fn issue_personal(
    db: &mut impl Db,
    kind: PromoKind,
    valid_until: Option<DateTime<Utc>>,
    note: &str,
) -> Result<PromoCode, ModuleError> {
    let value = match kind {
        PromoKind::Percent { percent } => NewPromoValue::Percent { percent },
        PromoKind::Fixed { amount } => NewPromoValue::Fixed {
            amount: amount.minor.to_string(),
        },
        PromoKind::Days { days } => NewPromoValue::Days { days },
    };
    for _ in 0..8 {
        let code = personal_code_text();
        let new = NewPromo {
            code: code.clone(),
            value: value.clone(),
            max_uses: Some(1),
            per_client: 1,
            valid_from: None,
            valid_until,
            first_purchase_only: false,
            tariff_ids: Vec::new(),
            note: Some(note.chars().take(500).collect()),
        };
        if let Some(created) = store::create_if_free(db, &code, kind, &new).await? {
            return Ok(created);
        }
    }
    Err(ModuleError::Core(
        "не удалось придумать свободный промокод".into(),
    ))
}

pub async fn list(
    ctx: &ModuleContext,
    filter: ListFilter,
    page: PageRequest,
) -> Result<Page<PromoListItem>, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    Ok(store::list(&mut conn, filter, page, now).await?)
}

pub async fn set_active(
    ctx: &ModuleContext,
    id: i64,
    active: bool,
) -> Result<PromoCode, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    store::set_active(&mut conn, id, active)
        .await?
        .ok_or(ModuleError::NotFound("промокод"))
}

async fn on_paid(ctx: &ModuleContext, order_id: OrderId) -> Result<(), ModuleError> {
    let mut tx = ctx.store.begin().await?;
    if store::mark_order_used(&mut tx, order_id).await? == 0
        && store::redemption_for_order(&mut tx, order_id)
            .await?
            .is_none()
    {
        let order = tx.orders().get(order_id).await?;
        if let Some((promo_id, _)) = promo_in_quote(&order.quote) {
            store::insert_redemption(
                &mut tx,
                &store::NewRedemption {
                    promo_id,
                    client_id: order.client_id,
                    order_id: Some(order_id),
                    subscription_id: None,
                    status: store::RedemptionStatus::Used,
                    reserved_until: None,
                },
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

async fn on_refunded(
    ctx: &ModuleContext,
    payment_id: sn_domain::ids::PaymentId,
    order_id: OrderId,
) -> Result<(), ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let payment = conn.payments().get(payment_id).await?;
    if payment.status == PaymentStatus::Refunded {
        store::release_order(&mut conn, order_id, store::RedemptionStatus::Used).await?;
    }
    Ok(())
}

pub struct PromoModule;

#[async_trait]
impl Module for PromoModule {
    fn id(&self) -> ModuleId {
        ModuleId::Promo
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    fn client_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::client(ctx))
    }

    fn bot_entries(&self) -> Vec<BotEntry> {
        vec![BotEntry {
            key: BOT_ENTRY,
            label: "Промокод",
            order: 30,
        }]
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: SETTLE_TASK,
            every: std::time::Duration::from_secs(600),
        }]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        if task != SETTLE_TASK {
            return Err(ModuleError::Invalid(format!("у модуля нет задачи {task}")));
        }
        let fixed = settle(ctx, 200).await?;
        if fixed > 0 {
            tracing::info!(
                orders = fixed,
                "промокоды: использования по оплаченным заказам досведены"
            );
        }
        Ok(())
    }

    async fn on_order_created(
        &self,
        _ctx: &ModuleContext,
        tx: &mut Tx,
        order: &Order,
        payment: &Payment,
    ) -> Result<(), ModuleError> {
        hold(tx, order, payment.expires_at).await.map(|_| ())
    }

    async fn on_order_paid(
        &self,
        _ctx: &ModuleContext,
        tx: &mut Tx,
        order: &Order,
        payment: &Payment,
    ) -> Result<(), ModuleError> {
        confirm(tx, order, payment).await
    }

    async fn on_order_canceled(
        &self,
        _ctx: &ModuleContext,
        tx: &mut Tx,
        order: &Order,
    ) -> Result<(), ModuleError> {
        store::release_order(tx, order.id, store::RedemptionStatus::Reserved).await?;
        Ok(())
    }

    async fn on_event(&self, ctx: &ModuleContext, event: &DomainEvent) -> Result<(), ModuleError> {
        match event {
            DomainEvent::PaymentPaid { order_id, .. } => on_paid(ctx, *order_id).await,
            DomainEvent::PaymentRefunded {
                payment_id,
                order_id,
                ..
            } => on_refunded(ctx, *payment_id, *order_id).await,
            _ => Ok(()),
        }
    }

    async fn quote_adjustments(
        &self,
        ctx: &ModuleContext,
        hook: &QuoteHook,
    ) -> Result<Vec<PriceAdjustment>, ModuleError> {
        self.quote_adjustments_from(ctx, hook, &QuoteOrigin::default())
            .await
    }

    async fn quote_adjustments_from(
        &self,
        ctx: &ModuleContext,
        hook: &QuoteHook,
        origin: &QuoteOrigin,
    ) -> Result<Vec<PriceAdjustment>, ModuleError> {
        let Some(raw) = hook
            .promo_code
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        else {
            return Ok(Vec::new());
        };
        let purpose = Purpose::Checkout {
            tariff: hook.tariff_id,
            currency: hook.currency,
        };
        match check_from(ctx, hook.client_id, origin.address, raw, purpose).await {
            Ok(promo) => Ok(promo.adjustment().into_iter().collect()),
            Err(PromoError::Rejected(rejection)) => {
                Err(ModuleError::Invalid(rejection.message().to_string()))
            }
            Err(PromoError::Module(err)) => Err(err),
        }
    }
}
