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
use sn_domain::pricing::{Quote, QuoteLine, QuoteLineKind, StarsTotal};
use sn_domain::subscription::SubscriptionStatus;
use sn_store::{Db, StoreError, StoreResult};

pub const BOT_ENTRY: &str = "options";
pub const ITEM_DEVICES: &str = "devices";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DevicesOffer {
    pub subscription_id: SubscriptionId,
    pub included: u32,
    pub extra_now: u32,
    pub can_add: u32,
    pub months: u32,
    pub monthly: Money,
    pub monthly_stars: Option<Stars>,
    pub expire_at: DateTime<Utc>,
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

pub fn months_left(expire_at: DateTime<Utc>, now: DateTime<Utc>) -> u32 {
    let seconds = (expire_at - now).num_seconds();
    if seconds <= 0 {
        return 0;
    }
    let days = (seconds + 86_399) / 86_400;
    let months = (days * 12 + 364) / 365;
    u32::try_from(months.max(1)).unwrap_or(u32::MAX)
}

pub async fn offer(
    db: &mut impl Db,
    client_id: ClientId,
    subscription_id: SubscriptionId,
    currency: Currency,
    now: DateTime<Utc>,
) -> Result<DevicesOffer, OptionsError> {
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
    let tariff = db.tariffs().get(tariff_id).await?;
    let extra = tariff
        .devices
        .extra
        .as_ref()
        .ok_or(rejected(OptionsRejection::NotSold))?;
    let monthly = extra
        .monthly
        .get(currency)
        .ok_or(rejected(OptionsRejection::NoPrice))?;
    let pending = pending_devices(db, subscription_id).await?;
    let extra_now = subscription.extra_devices.max(pending);
    Ok(DevicesOffer {
        subscription_id,
        included: tariff.devices.included,
        extra_now,
        can_add: extra.max.saturating_sub(extra_now),
        months: months_left(expire_at, now),
        monthly,
        monthly_stars: extra.monthly_stars,
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
    let units = i64::from(count) * i64::from(offer.months);
    let amount = offer
        .monthly
        .checked_mul(units)
        .map_err(|e| ModuleError::Invalid(e.to_string()))?;
    let stars = offer
        .monthly_stars
        .map(|s| s.checked_mul(units))
        .transpose()
        .map_err(|e| ModuleError::Invalid(e.to_string()))?;
    Ok(Quote {
        currency: amount.currency,
        lines: vec![QuoteLine {
            kind: QuoteLineKind::ExtraDevices {
                count,
                months: offer.months,
            },
            label: format!(
                "Доп. устройства: {count} × {}{}{}",
                offer.months,
                format::NBSP,
                format::plural_ru(offer.months as u64, "месяц", "месяца", "месяцев")
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

async fn pending_devices(db: &mut impl Db, subscription_id: SubscriptionId) -> StoreResult<u32> {
    let pending: Option<i32> = sqlx::query_scalar(
        "SELECT max(devices_after) FROM option_purchases WHERE subscription_id = $1 AND applied_at IS NULL",
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

fn parse_request(kind: &str, payload: &serde_json::Value) -> Result<DevicesRequest, ModuleError> {
    if kind != ITEM_DEVICES {
        return Err(ModuleError::Invalid(format!("неизвестная опция: {kind}")));
    }
    serde_json::from_value(payload.clone())
        .map_err(|e| ModuleError::Invalid(format!("опция не разобрана: {e}")))
}

pub async fn record_purchase(
    ctx: &ModuleContext,
    order: &Order,
) -> Result<DevicesPurchase, ModuleError> {
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
    let request = parse_request(kind, payload)?;
    let months = order
        .quote
        .lines
        .iter()
        .find_map(|line| match line.kind {
            QuoteLineKind::ExtraDevices { months, .. } => Some(months),
            _ => None,
        })
        .unwrap_or(1);
    let mut tx = ctx.store.begin().await?;
    let subscription = tx.subscriptions().lock(request.subscription_id).await?;
    if subscription.client_id != order.client_id {
        return Err(ModuleError::NotFound("подписка"));
    }
    let before = subscription
        .extra_devices
        .max(pending_devices(&mut tx, request.subscription_id).await?);
    let after = before.saturating_add(request.count);
    sqlx::query(
        "INSERT INTO option_purchases (order_id, subscription_id, client_id, devices_before, devices_after, months) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (order_id) DO NOTHING",
    )
    .bind(order.id.0)
    .bind(request.subscription_id.0)
    .bind(order.client_id.0)
    .bind(i32::try_from(before).unwrap_or(i32::MAX))
    .bind(i32::try_from(after).unwrap_or(i32::MAX))
    .bind(i32::try_from(months).unwrap_or(1))
    .execute(tx.conn())
    .await
    .map_err(StoreError::from)?;
    let (stored_before, stored_after, applied): (i32, i32, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT devices_before, devices_after, applied_at FROM option_purchases WHERE order_id = $1",
    )
    .bind(order.id.0)
    .fetch_one(tx.conn())
    .await
    .map_err(StoreError::from)?;
    tx.commit().await?;
    Ok(DevicesPurchase {
        subscription_id: request.subscription_id,
        devices_before: u32::try_from(stored_before).unwrap_or(0),
        devices_after: u32::try_from(stored_after).unwrap_or(0),
        applied: applied.is_some(),
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
                .route("/devices/{subscription_id}", get(devices_offer))
                .route("/devices/quote", post(devices_price))
                .with_state(ctx),
        )
    }

    fn bot_entries(&self) -> Vec<BotEntry> {
        vec![BotEntry {
            key: BOT_ENTRY,
            label: "Докупить устройства",
            order: 40,
        }]
    }

    async fn quote_item(&self, ctx: &ModuleContext, hook: &ItemHook) -> Result<Quote, ModuleError> {
        let request = parse_request(&hook.kind, &hook.payload)?;
        let mut conn = ctx.store.conn().await?;
        let now = db_now(&mut conn).await?;
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

    async fn fulfill(&self, ctx: &ModuleContext, order: &Order) -> Result<(), ModuleError> {
        let purchase = record_purchase(ctx, order).await?;
        if purchase.applied {
            return Ok(());
        }
        Err(ModuleError::NotImplemented(
            "доп. устройства: ядро пока не меняет лимит устройств (нужен CoreApi::set_extra_devices)",
        ))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::testing::{TestCore, client, context, order, usd};
    use chrono::{Duration, TimeZone};
    use sn_domain::ids::{RemnaUserId, SquadId};
    use sn_domain::order::OrderKind;
    use sn_domain::subscription::{AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind};
    use sn_domain::tariff::{CurrencyPrices, DevicePolicy, ExtraDevices};
    use sn_domain::traffic::TrafficPolicy;
    use sn_store::testing::test_store;
    use sn_store::{NewCategory, NewPeriod, NewSubscription, NewTariff};
    use uuid::Uuid;

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, 12, 0, 0).unwrap()
    }

    #[test]
    fn оставшиеся_месяцы_округляются_вверх() {
        let now = at(16);
        assert_eq!(months_left(now + Duration::days(90), now), 3);
        assert_eq!(months_left(now + Duration::days(365), now), 12);
        assert_eq!(months_left(now + Duration::days(30), now), 1);
        assert_eq!(months_left(now + Duration::days(31), now), 2);
        assert_eq!(months_left(now + Duration::days(61), now), 3);
        assert_eq!(months_left(now + Duration::hours(5), now), 1);
        assert_eq!(months_left(now - Duration::hours(1), now), 0);
    }

    fn sample_offer(can_add: u32) -> DevicesOffer {
        DevicesOffer {
            subscription_id: SubscriptionId(1),
            included: 3,
            extra_now: 2 - can_add.min(2),
            can_add,
            months: 3,
            monthly: usd(99),
            monthly_stars: Some(Stars(50)),
            expire_at: at(16) + Duration::days(90),
        }
    }

    #[test]
    fn цена_доп_устройств_за_оставшиеся_месяцы() {
        let q = devices_quote(&sample_offer(2), 2).unwrap();
        assert_eq!(q.total, usd(2 * 99 * 3));
        assert_eq!(q.stars.unwrap().total, Stars(300));
        assert_eq!(q.extra_devices, 2);
        assert_eq!(q.lines[0].label, "Доп. устройства: 2 × 3\u{a0}месяца");
        assert!(matches!(
            devices_quote(&sample_offer(1), 2),
            Err(OptionsError::Rejected(OptionsRejection::TooMany {
                can_add: 1
            }))
        ));
        assert!(matches!(
            devices_quote(&sample_offer(2), 0),
            Err(OptionsError::Rejected(OptionsRejection::ZeroCount))
        ));
    }

    #[tokio::test]
    async fn покупка_устройств_фиксирует_цель_и_ждёт_ядро() {
        let Some(db) = test_store().await else {
            return;
        };
        let core = TestCore::new(&db.store);
        let ctx = context(&db.store, &core, Currency::Usd);
        let owner = client(&db.store).await;
        let sub_id = {
            let mut tx = db.store.begin().await.unwrap();
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
                    traffic: TrafficPolicy::UNLIMITED,
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
                        user_id: RemnaUserId(77),
                        username: "sn_test_77".into(),
                        short_uuid: None,
                    },
                )
                .await
                .unwrap();
            tx.subscriptions()
                .apply_mirror(
                    sub.id,
                    &RemnaMirror {
                        status: Some(SubscriptionStatus::Active),
                        expire_at: Some(Utc::now() + Duration::days(89)),
                        ..RemnaMirror::default()
                    },
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            sub.id
        };
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
        assert_eq!(quote.total, usd(2 * 99 * 3));
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
        assert!(matches!(
            OptionsModule.fulfill(&ctx, &paid).await,
            Err(ModuleError::NotImplemented(_))
        ));
        let again = record_purchase(&ctx, &paid).await.unwrap();
        assert_eq!((again.devices_before, again.devices_after), (0, 2));
        assert!(!again.applied);
        let mut conn = db.store.conn().await.unwrap();
        let now = db_now(&mut conn).await.unwrap();
        let left = offer(&mut conn, owner, sub_id, Currency::Usd, now)
            .await
            .unwrap();
        assert_eq!((left.extra_now, left.can_add), (2, 0));
        drop(conn);
        mark_applied(&ctx, paid.id).await.unwrap();
        assert!(OptionsModule.fulfill(&ctx, &paid).await.is_ok());
        db.cleanup().await;
    }
}
