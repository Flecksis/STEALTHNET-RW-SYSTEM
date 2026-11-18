use super::*;
use sn_domain::client::Locale;
use sn_domain::money::{Amount, Money, PayCurrency};
use sn_domain::order::OrderStatus;
use sn_domain::payment::PaymentStatus;
use sn_domain::pricing::Quote;
use sn_store::NewClient;

async fn with_modules() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(sn_modules::catalog::all()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

async fn paid(h: &Http, method: Option<&str>, amount: i64) {
    let mut conn = h.services().store().conn().await.unwrap();
    let client = conn
        .clients()
        .create(&NewClient {
            locale: Locale::Ru,
            registration_source: RegistrationSource::Bot,
            note: None,
        })
        .await
        .unwrap();
    let total = Money::new(amount, Currency::Rub);
    let order = conn
        .orders()
        .create(&NewOrder {
            client_id: client.id,
            subscription_id: None,
            kind: OrderKind::New,
            intent: PurchaseIntent::Trial,
            quote: Quote {
                currency: Currency::Rub,
                lines: Vec::new(),
                subtotal: total,
                discount: Money::zero(Currency::Rub),
                total,
                stars: None,
                days: 30,
                extra_devices: 0,
            },
            snapshot: None,
        })
        .await
        .unwrap();
    conn.orders()
        .transition(order.id, OrderStatus::Paid)
        .await
        .unwrap();
    let payment = conn
        .payments()
        .create(&NewPayment {
            order_id: order.id,
            client_id: client.id,
            provider: ProviderId::from(providers::YOOKASSA),
            method: None,
            amount: Amount::new(amount, PayCurrency::Rub),
            expires_at: None,
        })
        .await
        .unwrap();
    if let Some(code) = method {
        conn.payments()
            .set_pay_method(payment.id, code)
            .await
            .unwrap();
    }
    conn.payments()
        .transition(
            payment.id,
            PaymentStatus::Paid,
            Some(&format!("yk-{}", payment.id)),
            None,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn аналитика_видна_с_правом_деньги_и_называет_способы_как_оператор() {
    let Some(h) = with_modules().await else {
        return;
    };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, body) = h.get("/api/admin/m/analytics/report", &owner).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "module.disabled");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "currency": "RUB" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/yookassa",
            &owner,
            json!({ "methods": [{ "code": "sbp", "title": "СБП в приложении" }, { "code": "bank_card" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .write(
            Method::PATCH,
            "/api/admin/modules/analytics",
            &owner,
            json!({ "enabled": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    paid(&h, Some("sbp"), 50_000).await;
    paid(&h, Some("bank_card"), 30_000).await;
    paid(&h, None, 20_000).await;

    let (status, _) = h.get("/api/admin/m/analytics/report", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = h
        .get("/api/admin/m/analytics/report?months=3", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["currency"], "RUB");
    assert_eq!(body["months"].as_array().unwrap().len(), 3);
    assert_eq!(body["totals"]["revenue"], 100_000);
    assert_eq!(body["totals"]["new"], 3);
    let yookassa = &body["providers"][0];
    assert_eq!(yookassa["provider"], "yookassa");
    assert_eq!(yookassa["title"], "ЮKassa");
    assert_eq!(yookassa["revenue"], 100_000);
    let methods: Vec<(&str, i64)> = yookassa["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["title"].as_str().unwrap(), m["revenue"].as_i64().unwrap()))
        .collect();
    assert_eq!(
        methods,
        [
            ("СБП в приложении", 50_000),
            ("Карта", 30_000),
            ("Кнопка «Банковская карта»", 20_000)
        ]
    );
    let (status, body) = h
        .get("/api/admin/m/analytics/report?all=true", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["totals"]["revenue"], 100_000);
    let (status, body) = h
        .get(
            "/api/admin/m/analytics/report?from=2026-05&to=2026-02",
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "analytics.period");
    h.db.cleanup().await;
}
