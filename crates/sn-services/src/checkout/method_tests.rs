use super::testkit::*;
use super::*;
use crate::settings::ProviderUpdate;
use sn_domain::ids::AdminId;
use sn_domain::payment::MethodChoice;
use sn_domain::traffic::TrafficPolicy;

fn code(err: ServiceError) -> String {
    match err {
        ServiceError::Rejected { code, .. } => code.to_string(),
        other => format!("{other:?}"),
    }
}

fn choice(code: &str, title: Option<&str>) -> MethodChoice {
    MethodChoice {
        code: code.into(),
        title: title.map(str::to_string),
    }
}

async fn owner(kit: &Kit) -> AdminId {
    kit.services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

async fn choose(kit: &Kit, admin: AdminId, methods: Vec<MethodChoice>) -> ServiceResult<()> {
    kit.services
        .settings_ops()
        .update_provider(
            TEST_PAY,
            &ProviderUpdate {
                methods: Some(methods),
                ..Default::default()
            },
            admin,
        )
        .await?;
    kit.services.set_remna(Some(kit.remna.clone()));
    Ok(())
}

fn ours(methods: &[CheckoutMethod]) -> Vec<(Option<&str>, &str)> {
    methods
        .iter()
        .filter(|m| m.provider == TEST_PAY)
        .map(|m| (m.method.as_deref(), m.title.as_str()))
        .collect()
}

fn subscription_request(cart: &CartRequest, method: Option<&str>) -> CheckoutRequest {
    CheckoutRequest {
        cart: cart.clone(),
        provider: TEST_PAY.into(),
        method: method.map(str::to_string),
        channel: Channel::Cabinet,
        return_url: None,
    }
}

#[tokio::test]
async fn способы_платёжки_становятся_отдельными_кнопками_и_доходят_до_счёта() {
    let Some(kit) = kit().await else { return };
    let admin = owner(&kit).await;
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

    let legacy = checkout.quote(client_id, &cart).await.unwrap();
    assert_eq!(ours(&legacy.methods), vec![(None, "Банковская карта")]);

    choose(
        &kit,
        admin,
        vec![
            choice("card", Some("  Карта МИР ")),
            choice("sbp", None),
            choice("card", None),
            choice("loan", None),
        ],
    )
    .await
    .unwrap();
    let view = checkout.quote(client_id, &cart).await.unwrap();
    assert_eq!(
        ours(&view.methods),
        vec![(Some("card"), "Карта МИР"), (Some("sbp"), "СБП")]
    );

    let result = checkout
        .start(client_id, &subscription_request(&cart, Some("sbp")))
        .await
        .unwrap();
    let CheckoutResult::Redirect { payment_id, .. } = result else {
        panic!("ожидалась ссылка на оплату");
    };
    assert_eq!(
        kit.pay
            .invoices
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .method
            .as_deref(),
        Some("sbp")
    );
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        let payment = conn.payments().by_public_id(payment_id).await.unwrap();
        assert_eq!(payment.pay_method.as_deref(), Some("sbp"));
        assert_eq!(payment.method, None);
    }

    checkout
        .start(client_id, &subscription_request(&cart, None))
        .await
        .unwrap();
    assert_eq!(
        kit.pay
            .invoices
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .method
            .as_deref(),
        Some("card")
    );

    for code_name in ["loan", "cash"] {
        assert_eq!(
            code(
                checkout
                    .start(client_id, &subscription_request(&cart, Some(code_name)))
                    .await
                    .unwrap_err()
            ),
            "checkout.method_unavailable",
            "{code_name}"
        );
    }
    kit.db.cleanup().await;
}

#[tokio::test]
async fn пополнение_учитывает_способ_и_его_минимальную_сумму() {
    let Some(kit) = kit().await else { return };
    let admin = owner(&kit).await;
    let client_id = client(&kit.services, 5_093_118_423).await;
    choose(
        &kit,
        admin,
        vec![choice("loan", None), choice("card", None)],
    )
    .await
    .unwrap();
    let checkout = kit.services.checkout();

    let options = checkout
        .topup_options(client_id, Channel::Cabinet)
        .await
        .unwrap();
    let listed: Vec<Option<&str>> = options
        .methods
        .iter()
        .filter(|m| m.provider == TEST_PAY)
        .map(|m| m.method.as_deref())
        .collect();
    assert_eq!(listed, vec![Some("card")]);

    checkout
        .topup(
            client_id,
            Money::new(5_000, Currency::Usd),
            TEST_PAY,
            Some("loan"),
            Channel::Cabinet,
        )
        .await
        .unwrap();
    assert_eq!(
        kit.pay
            .invoices
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .method
            .as_deref(),
        Some("loan")
    );
    assert_eq!(
        code(
            checkout
                .topup(
                    client_id,
                    Money::new(1_000, Currency::Usd),
                    TEST_PAY,
                    Some("loan"),
                    Channel::Cabinet,
                )
                .await
                .unwrap_err()
        ),
        "checkout.method_unavailable"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn оператор_не_может_включить_чужой_способ_или_длинное_название() {
    let Some(kit) = kit().await else { return };
    let admin = owner(&kit).await;
    assert_eq!(
        code(
            choose(&kit, admin, vec![choice("cash", None)])
                .await
                .unwrap_err()
        ),
        "settings.method_unknown"
    );
    assert_eq!(
        code(
            choose(&kit, admin, vec![choice("sbp", Some(&"С".repeat(41)))])
                .await
                .unwrap_err()
        ),
        "settings.method_title"
    );
    choose(&kit, admin, vec![choice("sbp", Some(&"С".repeat(40)))])
        .await
        .unwrap();
    let view = kit
        .services
        .payments()
        .views()
        .into_iter()
        .find(|v| v.id == TEST_PAY)
        .unwrap();
    let codes: Vec<(&str, bool)> = view.methods.iter().map(|m| (m.code, m.enabled)).collect();
    assert_eq!(codes, vec![("sbp", true), ("card", false), ("loan", false)]);
    choose(&kit, admin, Vec::new()).await.unwrap();
    let client_id = client(&kit.services, 5_093_118_424).await;
    let options = kit
        .services
        .checkout()
        .topup_options(client_id, Channel::Cabinet)
        .await
        .unwrap();
    assert!(
        options
            .methods
            .iter()
            .any(|m| m.provider == TEST_PAY && m.method.is_none())
    );
    kit.db.cleanup().await;
}
