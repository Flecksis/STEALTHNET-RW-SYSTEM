use crate::catalog::web::{Customer, WebError};
use crate::catalog::{db_now, installation_currency};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{BotEntry, ItemHook, Module};
use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::format;
use sn_domain::ids::{ClientId, OrderId, SubscriptionId};
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money, Stars};
use sn_domain::order::{Order, OrderStatus, PurchaseIntent};
use sn_domain::pricing::{
    Quote, QuoteLine, QuoteLineKind, StarsTotal, days_left, prorated, prorated_stars,
};
use sn_domain::subscription::{Subscription, SubscriptionStatus};
use sn_domain::tariff::{Tariff, TrafficLeftover};
use sn_domain::traffic::{GIB, TrafficReset};
use sn_store::{Db, StoreError, StoreResult};

pub const BOT_ENTRY: &str = "options";
pub const ITEM_DEVICES: &str = "devices";
pub const ITEM_TRAFFIC: &str = "traffic";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesRequest {
    pub subscription_id: SubscriptionId,
    pub count: u32,
}

impl DevicesRequest {
    pub fn payload(&self) -> serde_json::Value {
        serde_json::json!({ "subscription_id": self.subscription_id, "count": self.count })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficRequest {
    pub subscription_id: SubscriptionId,
    pub gb: u32,
}

impl TrafficRequest {
    pub fn payload(&self) -> serde_json::Value {
        serde_json::json!({ "subscription_id": self.subscription_id, "gb": self.gb })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionItem {
    Devices(DevicesRequest),
    Traffic(TrafficRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DevicesOffer {
    pub subscription_id: SubscriptionId,
    pub included: u32,
    pub extra_now: u32,
    pub can_add: u32,
    pub days: u32,
    pub monthly: Money,
    pub monthly_stars: Option<Stars>,
    pub per_device: Money,
    pub per_device_stars: Option<Stars>,
    pub expire_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrafficPackOffer {
    pub gb: u32,
    pub price: Money,
    pub stars: Option<Stars>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrafficOffer {
    pub subscription_id: SubscriptionId,
    pub packs: Vec<TrafficPackOffer>,
    pub limit_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub extra_bytes: u64,
    pub leftover: TrafficLeftover,
    pub reset: TrafficReset,
    pub expire_at: DateTime<Utc>,
    pub window_ends_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unavailable {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Availability<T> {
    pub offer: Option<T>,
    pub unavailable: Option<Unavailable>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddonsOffer {
    pub subscription_id: SubscriptionId,
    pub devices: Availability<DevicesOffer>,
    pub traffic: Availability<TrafficOffer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum OptionsRejection {
    NoSubscription,
    SubscriptionEnded,
    SubscriptionBlocked,
    NotSold,
    NoPrice,
    TooMany { can_add: u32 },
    ZeroCount,
    TrafficNotSold,
    TrafficUnlimited,
    NoPack,
}

impl OptionsRejection {
    pub const fn code(self) -> &'static str {
        match self {
            OptionsRejection::NoSubscription => "options.no_subscription",
            OptionsRejection::SubscriptionEnded => "options.subscription_ended",
            OptionsRejection::SubscriptionBlocked => "options.subscription_blocked",
            OptionsRejection::NotSold => "options.not_sold",
            OptionsRejection::NoPrice => "options.no_price",
            OptionsRejection::TooMany { .. } => "options.too_many",
            OptionsRejection::ZeroCount => "options.zero",
            OptionsRejection::TrafficNotSold => "options.traffic_not_sold",
            OptionsRejection::TrafficUnlimited => "options.traffic_unlimited",
            OptionsRejection::NoPack => "options.no_pack",
        }
    }

    pub fn message(self) -> String {
        match self {
            OptionsRejection::NoSubscription => "Подписка не найдена".into(),
            OptionsRejection::SubscriptionEnded => {
                "Подписка закончилась: сначала продлите её".into()
            }
            OptionsRejection::SubscriptionBlocked => {
                "Подписка заблокирована. Напишите в поддержку.".into()
            }
            OptionsRejection::NotSold => "Для этого тарифа доп. устройства не продаются".into(),
            OptionsRejection::NoPrice => "У доп. устройства нет цены в валюте сервиса".into(),
            OptionsRejection::TooMany { can_add: 0 } => {
                "Докупить больше устройств к этой подписке нельзя".into()
            }
            OptionsRejection::TooMany { can_add } => format!(
                "Можно докупить не больше {can_add}{}{}",
                format::NBSP,
                format::plural_ru(can_add as u64, "устройства", "устройств", "устройств")
            ),
            OptionsRejection::ZeroCount => "Выберите хотя бы одно устройство".into(),
            OptionsRejection::TrafficNotSold => "Для этого тарифа трафик не докупается".into(),
            OptionsRejection::TrafficUnlimited => "Трафик в тарифе без ограничений".into(),
            OptionsRejection::NoPack => "Такого пакета трафика нет, выберите другой".into(),
        }
    }

    fn unavailable(self) -> Unavailable {
        Unavailable {
            code: self.code(),
            message: self.message(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OptionsError {
    #[error("{}", .0.message())]
    Rejected(OptionsRejection),
    #[error(transparent)]
    Module(#[from] ModuleError),
}

impl From<StoreError> for OptionsError {
    fn from(err: StoreError) -> Self {
        OptionsError::Module(ModuleError::Store(err))
    }
}

impl From<OptionsError> for ModuleError {
    fn from(err: OptionsError) -> Self {
        match err {
            OptionsError::Rejected(rejection) => ModuleError::Invalid(rejection.message()),
            OptionsError::Module(err) => err,
        }
    }
}

impl From<OptionsError> for WebError {
    fn from(err: OptionsError) -> Self {
        match err {
            OptionsError::Rejected(rejection) => {
                WebError::rejected(rejection.code(), rejection.message())
            }
            OptionsError::Module(err) => err.into(),
        }
    }
}

fn domain(err: sn_domain::DomainError) -> OptionsError {
    OptionsError::Module(ModuleError::Invalid(err.to_string()))
}

fn days_word(days: u32) -> &'static str {
    format::plural_ru(u64::from(days), "день", "дня", "дней")
}

struct Live {
    subscription: Subscription,
    tariff: Tariff,
    expire_at: DateTime<Utc>,
}

async fn live(
    db: &mut impl Db,
    client_id: ClientId,
    subscription_id: SubscriptionId,
    now: DateTime<Utc>,
) -> Result<Live, OptionsError> {
    let rejected = OptionsError::Rejected;
    let subscription = match db.subscriptions().get(subscription_id).await {
        Ok(subscription) => subscription,
        Err(StoreError::NotFound(_)) => return Err(rejected(OptionsRejection::NoSubscription)),
        Err(err) => return Err(err.into()),
    };
    if subscription.client_id != client_id || subscription.remna.is_none() {
        return Err(rejected(OptionsRejection::NoSubscription));
    }
    match subscription.mirror.status {
        Some(SubscriptionStatus::Disabled) => {
            return Err(rejected(OptionsRejection::SubscriptionBlocked));
        }
        Some(SubscriptionStatus::Expired) => {
            return Err(rejected(OptionsRejection::SubscriptionEnded));
        }
        _ => {}
    }
    let expire_at = subscription
        .mirror
        .expire_at
        .filter(|at| *at > now && subscription.ended_at.is_none())
        .ok_or(rejected(OptionsRejection::SubscriptionEnded))?;
    let tariff_id = subscription
        .tariff_id
        .ok_or(rejected(OptionsRejection::NotSold))?;
    let tariff = match db.tariffs().get(tariff_id).await {
        Ok(tariff) => tariff,
        Err(StoreError::NotFound(_)) => return Err(rejected(OptionsRejection::NotSold)),
        Err(err) => return Err(err.into()),
    };
    Ok(Live {
        subscription,
        tariff,
        expire_at,
    })
}

pub async fn offer(
    db: &mut impl Db,
    client_id: ClientId,
    subscription_id: SubscriptionId,
    currency: Currency,
    now: DateTime<Utc>,
) -> Result<DevicesOffer, OptionsError> {
    let rejected = OptionsError::Rejected;
    let Live {
        subscription,
        tariff,
        expire_at,
    } = live(db, client_id, subscription_id, now).await?;
    let extra = tariff
        .devices
        .extra
        .as_ref()
        .ok_or(rejected(OptionsRejection::NotSold))?;
    let monthly = extra
        .monthly
        .get(currency)
        .ok_or(rejected(OptionsRejection::NoPrice))?;
    let days = days_left(expire_at, now);
    let pending = pending_devices(db, subscription_id).await?;
    let extra_now = subscription.extra_devices.max(pending);
    Ok(DevicesOffer {
        subscription_id,
        included: tariff.devices.included,
        extra_now,
        can_add: extra.max.saturating_sub(extra_now),
        days,
        monthly,
        monthly_stars: extra.monthly_stars,
        per_device: prorated(monthly, 1, days).map_err(domain)?,
        per_device_stars: extra
            .monthly_stars
            .map(|s| prorated_stars(s, 1, days))
            .transpose()
            .map_err(domain)?,
        expire_at,
    })
}

pub fn devices_quote(offer: &DevicesOffer, count: u32) -> Result<Quote, OptionsError> {
    if count == 0 {
        return Err(OptionsError::Rejected(OptionsRejection::ZeroCount));
    }
    if count > offer.can_add {
        return Err(OptionsError::Rejected(OptionsRejection::TooMany {
            can_add: offer.can_add,
        }));
    }
    let amount = prorated(offer.monthly, count, offer.days).map_err(domain)?;
    let stars = offer
        .monthly_stars
        .map(|s| prorated_stars(s, count, offer.days))
        .transpose()
        .map_err(domain)?;
    Ok(Quote {
        currency: amount.currency,
        lines: vec![QuoteLine {
            kind: QuoteLineKind::DeviceAddon {
                count,
                days: offer.days,
            },
            label: format!(
                "Доп. устройства: +{count} на {}{}{}",
                offer.days,
                format::NBSP,
                days_word(offer.days)
            ),
            amount,
            stars,
        }],
        subtotal: amount,
        discount: Money::zero(amount.currency),
        total: amount,
        stars: stars.map(|total| StarsTotal {
            subtotal: total,
            discount: Stars::ZERO,
            total,
        }),
        days: 0,
        extra_devices: count,
    })
}

pub async fn traffic_offer(
    db: &mut impl Db,
    client_id: ClientId,
    subscription_id: SubscriptionId,
    currency: Currency,
    now: DateTime<Utc>,
) -> Result<TrafficOffer, OptionsError> {
    let rejected = OptionsError::Rejected;
    let Live {
        subscription,
        tariff,
        expire_at,
    } = live(db, client_id, subscription_id, now).await?;
    if !tariff.traffic.is_limited() || subscription.mirror.traffic_limit_bytes == Some(0) {
        return Err(rejected(OptionsRejection::TrafficUnlimited));
    }
    let packs: Vec<TrafficPackOffer> = tariff
        .traffic_packs(currency)
        .into_iter()
        .filter_map(|pack| {
            Some(TrafficPackOffer {
                gb: pack.gb,
                price: pack.price(currency)?,
                stars: pack.stars,
            })
        })
        .collect();
    if packs.is_empty() {
        return Err(rejected(OptionsRejection::TrafficNotSold));
    }
    let extra = db.subscriptions().extra_traffic(subscription_id).await?;
    Ok(TrafficOffer {
        subscription_id,
        packs,
        limit_bytes: subscription.mirror.traffic_limit_bytes,
        used_bytes: subscription.mirror.traffic_used_bytes,
        extra_bytes: extra.map_or(0, |e| e.bytes),
        leftover: tariff.traffic_leftover(),
        reset: tariff.traffic.reset,
        expire_at,
        window_ends_at: extra.and_then(|e| e.window_ends_at),
    })
}

pub fn traffic_quote(offer: &TrafficOffer, gb: u32) -> Result<Quote, OptionsError> {
    let pack = offer
        .packs
        .iter()
        .find(|p| p.gb == gb)
        .ok_or(OptionsError::Rejected(OptionsRejection::NoPack))?;
    Ok(Quote {
        currency: pack.price.currency,
        lines: vec![QuoteLine {
            kind: QuoteLineKind::TrafficAddon { gb },
            label: format!("Трафик: +{gb}{}ГБ", format::NBSP),
            amount: pack.price,
            stars: pack.stars,
        }],
        subtotal: pack.price,
        discount: Money::zero(pack.price.currency),
        total: pack.price,
        stars: pack.stars.map(|total| StarsTotal {
            subtotal: total,
            discount: Stars::ZERO,
            total,
        }),
        days: 0,
        extra_devices: 0,
    })
}

fn available<T>(result: Result<T, OptionsError>) -> Result<Availability<T>, OptionsError> {
    match result {
        Ok(offer) => Ok(Availability {
            offer: Some(offer),
            unavailable: None,
        }),
        Err(OptionsError::Rejected(rejection)) => Ok(Availability {
            offer: None,
            unavailable: Some(rejection.unavailable()),
        }),
        Err(err) => Err(err),
    }
}

pub async fn addons_offer(
    db: &mut impl Db,
    client_id: ClientId,
    subscription_id: SubscriptionId,
    currency: Currency,
    now: DateTime<Utc>,
) -> Result<AddonsOffer, OptionsError> {
    let devices = available(offer(db, client_id, subscription_id, currency, now).await)?;
    let traffic = available(traffic_offer(db, client_id, subscription_id, currency, now).await)?;
    Ok(AddonsOffer {
        subscription_id,
        devices,
        traffic,
    })
}

async fn pending_devices(db: &mut impl Db, subscription_id: SubscriptionId) -> StoreResult<u32> {
    let pending: Option<i32> = sqlx::query_scalar(
        "SELECT max(devices_after) FROM option_purchases \
         WHERE subscription_id = $1 AND item = 'devices' AND applied_at IS NULL",
    )
    .bind(subscription_id.0)
    .fetch_one(db.conn())
    .await?;
    Ok(pending.and_then(|p| u32::try_from(p).ok()).unwrap_or(0))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DevicesPurchase {
    pub subscription_id: SubscriptionId,
    pub devices_before: u32,
    pub devices_after: u32,
    pub applied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TrafficPurchase {
    pub subscription_id: SubscriptionId,
    pub bytes: u64,
    pub applied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum Purchase {
    Devices(DevicesPurchase),
    Traffic(TrafficPurchase),
}

fn parse_request(kind: &str, payload: &serde_json::Value) -> Result<OptionItem, ModuleError> {
    let parsed = match kind {
        ITEM_DEVICES => serde_json::from_value(payload.clone()).map(OptionItem::Devices),
        ITEM_TRAFFIC => serde_json::from_value(payload.clone()).map(OptionItem::Traffic),
        other => return Err(ModuleError::Invalid(format!("неизвестная опция: {other}"))),
    };
    parsed.map_err(|e| ModuleError::Invalid(format!("опция не разобрана: {e}")))
}

fn device_days(order: &Order) -> u32 {
    order
        .quote
        .lines
        .iter()
        .find_map(|line| match line.kind {
            QuoteLineKind::DeviceAddon { days, .. } => Some(days.max(1)),
            QuoteLineKind::ExtraDevices { months, .. } => Some(months.max(1) * 30),
            _ => None,
        })
        .unwrap_or(1)
}

pub async fn record_purchase(ctx: &ModuleContext, order: &Order) -> Result<Purchase, ModuleError> {
    let PurchaseIntent::Module {
        module: ModuleId::Options,
        kind,
        payload,
    } = &order.intent
    else {
        return Err(ModuleError::Invalid("заказ не на доп. опции".into()));
    };
    if !matches!(order.status, OrderStatus::Paid | OrderStatus::Review) {
        return Err(ModuleError::Invalid("заказ не оплачен".into()));
    }
    let item = parse_request(kind, payload)?;
    let subscription_id = match item {
        OptionItem::Devices(request) => request.subscription_id,
        OptionItem::Traffic(request) => request.subscription_id,
    };
    let mut tx = ctx.store.begin().await?;
    let subscription = tx.subscriptions().lock(subscription_id).await?;
    if subscription.client_id != order.client_id {
        return Err(ModuleError::NotFound("подписка"));
    }
    let before = subscription
        .extra_devices
        .max(pending_devices(&mut tx, subscription_id).await?);
    let (item_name, after, traffic, days) = match item {
        OptionItem::Devices(request) => (
            ITEM_DEVICES,
            before.saturating_add(request.count),
            None,
            Some(device_days(order)),
        ),
        OptionItem::Traffic(request) => (
            ITEM_TRAFFIC,
            before,
            Some(u64::from(request.gb) * GIB),
            None,
        ),
    };
    sqlx::query(
        "INSERT INTO option_purchases (order_id, subscription_id, client_id, devices_before, devices_after, \
         months, item, traffic_bytes, days) VALUES ($1, $2, $3, $4, $5, 0, $6, $7, $8) \
         ON CONFLICT (order_id) DO NOTHING",
    )
    .bind(order.id.0)
    .bind(subscription_id.0)
    .bind(order.client_id.0)
    .bind(i32::try_from(before).unwrap_or(i32::MAX))
    .bind(i32::try_from(after).unwrap_or(i32::MAX))
    .bind(item_name)
    .bind(traffic.map(|b| i64::try_from(b).unwrap_or(i64::MAX)))
    .bind(days.map(|d| i32::try_from(d).unwrap_or(i32::MAX)))
    .execute(tx.conn())
    .await
    .map_err(StoreError::from)?;
    let (stored_item, stored_before, stored_after, stored_bytes, applied): (
        String,
        i32,
        i32,
        Option<i64>,
        Option<DateTime<Utc>>,
    ) = sqlx::query_as(
        "SELECT item, devices_before, devices_after, traffic_bytes, applied_at \
         FROM option_purchases WHERE order_id = $1",
    )
    .bind(order.id.0)
    .fetch_one(tx.conn())
    .await
    .map_err(StoreError::from)?;
    tx.commit().await?;
    Ok(if stored_item == ITEM_TRAFFIC {
        Purchase::Traffic(TrafficPurchase {
            subscription_id,
            bytes: stored_bytes
                .and_then(|b| u64::try_from(b).ok())
                .unwrap_or(0),
            applied: applied.is_some(),
        })
    } else {
        Purchase::Devices(DevicesPurchase {
            subscription_id,
            devices_before: u32::try_from(stored_before).unwrap_or(0),
            devices_after: u32::try_from(stored_after).unwrap_or(0),
            applied: applied.is_some(),
        })
    })
}

pub async fn mark_applied(ctx: &ModuleContext, order_id: OrderId) -> Result<(), ModuleError> {
    let mut conn = ctx.store.conn().await?;
    sqlx::query(
        "UPDATE option_purchases SET applied_at = COALESCE(applied_at, now()) WHERE order_id = $1",
    )
    .bind(order_id.0)
    .execute(conn.conn())
    .await
    .map_err(StoreError::from)?;
    Ok(())
}

pub struct OptionsModule;

#[async_trait]
impl Module for OptionsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Options
    }

    fn client_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(
            Router::new()
                .route("/offer/{subscription_id}", get(addons))
                .route("/devices/{subscription_id}", get(devices_offer))
                .route("/devices/quote", post(devices_price))
                .route("/traffic/{subscription_id}", get(traffic))
                .route("/traffic/quote", post(traffic_price))
                .with_state(ctx),
        )
    }

    fn bot_entries(&self) -> Vec<BotEntry> {
        vec![BotEntry {
            key: BOT_ENTRY,
            label: "Докупить",
            order: 40,
        }]
    }

    async fn quote_item(&self, ctx: &ModuleContext, hook: &ItemHook) -> Result<Quote, ModuleError> {
        let item = parse_request(&hook.kind, &hook.payload)?;
        let mut conn = ctx.store.conn().await?;
        let now = db_now(&mut conn).await?;
        match item {
            OptionItem::Devices(request) => {
                let offer = offer(
                    &mut conn,
                    hook.client_id,
                    request.subscription_id,
                    hook.currency,
                    now,
                )
                .await?;
                Ok(devices_quote(&offer, request.count)?)
            }
            OptionItem::Traffic(request) => {
                let offer = traffic_offer(
                    &mut conn,
                    hook.client_id,
                    request.subscription_id,
                    hook.currency,
                    now,
                )
                .await?;
                Ok(traffic_quote(&offer, request.gb)?)
            }
        }
    }

    async fn fulfill(&self, ctx: &ModuleContext, order: &Order) -> Result<(), ModuleError> {
        match record_purchase(ctx, order).await? {
            Purchase::Devices(purchase) if !purchase.applied => {
                ctx.core
                    .apply_device_addon(order.id, purchase.subscription_id, purchase.devices_after)
                    .await?;
            }
            Purchase::Traffic(purchase) if !purchase.applied => {
                ctx.core
                    .apply_traffic_addon(order.id, purchase.subscription_id, purchase.bytes)
                    .await?;
            }
            _ => return Ok(()),
        }
        mark_applied(ctx, order.id).await
    }
}

async fn addons(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Path(subscription_id): Path<SubscriptionId>,
) -> Result<Json<AddonsOffer>, WebError> {
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let now = db_now(&mut conn).await?;
    Ok(Json(
        addons_offer(
            &mut conn,
            customer.0.client_id,
            subscription_id,
            currency,
            now,
        )
        .await?,
    ))
}

async fn devices_offer(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Path(subscription_id): Path<SubscriptionId>,
) -> Result<Json<DevicesOffer>, WebError> {
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let now = db_now(&mut conn).await?;
    Ok(Json(
        offer(
            &mut conn,
            customer.0.client_id,
            subscription_id,
            currency,
            now,
        )
        .await?,
    ))
}

async fn devices_price(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Json(request): Json<DevicesRequest>,
) -> Result<Json<Quote>, WebError> {
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let now = db_now(&mut conn).await?;
    let offer = offer(
        &mut conn,
        customer.0.client_id,
        request.subscription_id,
        currency,
        now,
    )
    .await?;
    Ok(Json(devices_quote(&offer, request.count)?))
}

async fn traffic(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Path(subscription_id): Path<SubscriptionId>,
) -> Result<Json<TrafficOffer>, WebError> {
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let now = db_now(&mut conn).await?;
    Ok(Json(
        traffic_offer(
            &mut conn,
            customer.0.client_id,
            subscription_id,
            currency,
            now,
        )
        .await?,
    ))
}

async fn traffic_price(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Json(request): Json<TrafficRequest>,
) -> Result<Json<Quote>, WebError> {
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let now = db_now(&mut conn).await?;
    let offer = traffic_offer(
        &mut conn,
        customer.0.client_id,
        request.subscription_id,
        currency,
        now,
    )
    .await?;
    Ok(Json(traffic_quote(&offer, request.gb)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::testing::{TestCore, client, context, order, usd};
    use chrono::{Duration, TimeZone};
    use sn_domain::ids::{RemnaUserId, SquadId};
    use sn_domain::order::OrderKind;
    use sn_domain::subscription::{AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind};
    use sn_domain::tariff::{
        CurrencyPrices, DevicePolicy, ExtraDevices, TrafficAddon, TrafficLeftover, TrafficPack,
    };
    use sn_domain::traffic::TrafficPolicy;
    use sn_store::testing::test_store;
    use sn_store::{NewCategory, NewPeriod, NewSubscription, NewTariff, Store};
    use uuid::Uuid;

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, 12, 0, 0).unwrap()
    }

    fn sample_offer(can_add: u32, days: u32) -> DevicesOffer {
        DevicesOffer {
            subscription_id: SubscriptionId(1),
            included: 3,
            extra_now: 2 - can_add.min(2),
            can_add,
            days,
            monthly: usd(99),
            monthly_stars: Some(Stars(50)),
            per_device: prorated(usd(99), 1, days).unwrap(),
            per_device_stars: Some(prorated_stars(Stars(50), 1, days).unwrap()),
            expire_at: at(16) + Duration::days(i64::from(days)),
        }
    }

    #[test]
    fn цена_доп_устройств_за_оставшиеся_дни() {
        let half = devices_quote(&sample_offer(2, 15), 1).unwrap();
        assert_eq!(half.total, usd(50));
        assert_eq!(half.stars.unwrap().total, Stars(25));
        let two = devices_quote(&sample_offer(2, 15), 2).unwrap();
        assert_eq!(two.total, usd(99));
        assert_eq!(two.extra_devices, 2);
        assert_eq!(two.lines[0].label, "Доп. устройства: +2 на 15\u{a0}дней");
        assert_eq!(
            two.lines[0].kind,
            QuoteLineKind::DeviceAddon { count: 2, days: 15 }
        );
        let quarter = devices_quote(&sample_offer(2, 90), 1).unwrap();
        assert_eq!(quarter.total, usd(297));
        assert!(matches!(
            devices_quote(&sample_offer(1, 15), 2),
            Err(OptionsError::Rejected(OptionsRejection::TooMany {
                can_add: 1
            }))
        ));
        assert!(matches!(
            devices_quote(&sample_offer(2, 15), 0),
            Err(OptionsError::Rejected(OptionsRejection::ZeroCount))
        ));
    }

    fn sample_traffic() -> TrafficOffer {
        TrafficOffer {
            subscription_id: SubscriptionId(1),
            packs: vec![
                TrafficPackOffer {
                    gb: 10,
                    price: usd(150),
                    stars: Some(Stars(75)),
                },
                TrafficPackOffer {
                    gb: 50,
                    price: usd(500),
                    stars: None,
                },
            ],
            limit_bytes: Some(100 * GIB),
            used_bytes: Some(98 * GIB),
            extra_bytes: 0,
            leftover: TrafficLeftover::Burn,
            reset: TrafficReset::Month,
            expire_at: at(30),
            window_ends_at: None,
        }
    }

    #[test]
    fn пакет_трафика_по_полной_цене() {
        let q = traffic_quote(&sample_traffic(), 10).unwrap();
        assert_eq!(q.total, usd(150));
        assert_eq!(q.stars.unwrap().total, Stars(75));
        assert_eq!(q.lines[0].kind, QuoteLineKind::TrafficAddon { gb: 10 });
        assert_eq!(q.lines[0].label, "Трафик: +10\u{a0}ГБ");
        assert!(
            traffic_quote(&sample_traffic(), 50)
                .unwrap()
                .stars
                .is_none()
        );
        assert!(matches!(
            traffic_quote(&sample_traffic(), 20),
            Err(OptionsError::Rejected(OptionsRejection::NoPack))
        ));
    }

    async fn subscription_with(
        store: &Store,
        owner: ClientId,
        remna_user: i64,
        traffic: TrafficPolicy,
        traffic_addon: Option<TrafficAddon>,
        mirror: RemnaMirror,
    ) -> SubscriptionId {
        let mut tx = store.begin().await.unwrap();
        let category = tx
            .tariffs()
            .create_category(&NewCategory {
                name: "Основные".into(),
                sort: 0,
                visible: true,
            })
            .await
            .unwrap();
        let tariff = tx
            .tariffs()
            .create(&NewTariff {
                traffic_addon,
                category_id: category.id,
                name: "Стандарт".into(),
                description: None,
                on_sale: true,
                sort: 0,
                devices: DevicePolicy {
                    included: 3,
                    extra: Some(ExtraDevices {
                        max: 2,
                        monthly: CurrencyPrices {
                            usd: Some(99),
                            rub: Some(4_900),
                        },
                        monthly_stars: Some(Stars(50)),
                    }),
                },
                traffic,
                squads: vec![SquadId(Uuid::from_u128(1))],
                external_squad: None,
                periods: vec![NewPeriod {
                    days: 90,
                    prices: CurrencyPrices {
                        usd: Some(699),
                        rub: Some(49_900),
                    },
                    stars: Some(Stars(350)),
                    sort: 0,
                    active: true,
                }],
            })
            .await
            .unwrap();
        let sub = tx
            .subscriptions()
            .create(&NewSubscription {
                client_id: owner,
                tariff_id: Some(tariff.id),
                kind: SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: AutoRenewState::default(),
            })
            .await
            .unwrap();
        tx.subscriptions()
            .link_remna(
                sub.id,
                &RemnaLink {
                    user_id: RemnaUserId(remna_user),
                    username: format!("sn_test_{remna_user}"),
                    short_uuid: None,
                },
            )
            .await
            .unwrap();
        tx.subscriptions()
            .apply_mirror(sub.id, &mirror)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        sub.id
    }

    #[tokio::test]
    async fn покупка_устройств_доходит_до_ядра_один_раз() {
        let Some(db) = test_store().await else {
            return;
        };
        let core = TestCore::new(&db.store);
        let ctx = context(&db.store, &core, Currency::Usd);
        let owner = client(&db.store).await;
        let sub_id = subscription_with(
            &db.store,
            owner,
            77,
            TrafficPolicy::UNLIMITED,
            None,
            RemnaMirror {
                status: Some(SubscriptionStatus::Active),
                expire_at: Some(Utc::now() + Duration::days(14) + Duration::hours(1)),
                ..RemnaMirror::default()
            },
        )
        .await;
        let request = DevicesRequest {
            subscription_id: sub_id,
            count: 2,
        };
        let hook = ItemHook {
            client_id: owner,
            currency: Currency::Usd,
            kind: ITEM_DEVICES.into(),
            payload: request.payload(),
        };
        let quote = OptionsModule.quote_item(&ctx, &hook).await.unwrap();
        assert_eq!(quote.total, usd(99));
        let stranger = client(&db.store).await;
        assert!(
            OptionsModule
                .quote_item(
                    &ctx,
                    &ItemHook {
                        client_id: stranger,
                        ..hook.clone()
                    }
                )
                .await
                .is_err()
        );
        let mut paid = order(
            &db.store,
            owner,
            OrderKind::Module,
            PurchaseIntent::Module {
                module: ModuleId::Options,
                kind: ITEM_DEVICES.into(),
                payload: request.payload(),
            },
            quote,
        )
        .await;
        paid.status = OrderStatus::Paid;
        OptionsModule.fulfill(&ctx, &paid).await.unwrap();
        OptionsModule.fulfill(&ctx, &paid).await.unwrap();
        assert_eq!(
            core.addons(),
            vec![(paid.id, sub_id, ITEM_DEVICES.to_string(), 2)]
        );
        let again = record_purchase(&ctx, &paid).await.unwrap();
        assert_eq!(
            again,
            Purchase::Devices(DevicesPurchase {
                subscription_id: sub_id,
                devices_before: 0,
                devices_after: 2,
                applied: true,
            })
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn пакет_трафика_продаётся_только_при_лимите() {
        let Some(db) = test_store().await else {
            return;
        };
        let core = TestCore::new(&db.store);
        let ctx = context(&db.store, &core, Currency::Usd);
        let owner = client(&db.store).await;
        let addon = TrafficAddon {
            packs: vec![TrafficPack {
                gb: 10,
                prices: CurrencyPrices {
                    usd: Some(150),
                    rub: Some(10_000),
                },
                stars: Some(Stars(75)),
            }],
            leftover: TrafficLeftover::Carry,
        };
        let mirror = RemnaMirror {
            status: Some(SubscriptionStatus::Limited),
            expire_at: Some(Utc::now() + Duration::days(20)),
            traffic_limit_bytes: Some(100 * GIB),
            traffic_used_bytes: Some(100 * GIB),
            ..RemnaMirror::default()
        };
        let limited = TrafficPolicy {
            limit_bytes: Some(100 * GIB),
            reset: TrafficReset::OnPayment,
        };
        let sub_id = subscription_with(
            &db.store,
            owner,
            78,
            limited,
            Some(addon.clone()),
            mirror.clone(),
        )
        .await;
        let mut conn = db.store.conn().await.unwrap();
        let now = db_now(&mut conn).await.unwrap();
        let view = addons_offer(&mut conn, owner, sub_id, Currency::Usd, now)
            .await
            .unwrap();
        let traffic = view.traffic.offer.unwrap();
        assert_eq!(traffic.packs.len(), 1);
        assert_eq!(traffic.leftover, TrafficLeftover::Carry);
        assert_eq!(traffic.used_bytes, Some(100 * GIB));
        assert!(view.devices.offer.is_some());
        drop(conn);
        let request = TrafficRequest {
            subscription_id: sub_id,
            gb: 10,
        };
        let quote = OptionsModule
            .quote_item(
                &ctx,
                &ItemHook {
                    client_id: owner,
                    currency: Currency::Usd,
                    kind: ITEM_TRAFFIC.into(),
                    payload: request.payload(),
                },
            )
            .await
            .unwrap();
        assert_eq!(quote.total, usd(150));
        let mut paid = order(
            &db.store,
            owner,
            OrderKind::Module,
            PurchaseIntent::Module {
                module: ModuleId::Options,
                kind: ITEM_TRAFFIC.into(),
                payload: request.payload(),
            },
            quote,
        )
        .await;
        paid.status = OrderStatus::Paid;
        OptionsModule.fulfill(&ctx, &paid).await.unwrap();
        assert_eq!(
            core.addons(),
            vec![(paid.id, sub_id, ITEM_TRAFFIC.to_string(), 10 * GIB)]
        );
        let unlimited = subscription_with(
            &db.store,
            owner,
            79,
            TrafficPolicy::UNLIMITED,
            Some(addon),
            mirror,
        )
        .await;
        let mut conn = db.store.conn().await.unwrap();
        let view = addons_offer(&mut conn, owner, unlimited, Currency::Usd, now)
            .await
            .unwrap();
        assert_eq!(
            view.traffic.unavailable.map(|u| u.code),
            Some("options.traffic_unlimited")
        );
        drop(conn);
        db.cleanup().await;
    }
}
