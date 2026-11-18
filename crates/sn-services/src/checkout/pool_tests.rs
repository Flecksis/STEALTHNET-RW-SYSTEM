use super::testkit::*;
use super::*;
use crate::SystemClock;
use crate::services::{EnvFallbacks, ServiceDeps};
use sn_modules::Module;
use sn_modules::catalog::options::{DevicesRequest, ITEM_DEVICES, OptionsModule};
use sn_modules::catalog::promo::{self, NewPromo, NewPromoValue, PromoModule};
use sn_payments::ProviderRegistry;
use sn_payments::providers::Balance;
use sn_store::{Store, StoreOptions};
use std::time::{Duration as Wait, Instant};

const PARALLEL: i64 = 12;

fn percent(code: &str, percent: u8) -> NewPromo {
    NewPromo {
        code: code.into(),
        value: NewPromoValue::Percent { percent },
        max_uses: None,
        per_client: 1,
        valid_from: None,
        valid_until: None,
        first_purchase_only: false,
        tariff_ids: Vec::new(),
        note: None,
    }
}

fn modules() -> Vec<Arc<dyn Module>> {
    vec![Arc::new(PromoModule), Arc::new(OptionsModule)]
}

async fn pooled(kit: &Kit, connections: u32, acquire_timeout: Wait) -> (Store, Services) {
    let url = sn_store::testing::test_database_url().unwrap();
    let schema: String = {
        let mut conn = kit.services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT current_schema()")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    let store = Store::connect(
        &url,
        StoreOptions {
            max_connections: connections,
            acquire_timeout,
            search_path: Some(schema),
            ..StoreOptions::default()
        },
    )
    .await
    .unwrap();
    let services = Services::new(ServiceDeps {
        store: store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(TestPay::default()),
            Arc::new(Balance),
        ])),
        modules: Arc::new(ModuleRegistry::new(modules()).unwrap()),
        secrets: kit.services.secrets().cloned(),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    services.set_remna(Some(kit.remna.clone()));
    (store, services)
}

fn devices(subscription: &Subscription, count: u32) -> ItemRequest {
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

fn promo_cart(tariff: &Tariff, code: &str) -> CartRequest {
    let mut cart = cart(tariff, 30, None);
    cart.promo_code = Some(code.into());
    cart
}

#[tokio::test]
async fn одновременные_расчёты_с_промокодом_не_исчерпывают_пул_соединений() {
    let Some(kit) = kit_with(modules()).await else {
        return;
    };
    promo::create(&kit.services.module_context(), &percent("SALE10", 10), None)
        .await
        .unwrap();
    let tariff = standard(&kit.services).await;
    let mut clients = Vec::new();
    for n in 0..PARALLEL {
        clients.push(client(&kit.services, 7_700 + n).await);
    }
    let pool_timeout = StoreOptions::default().acquire_timeout;
    let (store, services) = pooled(&kit, 4, pool_timeout).await;
    let cart = promo_cart(&tariff, "SALE10");
    let started = Instant::now();
    let mut handles = Vec::new();
    for client_id in clients {
        let services = services.clone();
        let cart = cart.clone();
        handles.push(tokio::spawn(async move {
            services.checkout().quote(client_id, &cart).await
        }));
    }
    let mut totals = Vec::new();
    for handle in handles {
        let view = handle
            .await
            .unwrap()
            .unwrap_or_else(|err| panic!("расчёт с промокодом не прошёл: {err:?}"));
        totals.push(view.quote.total);
    }
    let elapsed = started.elapsed();
    assert_eq!(
        totals,
        vec![Money::new(269, Currency::Usd); PARALLEL as usize]
    );
    assert!(
        elapsed < pool_timeout / 2,
        "расчёты ждали соединения: {elapsed:?}"
    );
    store.close().await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn одновременные_расчёты_доп_устройств_не_исчерпывают_пул_соединений() {
    let Some(kit) = kit_with(modules()).await else {
        return;
    };
    let tariff = standard(&kit.services).await;
    let client_id = client(&kit.services, 7_800).await;
    let subscription = provision(&kit, client_id, &tariff, 90).await;
    let pool_timeout = StoreOptions::default().acquire_timeout;
    let (store, services) = pooled(&kit, 4, pool_timeout).await;
    let item = devices(&subscription, 1);
    let started = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..PARALLEL {
        let services = services.clone();
        let item = item.clone();
        handles.push(tokio::spawn(async move {
            services.checkout().quote_item(client_id, &item).await
        }));
    }
    let mut totals = Vec::new();
    for handle in handles {
        let view = handle
            .await
            .unwrap()
            .unwrap_or_else(|err| panic!("расчёт доп. устройств не прошёл: {err:?}"));
        totals.push(view.quote.total);
    }
    let elapsed = started.elapsed();
    assert_eq!(
        totals,
        vec![Money::new(297, Currency::Usd); PARALLEL as usize]
    );
    assert!(
        elapsed < pool_timeout / 2,
        "расчёты ждали соединения: {elapsed:?}"
    );
    store.close().await;
    kit.db.cleanup().await;
}

#[tokio::test]
async fn расчёт_и_оформление_обходятся_одним_соединением() {
    let Some(kit) = kit_with(modules()).await else {
        return;
    };
    promo::create(&kit.services.module_context(), &percent("SALE10", 10), None)
        .await
        .unwrap();
    let tariff = standard(&kit.services).await;
    let by_card = client(&kit.services, 7_900).await;
    let by_balance = client(&kit.services, 7_901).await;
    credit(&kit.services, by_balance, 1_000).await;
    let owner = client(&kit.services, 7_902).await;
    let subscription = provision(&kit, owner, &tariff, 90).await;
    let (store, services) = pooled(&kit, 1, Wait::from_secs(2)).await;
    let checkout = services.checkout();
    let cart = promo_cart(&tariff, "SALE10");
    let view = checkout.quote(by_card, &cart).await.unwrap();
    assert_eq!(view.quote.total.minor, 269);
    let invoiced = checkout
        .start(
            by_card,
            &CheckoutRequest {
                cart: cart.clone(),
                provider: TEST_PAY.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(invoiced, CheckoutResult::Redirect { .. }),
        "{invoiced:?}"
    );
    let paid = checkout
        .start(
            by_balance,
            &CheckoutRequest {
                cart,
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(paid, CheckoutResult::Completed { .. }), "{paid:?}");
    let item = devices(&subscription, 1);
    let priced = checkout.quote_item(owner, &item).await.unwrap();
    assert_eq!(priced.quote.total.minor, 297);
    let bought = checkout
        .start_item(
            owner,
            &ItemCheckoutRequest {
                item,
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Cabinet,
                return_url: None,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(bought, CheckoutResult::Completed { .. }),
        "{bought:?}"
    );
    store.close().await;
    kit.db.cleanup().await;
}
