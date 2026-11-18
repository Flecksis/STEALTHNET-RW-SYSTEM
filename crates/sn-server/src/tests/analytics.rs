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

async fn ready_tab(h: &Http, path: &str, admin: &str) -> serde_json::Value {
    for _ in 0..100 {
        let (status, body) = h.get(path, admin).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["preparing"] == false {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("аналитика так и не посчиталась");
}

fn metric<'a>(body: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["metrics"].as_array().unwrap().iter())
        .find(|m| m["key"] == key)
        .unwrap_or_else(|| panic!("нет показателя {key}"))
}

#[tokio::test]
async fn аналитика_видна_с_правом_деньги_и_называет_способы_как_оператор() {
    let Some(h) = with_modules().await else {
        return;
    };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, body) = h.get("/api/admin/m/analytics/tab/money", &owner).await;
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

    let (status, _) = h.get("/api/admin/m/analytics/tab/money", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let body = ready_tab(&h, "/api/admin/m/analytics/tab/money", &owner).await;
    assert_eq!(body["currency"], "RUB");
    let revenue = metric(&body, "revenue");
    assert_eq!(revenue["value"], 100_000.0);
    let methods: Vec<(&str, f64)> = revenue["detail"]["breakdowns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["key"] == "method")
        .unwrap()["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["label"].as_str().unwrap(), r["value"].as_f64().unwrap()))
        .collect();
    assert_eq!(
        methods,
        [
            ("ЮKassa · СБП в приложении", 50_000.0),
            ("ЮKassa · Карта", 30_000.0),
            ("ЮKassa · Кнопка «Банковская карта»", 20_000.0)
        ]
    );
    let from = body["range"]["from"].as_str().unwrap().to_string();
    let to = body["range"]["to"].as_str().unwrap().to_string();
    let (status, list) = h
        .get(
            &format!("/api/admin/m/analytics/list?set=revenue&slice=method:yookassa/sbp&from={from}&to={to}"),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["amount"], 50_000);
    let (status, options) = h.get("/api/admin/m/analytics/options", &owner).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["providers"][0]["value"], "yookassa");
    for tab in ["clients", "retention", "payments", "sources", "tariffs", "economy", "support"] {
        let body = ready_tab(&h, &format!("/api/admin/m/analytics/tab/{tab}"), &owner).await;
        assert!(!body["groups"].as_array().unwrap().is_empty(), "{tab}");
    }
    let (status, body) = h
        .get(
            "/api/admin/m/analytics/tab/money?from=2026-05-10&to=2026-02-01",
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "analytics.period");
    let (status, body) = h.get("/api/admin/m/analytics/tab/nope", &owner).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "analytics.tab");
    h.db.cleanup().await;
}

#[tokio::test]
async fn ссылки_меток_создаются_и_не_повторяются() {
    let Some(h) = with_modules().await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PATCH,
            "/api/admin/modules/utm",
            &owner,
            json!({ "enabled": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let link = json!({ "title": "Осенние шортсы", "source": "tiktok", "medium": "video", "campaign": "osen-shorts" });
    let (status, body) = h
        .write(Method::POST, "/api/admin/m/utm/links", &owner, link.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_i64().unwrap();
    let (status, body) = h
        .write(Method::POST, "/api/admin/m/utm/links", &owner, link)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "utm.exists");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/m/utm/links",
            &owner,
            json!({ "title": "Кириллица", "source": "тикток" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "utm.value");
    let (status, body) = h.get("/api/admin/m/utm/links", &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["links"][0]["param"], "s_tiktok__m_video__k_osen-shorts");
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/m/utm/links/{id}/archive"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = h.get("/api/admin/m/utm/links", &owner).await;
    assert!(body["links"].as_array().unwrap().is_empty());
    h.db.cleanup().await;
}
