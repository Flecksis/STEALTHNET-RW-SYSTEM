use super::testkit::*;
use super::*;
use crate::autorenew::AutoRenewReport;
use crate::fulfillment::FulfillmentOutcome;
use sn_domain::money::Stars;
use sn_domain::pricing::{QuoteLine, QuoteLineKind};
use sn_domain::tariff::DeviceDiscount;
use sn_domain::traffic::TrafficPolicy;
use sn_modules::Module;
use sn_modules::catalog::options::{DevicesRequest, ITEM_DEVICES, OptionsModule};
use sn_modules::catalog::promo::{self, NewPromo, NewPromoValue, PromoModule};

async fn options_kit(with_promo: bool) -> Option<Kit> {
    let mut modules: Vec<Arc<dyn Module>> = vec![Arc::new(OptionsModule)];
    if with_promo {
        modules.push(Arc::new(PromoModule));
    }
    kit_with(modules).await
}

async fn with_ladder(kit: &Kit, tariff: &Tariff) -> Tariff {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.tariffs()
        .set_device_discounts(
            tariff.id,
            &[DeviceDiscount {
                from: 2,
                percent: 10,
            }],
        )
        .await
        .unwrap();
    conn.tariffs().get(tariff.id).await.unwrap()
}

fn with_devices(cart: CartRequest, extra_devices: u32) -> CartRequest {
    CartRequest {
        extra_devices,
        ..cart
    }
}

