use super::testkit::*;
use super::*;
use crate::clients::GrantRequest;
use crate::inbox::saved_method_context;
use sn_domain::ids::AdminId;
use sn_modules::catalog::options::{DevicesRequest, ITEM_DEVICES, OptionsModule};
use sn_modules::{BalanceChange, CoreApi, Module};
use sn_payments::ProviderStatus;

async fn channel_of(services: &Services, order: &Order) -> Option<OrderChannel> {
    let mut conn = services.store().conn().await.unwrap();
    conn.orders().channel(order.id).await.unwrap()
}

fn sale(provider: &str, cart: CartRequest, channel: Channel) -> CheckoutRequest {
    CheckoutRequest {
        cart,
        provider: provider.into(),
        method: None,
        channel,
        return_url: None,
    }
}

async fn placed(services: &Services, result: CheckoutResult) -> Order {
    let (CheckoutResult::Redirect { order_id, .. } | CheckoutResult::Completed { order_id }) =
        result;
    order_by_public_id(services, order_id).await
}

async fn renewal(services: &Services, client_id: ClientId) -> Order {
    let mut conn = services.store().conn().await.unwrap();
    conn.orders()
        .for_client(client_id, Default::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|o| o.kind == OrderKind::Renewal)
        .expect("заказа автопродления нет")
}

#[tokio::test]
async fn канал_покупки_сохраняется_у_подписки_пополнения_и_товара_модуля() {
    let modules: Vec<Arc<dyn Module>> = vec![Arc::new(OptionsModule)];
    let Some(kit) = kit_with(modules).await else {
        return;
    };
    let services = &kit.services;
    let tariff = standard(services).await;
    let checkout = services.checkout();

    let in_bot = client(services, 7_301).await;
    credit(services, in_bot, 1_000).await;
    let bought = checkout
        .start(
            in_bot,
            &sale(providers::BALANCE, cart(&tariff, 30, None), Channel::Bot),
        )
        .await
        .unwrap();
    let order = placed(services, bought).await;
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(channel_of(services, &order).await, Some(OrderChannel::Bot));

    let in_mini = client(services, 7_302).await;
    let invoiced = checkout
        .start(
            in_mini,
            &sale(TEST_PAY, cart(&tariff, 90, None), Channel::Miniapp),
        )
        .await
        .unwrap();
    assert!(
        matches!(invoiced, CheckoutResult::Redirect { .. }),
        "{invoiced:?}"
    );
    let order = placed(services, invoiced).await;
    assert_eq!(order.status, OrderStatus::Pending);
    assert_eq!(
        channel_of(services, &order).await,
        Some(OrderChannel::Miniapp)
    );

    let for_client = client(services, 7_303).await;
    let order = placed(
        services,
        checkout
            .start(
                for_client,
                &sale(TEST_PAY, cart(&tariff, 30, None), Channel::Admin),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        channel_of(services, &order).await,
        Some(OrderChannel::Admin)
    );

    let topped = checkout
        .topup(
            in_mini,
            Money::new(1_000, Currency::Usd),
            TEST_PAY,
            None,
            Channel::Cabinet,
        )
        .await
        .unwrap();
    let order = placed(services, topped).await;
    assert_eq!(order.kind, OrderKind::TopUp);
    assert_eq!(
        channel_of(services, &order).await,
        Some(OrderChannel::Cabinet)
    );

    let owner = client(services, 7_304).await;
    let subscription = provision(&kit, owner, &tariff, 90).await;
    let item = ItemRequest {
        module: ModuleId::Options,
        kind: ITEM_DEVICES.into(),
        payload: DevicesRequest {
            subscription_id: subscription.id,
            count: 1,
        }
        .payload(),
        subscription_id: Some(subscription.id),
    };
    let addon = checkout
        .start_item(
            owner,
            &ItemCheckoutRequest {
                item,
                provider: providers::BALANCE.into(),
                method: None,
                channel: Channel::Miniapp,
                return_url: None,
            },
        )
        .await
        .unwrap();
    let order = placed(services, addon).await;
    assert_eq!(order.kind, OrderKind::Module);
    assert_eq!(
        channel_of(services, &order).await,
        Some(OrderChannel::Miniapp)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn выдача_из_админки_и_автопродление_помечены_своим_каналом() {
    let Some(kit) = kit().await else { return };
    let services = &kit.services;
    let tariff = standard(services).await;

    let granted_to = client(services, 7_311).await;
    let granted = services
        .clients()
        .grant(
            granted_to,
            &GrantRequest {
                tariff_id: tariff.id,
                period_id: Some(tariff.periods[0].id),
                days: None,
                subscription_id: None,
                note: None,
            },
            Actor::Admin(AdminId(1)),
        )
        .await
        .unwrap();
    let order = order_by_public_id(services, granted.order_id).await;
    assert_eq!(order.kind, OrderKind::AdminGrant);
    assert_eq!(
        channel_of(services, &order).await,
        Some(OrderChannel::Admin)
    );

    let from_balance = client(services, 7_312).await;
    let first = provision(&kit, from_balance, &tariff, 90).await;
    expire_in(
        &kit,
        &first,
        moment(Duration::hours(12)),
        SubscriptionStatus::Active,
    )
    .await;
    let from_card = client(services, 7_313).await;
    let second = provision(&kit, from_card, &tariff, 90).await;
    expire_in(
        &kit,
        &second,
        moment(Duration::hours(12)),
        SubscriptionStatus::Active,
    )
    .await;
    let left = balance(services, from_card).await;
    services
        .change_balance(BalanceChange {
            client_id: from_card,
            wallet: Wallet::Main,
            amount: Money::new(100 - left, Currency::Usd),
            kind: LedgerKind::Purchase,
            comment: None,
            idempotency_key: format!("drain:{}", Uuid::new_v4()),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
    let sealed = services
        .secrets()
        .unwrap()
        .seal_str(&saved_method_context(from_card, TEST_PAY), "pm-1")
        .unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.saved_methods()
            .save(from_card, TEST_PAY, &sealed, Some("Visa •• 4242"))
            .await
            .unwrap();
    }
    *kit.pay.charge.lock().unwrap() = Some(ProviderStatus::Paid);
    let report = services.autorenew().run_due(services.now()).await.unwrap();
    assert_eq!((report.due, report.charged, report.failed), (2, 2, 0));
    for client_id in [from_balance, from_card] {
        let order = renewal(services, client_id).await;
        assert_eq!(
            channel_of(services, &order).await,
            Some(OrderChannel::Autorenew)
        );
    }
    let mut conn = services.store().conn().await.unwrap();
    let bought = conn
        .orders()
        .for_client(from_balance, Default::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|o| o.kind == OrderKind::New)
        .unwrap();
    assert_eq!(
        conn.orders().channel(bought.id).await.unwrap(),
        Some(OrderChannel::Cabinet)
    );
    drop(conn);
    kit.db.cleanup().await;
}
