use super::testkit::*;
use super::*;
use sn_domain::payment::PaymentChannels;
use sn_domain::settings::keys;
use sn_domain::traffic::TrafficPolicy;

fn code(err: ServiceError) -> String {
    match err {
        ServiceError::Rejected { code, .. } => code.to_string(),
        other => format!("{other:?}"),
    }
}

fn ids(view: &QuoteView) -> Vec<&'static str> {
    view.methods.iter().map(|m| m.provider).collect()
}

#[tokio::test]
async fn способ_оплаты_выключенный_в_канале_нельзя_выбрать_даже_напрямую() {
    let Some(kit) = kit().await else { return };
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(TEST_PAY),
                &serde_json::json!({
                    "enabled": true,
                    "channels": {"cabinet": true, "bot": false, "miniapp": false}
                }),
                None,
            )
            .await
            .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
    let client_id = client(&kit.services, 5_093_118_422).await;
    let category_id = category(&kit.services, "VPN").await;
    let tariff = tariff(
        &kit.services,
        category_id,
        "Стандарт",
        TrafficPolicy::UNLIMITED,
        vec![squad(1)],
    )
    .await;
    let cart = cart(&tariff, 90, None);
    let checkout = kit.services.checkout();

    let in_bot = checkout
        .quote_in(client_id, &cart, Channel::Bot)
        .await
        .unwrap();
    assert!(!ids(&in_bot).contains(&TEST_PAY));
    let in_mini = checkout
        .quote_for(client_id, &cart, None, Some(ClientChannel::Miniapp))
        .await
        .unwrap();
    assert!(!ids(&in_mini).contains(&TEST_PAY));
    let in_cabinet = checkout
        .quote_in(client_id, &cart, Channel::Cabinet)
        .await
        .unwrap();
    assert!(ids(&in_cabinet).contains(&TEST_PAY));
    assert!(ids(&checkout.quote(client_id, &cart).await.unwrap()).contains(&TEST_PAY));

    let request = |channel: Channel| CheckoutRequest {
        cart: cart.clone(),
        provider: TEST_PAY.into(),
        channel,
        return_url: None,
    };
    assert_eq!(
        code(
            checkout
                .start(client_id, &request(Channel::Bot))
                .await
                .unwrap_err()
        ),
        "checkout.method_unavailable"
    );
    assert_eq!(
        code(
            checkout
                .start(client_id, &request(Channel::Miniapp))
                .await
                .unwrap_err()
        ),
        "checkout.method_unavailable"
    );
    assert!(matches!(
        checkout
            .start(client_id, &request(Channel::Cabinet))
            .await
            .unwrap(),
        CheckoutResult::Redirect { .. }
    ));
    assert!(matches!(
        checkout
            .start(client_id, &request(Channel::Admin))
            .await
            .unwrap(),
        CheckoutResult::Redirect { .. }
    ));

    let amount = Money::new(1_000, Currency::Usd);
    assert_eq!(
        code(
            checkout
                .topup(client_id, amount, TEST_PAY, Channel::Bot)
                .await
                .unwrap_err()
        ),
        "checkout.method_unavailable"
    );
    assert!(
        checkout
            .topup(client_id, amount, TEST_PAY, Channel::Cabinet)
            .await
            .is_ok()
    );
    let views = kit.services.payments().views();
    let view = views.iter().find(|v| v.id == TEST_PAY).unwrap();
    assert_eq!(
        view.channels,
        PaymentChannels {
            cabinet: true,
            bot: false,
            miniapp: false
        }
    );
    kit.db.cleanup().await;
}
