use super::testkit::*;
use super::*;
use sn_domain::settings::keys;

fn code(err: ServiceError) -> String {
    match err {
        ServiceError::Rejected { code, .. } => code.to_string(),
        other => format!("{other:?}"),
    }
}

fn ids(view: &QuoteView) -> Vec<&'static str> {
    view.methods.iter().map(|m| m.provider).collect()
}

pub(crate) async fn balance_off(kit: &Kit) {
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::BALANCE),
                &serde_json::json!({"enabled": false}),
                None,
            )
            .await
            .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
}

#[tokio::test]
async fn выключенный_баланс_не_пополняется_а_остаток_можно_потратить() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 5_093_118_501).await;
    let tariff = standard(&kit.services).await;
    let cart = cart(&tariff, 90, None);
    credit(&kit.services, client_id, 500).await;
    assert!(kit.services.payments().balance_enabled());
    assert!(kit.services.checkout().topup_open().is_ok());
    balance_off(&kit).await;
    assert!(!kit.services.payments().balance_enabled());
    let checkout = kit.services.checkout();

    let short = checkout
        .quote_in(client_id, &cart, Channel::Cabinet)
        .await
        .unwrap();
    assert!(!ids(&short).contains(&providers::BALANCE));
    assert!(ids(&short).contains(&TEST_PAY));
    let pay_from_balance = CheckoutRequest {
        cart: cart.clone(),
        provider: providers::BALANCE.into(),
        channel: Channel::Cabinet,
        return_url: None,
    };
    assert_eq!(
        code(
            checkout
                .start(client_id, &pay_from_balance)
                .await
                .unwrap_err()
        ),
        BALANCE_OFF
    );
    assert_eq!(code(checkout.topup_open().unwrap_err()), BALANCE_OFF);
    for channel in [
        Channel::Bot,
        Channel::Cabinet,
        Channel::Miniapp,
        Channel::Admin,
    ] {
        assert_eq!(
            code(
                checkout
                    .topup(
                        client_id,
                        Money::new(1_000, Currency::Usd),
                        TEST_PAY,
                        channel
                    )
                    .await
                    .unwrap_err()
            ),
            BALANCE_OFF
        );
    }
    assert!(matches!(
        checkout
            .start(
                client_id,
                &CheckoutRequest {
                    provider: TEST_PAY.into(),
                    ..pay_from_balance.clone()
                }
            )
            .await
            .unwrap(),
        CheckoutResult::Redirect { .. }
    ));

    credit(&kit.services, client_id, 300).await;
    let enough = checkout
        .quote_in(client_id, &cart, Channel::Bot)
        .await
        .unwrap();
    let leftover = enough
        .methods
        .iter()
        .find(|m| m.provider == providers::BALANCE)
        .unwrap();
    assert_eq!(leftover.unavailable, None);
    let order = buy_with_balance(&kit.services, client_id, cart.clone()).await;
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(balance(&kit.services, client_id).await, 101);

    let view = kit.services.settings_ops().payments().await.unwrap();
    assert_eq!(view.balances.clients, 1);
    assert_eq!(view.balances.total, Money::new(101, Currency::Usd));
    let balance_view = view
        .providers
        .iter()
        .find(|p| p.id == providers::BALANCE)
        .unwrap();
    assert!(!balance_view.enabled);
    kit.db.cleanup().await;
}
