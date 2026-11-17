use super::*;
use crate::checkout::testkit::*;
use crate::checkout::{Channel, CheckoutResult, ItemCheckoutRequest, ItemRequest};
use chrono::{Duration as Days, Utc};
use sn_domain::ids::ClientId;
use sn_domain::module::ModuleId;
use sn_domain::money::Stars;
use sn_domain::payment::providers;
use sn_domain::subscription::SubscriptionStatus;
use sn_domain::tariff::{
    CurrencyPrices, DevicePolicy, ExtraDevices, Tariff, TrafficAddon, TrafficLeftover, TrafficPack,
};
use sn_domain::traffic::{GIB, TrafficPolicy, TrafficReset};
use sn_modules::Module;
use sn_modules::catalog::options::{
    DevicesRequest, ITEM_DEVICES, ITEM_TRAFFIC, OptionsModule, TrafficRequest,
};
use sn_remnawave::UserStatus;
use sn_store::{NewPeriod, NewTariff};
use std::sync::Arc;

async fn options_kit() -> Option<Kit> {
    kit_with(vec![Arc::new(OptionsModule) as Arc<dyn Module>]).await
}

async fn limited(kit: &Kit, reset: TrafficReset, leftover: TrafficLeftover) -> Tariff {
    let category = category(&kit.services, "Лимит").await;
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.tariffs()
        .create(&NewTariff {
            category_id: category,
            name: "Сто гигабайт".into(),
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
            traffic: TrafficPolicy {
                limit_bytes: Some(100 * GIB),
                reset,
            },
            traffic_addon: Some(TrafficAddon {
                packs: vec![TrafficPack {
                    gb: 10,
                    prices: CurrencyPrices {
                        usd: Some(150),
                        rub: Some(10_000),
                    },
                    stars: Some(Stars(75)),
                }],
                leftover,
            }),
            squads: vec![squad(1)],
            external_squad: None,
            periods: vec![NewPeriod {
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(299),
                    rub: Some(19_900),
                },
                stars: Some(Stars(150)),
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap()
}

fn devices_item(subscription: &Subscription, count: u32) -> ItemRequest {
    ItemRequest {
        module: ModuleId::Options,
        kind: ITEM_DEVICES.into(),
        payload: DevicesRequest {
            subscription_id: subscription.id,
            count,
        }
        .payload(),
        subscription_id: Some(subscription.id),
    }
}

fn traffic_item(subscription: &Subscription, gb: u32) -> ItemRequest {
    ItemRequest {
        module: ModuleId::Options,
        kind: ITEM_TRAFFIC.into(),
        payload: TrafficRequest {
            subscription_id: subscription.id,
            gb,
        }
        .payload(),
        subscription_id: Some(subscription.id),
    }
}

async fn buy_item(kit: &Kit, client_id: ClientId, item: ItemRequest) -> Order {
    let result = kit
        .services
        .checkout()
        .start_item(
            client_id,
            &ItemCheckoutRequest {
                item,
                provider: providers::BALANCE.into(),
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Completed { order_id } = result else {
        panic!("оплата с баланса должна проходить сразу: {result:?}");
    };
    let order = order_by_public_id(&kit.services, order_id).await;
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    self::order(&kit.services, order.id).await
}

async fn bought(kit: &Kit, subscription: &Subscription) -> Option<ExtraTraffic> {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.subscriptions()
        .extra_traffic(subscription.id)
        .await
        .unwrap()
}

async fn renew_with_balance(kit: &Kit, client_id: ClientId, tariff: &Tariff, sub: &Subscription) {
    let renewal = buy_with_balance(&kit.services, client_id, cart(tariff, 30, Some(sub))).await;
    let done = kit.services.fulfillment().run(renewal.id).await.unwrap();
    assert!(matches!(done, FulfillmentOutcome::Done { .. }), "{done:?}");
}

#[tokio::test]
async fn устройства_до_конца_подписки_стоят_по_дням_и_сразу_работают() {
    let Some(kit) = options_kit().await else {
        return;
    };
    let client_id = client(&kit.services, 801).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let subscription = expire_in(
        &kit,
        &subscription,
        Utc::now() + Days::days(14) + Days::hours(12),
        SubscriptionStatus::Active,
    )
    .await;
    let order = buy_item(&kit, client_id, devices_item(&subscription, 2)).await;
    assert_eq!(order.quote.total.minor, 99);
    assert_eq!(order.status, OrderStatus::Fulfilled);
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.hwid_device_limit, Some(5));
    let mut conn = kit.services.store().conn().await.unwrap();
    let stored = conn.subscriptions().get(subscription.id).await.unwrap();
    drop(conn);
    assert_eq!(stored.extra_devices, 2);
    let calls = kit.remna.calls_of("update_user").len();
    let again = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(again, FulfillmentOutcome::Done { .. }),
        "{again:?}"
    );
    assert_eq!(kit.remna.calls_of("update_user").len(), calls);
    let audit = {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.audit()
            .for_entity("subscription", subscription.id.0, 20)
            .await
            .unwrap()
    };
    assert_eq!(audit[0].action, crate::subscriptions::actions::DEVICE_ADDON);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пакет_трафика_включает_подписку_а_остаток_переносится_при_продлении() {
    let Some(kit) = options_kit().await else {
        return;
    };
    let client_id = client(&kit.services, 802).await;
    let tariff = limited(&kit, TrafficReset::OnPayment, TrafficLeftover::Carry).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let subscription = set_user(&kit, &subscription, |user| {
        user.user_traffic.used_traffic_bytes = (100 * GIB) as i64;
        user.status = UserStatus::Limited;
    })
    .await;
    let order = buy_item(&kit, client_id, traffic_item(&subscription, 10)).await;
    assert_eq!(order.quote.total.minor, 150);
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.traffic_limit_bytes, (110 * GIB) as i64);
    assert_eq!(user.status, UserStatus::Active);
    let state = bought(&kit, &subscription).await.unwrap();
    assert_eq!(state.bytes, 10 * GIB);
    assert_eq!(state.window_ends_at, None);
    let again = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(again, FulfillmentOutcome::Done { .. }),
        "{again:?}"
    );
    assert_eq!(
        remna_user(&kit, &subscription).await.traffic_limit_bytes,
        (110 * GIB) as i64
    );
    let subscription = set_user(&kit, &subscription, |user| {
        user.user_traffic.used_traffic_bytes = (105 * GIB) as i64;
    })
    .await;
    renew_with_balance(&kit, client_id, &tariff, &subscription).await;
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.traffic_limit_bytes, (105 * GIB) as i64);
    assert_eq!(user.user_traffic.used_traffic_bytes, 0);
    assert_eq!(bought(&kit, &subscription).await.unwrap().bytes, 5 * GIB);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn при_сгорании_продление_возвращает_лимит_тарифа() {
    let Some(kit) = options_kit().await else {
        return;
    };
    let client_id = client(&kit.services, 803).await;
    let tariff = limited(&kit, TrafficReset::OnPayment, TrafficLeftover::Burn).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    buy_item(&kit, client_id, traffic_item(&subscription, 10)).await;
    let subscription = set_user(&kit, &subscription, |user| {
        user.user_traffic.used_traffic_bytes = (40 * GIB) as i64;
    })
    .await;
    renew_with_balance(&kit, client_id, &tariff, &subscription).await;
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.traffic_limit_bytes, (100 * GIB) as i64);
    assert!(bought(&kit, &subscription).await.is_none());
    kit.db.cleanup().await;
}

async fn bought_before_reset(
    kit: &Kit,
    telegram_id: i64,
    leftover: TrafficLeftover,
) -> Subscription {
    let client_id = client(&kit.services, telegram_id).await;
    let tariff = limited(kit, TrafficReset::Month, leftover).await;
    let subscription = provision(kit, client_id, &tariff, 30).await;
    let subscription = set_user(kit, &subscription, |user| {
        user.user_traffic.used_traffic_bytes = (100 * GIB) as i64;
        user.user_traffic.lifetime_used_traffic_bytes = (100 * GIB) as i64;
        user.last_traffic_reset_at = Some(Utc::now() - Days::days(10));
        user.status = UserStatus::Limited;
    })
    .await;
    buy_item(kit, client_id, traffic_item(&subscription, 10)).await;
    set_user(kit, &subscription, |user| {
        user.user_traffic.used_traffic_bytes = (105 * GIB) as i64;
        user.user_traffic.lifetime_used_traffic_bytes = (105 * GIB) as i64;
    })
    .await
}

async fn reset_by_panel(kit: &Kit, subscription: &Subscription) {
    set_user(kit, subscription, |user| {
        user.user_traffic.used_traffic_bytes = (3 * GIB) as i64;
        user.user_traffic.lifetime_used_traffic_bytes = (108 * GIB) as i64;
        user.last_traffic_reset_at = Some(Utc::now());
    })
    .await;
}

#[tokio::test]
async fn после_сброса_панели_докупленное_сгорает() {
    let Some(kit) = options_kit().await else {
        return;
    };
    let subscription = bought_before_reset(&kit, 804, TrafficLeftover::Burn).await;
    let state = bought(&kit, &subscription).await.unwrap();
    let ends = state
        .window_ends_at
        .expect("у месячного сброса есть конец окна");
    assert_eq!(state.window_lifetime, Some(0));
    let ops = kit.services.subscriptions();
    assert_eq!(
        ops.settle_extra_traffic(ends - Days::minutes(1))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        ops.settle_extra_traffic(ends + Days::minutes(1))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        remna_user(&kit, &subscription).await.traffic_limit_bytes,
        (110 * GIB) as i64
    );
    reset_by_panel(&kit, &subscription).await;
    assert_eq!(
        ops.settle_extra_traffic(ends + Days::minutes(2))
            .await
            .unwrap(),
        1
    );
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.traffic_limit_bytes, (100 * GIB) as i64);
    assert!(bought(&kit, &subscription).await.is_none());
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_сброса_панели_остаток_переносится_в_новый_период() {
    let Some(kit) = options_kit().await else {
        return;
    };
    let subscription = bought_before_reset(&kit, 805, TrafficLeftover::Carry).await;
    let ends = bought(&kit, &subscription)
        .await
        .unwrap()
        .window_ends_at
        .unwrap();
    reset_by_panel(&kit, &subscription).await;
    let ops = kit.services.subscriptions();
    assert_eq!(
        ops.settle_extra_traffic(ends + Days::minutes(1))
            .await
            .unwrap(),
        1
    );
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.traffic_limit_bytes, (105 * GIB) as i64);
    let state = bought(&kit, &subscription).await.unwrap();
    assert_eq!(state.bytes, 5 * GIB);
    assert_eq!(state.window_lifetime, Some(105 * GIB));
    assert!(state.window_ends_at.unwrap() > ends);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ручной_лимит_админа_снимает_докупленный_остаток() {
    let Some(kit) = options_kit().await else {
        return;
    };
    let client_id = client(&kit.services, 806).await;
    let tariff = limited(&kit, TrafficReset::OnPayment, TrafficLeftover::Carry).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    buy_item(&kit, client_id, traffic_item(&subscription, 10)).await;
    assert!(bought(&kit, &subscription).await.is_some());
    kit.services
        .subscriptions()
        .change_limits(
            subscription.id,
            &crate::subscriptions::LimitsChange {
                traffic_limit_bytes: Some(Some(200 * GIB)),
                extra_devices: None,
            },
            crate::Actor::Admin(sn_domain::ids::AdminId(1)),
        )
        .await
        .unwrap();
    assert!(bought(&kit, &subscription).await.is_none());
    kit.db.cleanup().await;
}