fn discount_line(quote: &Quote) -> Option<&QuoteLine> {
    quote
        .lines
        .iter()
        .find(|line| matches!(line.kind, QuoteLineKind::DeviceDiscount { .. }))
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

async fn delivered(kit: &Kit, order: &Order) -> Subscription {
    let outcome = kit.services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = super::testkit::order(&kit.services, order.id).await;
    subscription(&kit.services, &order).await
}

#[tokio::test]
async fn покупка_с_доп_устройствами_получает_скидку_и_в_звёздах() {
    let Some(kit) = options_kit(false).await else {
        return;
    };
    let client_id = client(&kit.services, 951).await;
    let tariff = with_ladder(&kit, &standard(&kit.services).await).await;
    let cart = with_devices(cart(&tariff, 30, None), 2);
    let view = kit
        .services
        .checkout()
        .quote(client_id, &cart)
        .await
        .unwrap();
    let line = discount_line(&view.quote).expect("строка скидки за устройства");
    assert_eq!(
        line.kind,
        QuoteLineKind::DeviceDiscount {
            count: 2,
            percent: 10
        }
    );
    assert_eq!(line.amount, Money::new(-20, Currency::Usd));
    assert_eq!(line.stars, Some(Stars(-10)));
    assert_eq!(view.quote.total, Money::new(299 + 198 - 20, Currency::Usd));
    assert_eq!(
        view.quote.amount_in(PayCurrency::Xtr),
        Some(Stars(150 + 100 - 10).into())
    );
    let one = kit
        .services
        .checkout()
        .quote(client_id, &with_devices(cart.clone(), 1))
        .await
        .unwrap();
    assert!(discount_line(&one.quote).is_none());
    credit(&kit.services, client_id, 10_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart).await;
    assert_eq!(order.quote.total, view.quote.total);
    assert_eq!(balance(&kit.services, client_id).await, 10_000 - 477);
    let bought = delivered(&kit, &order).await;
    assert_eq!(bought.extra_devices, 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn продление_и_смена_тарифа_с_доп_устройствами_получают_скидку() {
    let Some(kit) = options_kit(false).await else {
        return;
    };
    let client_id = client(&kit.services, 952).await;
    let tariff = with_ladder(&kit, &standard(&kit.services).await).await;
    credit(&kit.services, client_id, 10_000).await;
    let first = buy_with_balance(
        &kit.services,
        client_id,
        with_devices(cart(&tariff, 30, None), 2),
    )
    .await;
    let subscription = delivered(&kit, &first).await;
    let renewal = kit
        .services
        .checkout()
        .quote(
            client_id,
            &with_devices(cart(&tariff, 90, Some(&subscription)), 2),
        )
        .await
        .unwrap();
    assert_eq!(renewal.kind, PlanKind::Renewal);
    assert_eq!(
        discount_line(&renewal.quote).map(|l| l.amount),
        Some(Money::new(-59, Currency::Usd))
    );
    assert_eq!(
        renewal.quote.total,
        Money::new(699 + 594 - 59, Currency::Usd)
    );
    let premium_category = category(&kit.services, "Премиум").await;
    let premium = tariff_in(&kit, premium_category).await;
    let premium = with_ladder(&kit, &premium).await;
    let change = kit
        .services
        .checkout()
        .quote(
            client_id,
            &with_devices(cart(&premium, 30, Some(&subscription)), 2),
        )
        .await
        .unwrap();
    assert_eq!(change.kind, PlanKind::Change);
    assert_eq!(
        discount_line(&change.quote).map(|l| l.amount),
        Some(Money::new(-20, Currency::Usd))
    );
    kit.db.cleanup().await;
}

async fn tariff_in(kit: &Kit, category_id: CategoryId) -> Tariff {
    tariff(
        &kit.services,
        category_id,
        "Премиум",
        TrafficPolicy::UNLIMITED,
        vec![squad(3)],
    )
    .await
}

#[tokio::test]
async fn автопродление_списывает_сумму_со_скидкой_за_устройства() {
    let Some(kit) = options_kit(false).await else {
        return;
    };
    let client_id = client(&kit.services, 953).await;
    let tariff = with_ladder(&kit, &standard(&kit.services).await).await;
    credit(&kit.services, client_id, 10_000).await;
    let first = buy_with_balance(
        &kit.services,
        client_id,
        with_devices(cart(&tariff, 90, None), 2),
    )
    .await;
    let subscription = delivered(&kit, &first).await;
    assert!(subscription.autorenew.enabled);
    let subscription = expire_in(
        &kit,
        &subscription,
        moment(chrono::Duration::hours(12)),
        SubscriptionStatus::Active,
    )
    .await;
    let before = balance(&kit.services, client_id).await;
    let report = kit
        .services
        .autorenew()
        .run_due(kit.services.now())
        .await
        .unwrap();
    assert_eq!(
        report,
        AutoRenewReport {
            due: 1,
            charged: 1,
            reminded: 0,
            failed: 0
        }
    );
    let total = 699 + 594 - 59;
    assert_eq!(balance(&kit.services, client_id).await, before - total);
    let mut conn = kit.services.store().conn().await.unwrap();
    let renewal = conn
        .orders()
        .for_client(client_id, Default::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|o| o.kind == OrderKind::Renewal && o.subscription_id == Some(subscription.id))
        .expect("заказ автопродления");
    drop(conn);
    assert_eq!(renewal.quote.total.minor, total);
    assert_eq!(
        discount_line(&renewal.quote).map(|l| l.kind.clone()),
        Some(QuoteLineKind::DeviceDiscount {
            count: 2,
            percent: 10
        })
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn докупка_устройств_без_продления_со_скидкой() {
    let Some(kit) = options_kit(false).await else {
        return;
    };
    let client_id = client(&kit.services, 954).await;
    let tariff = with_ladder(&kit, &standard(&kit.services).await).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let checkout = kit.services.checkout();
    let single = checkout
        .quote_item(client_id, &devices_item(&subscription, 1))
        .await
        .unwrap();
    assert!(discount_line(&single.quote).is_none());
    let view = checkout
        .quote_item(client_id, &devices_item(&subscription, 2))
        .await
        .unwrap();
    let addon = view.quote.lines[0].amount;
    let line = discount_line(&view.quote).expect("строка скидки за устройства");
    assert_eq!(
        line.kind,
        QuoteLineKind::DeviceDiscount {
            count: 2,
            percent: 10
        }
    );
    let cut = addon.percent(10).unwrap();
    assert_eq!(line.amount, cut.checked_neg().unwrap());
    assert_eq!(view.quote.total, addon.checked_sub(cut).unwrap());
    let stars = view.quote.stars.unwrap();
    assert_eq!(
        stars.total,
        stars.subtotal.checked_sub(stars.discount).unwrap()
    );
    assert!(stars.discount.0 > 0);
    let before = balance(&kit.services, client_id).await;
    let result = checkout
        .start_item(
            client_id,
            &ItemCheckoutRequest {
                item: devices_item(&subscription, 2),
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let CheckoutResult::Completed { order_id } = result else {
        panic!("оплата с баланса проходит сразу: {result:?}");
    };
    let order = order_by_public_id(&kit.services, order_id).await;
    assert_eq!(order.quote.total, view.quote.total);
    assert_eq!(
        balance(&kit.services, client_id).await,
        before - view.quote.total.minor
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn промокод_считается_от_суммы_после_скидки_за_устройства() {
    let Some(kit) = options_kit(true).await else {
        return;
    };
    promo::create(
        &kit.services.module_context(),
        &NewPromo {
            code: "ZIMA10".into(),
            value: NewPromoValue::Percent { percent: 10 },
            max_uses: None,
            per_client: 1,
            valid_from: None,
            valid_until: None,
            first_purchase_only: false,
            tariff_ids: Vec::new(),
            note: None,
        },
        None,
    )
    .await
    .unwrap();
    let client_id = client(&kit.services, 955).await;
    let tariff = with_ladder(&kit, &standard(&kit.services).await).await;
    let mut cart = with_devices(cart(&tariff, 30, None), 2);
    cart.promo_code = Some("ZIMA10".into());
    let view = kit
        .services
        .checkout()
        .quote(client_id, &cart)
        .await
        .unwrap();
    let kinds: Vec<&str> = view
        .quote
        .lines
        .iter()
        .map(|line| match line.kind {
            QuoteLineKind::Period { .. } => "period",
            QuoteLineKind::ExtraDevices { .. } => "devices",
            QuoteLineKind::DeviceDiscount { .. } => "device_discount",
            QuoteLineKind::Adjustment { .. } => "promo",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["period", "devices", "device_discount", "promo"]);
    assert_eq!(view.quote.lines[3].amount, Money::new(-48, Currency::Usd));
    assert_eq!(view.quote.total, Money::new(477 - 48, Currency::Usd));
    assert_eq!(view.quote.discount, Money::new(20 + 48, Currency::Usd));
    assert_eq!(view.quote.stars.unwrap().total, Stars(240 - 24));
    credit(&kit.services, client_id, 10_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart).await;
    assert_eq!(order.quote.total, view.quote.total);
    kit.db.cleanup().await;
}
