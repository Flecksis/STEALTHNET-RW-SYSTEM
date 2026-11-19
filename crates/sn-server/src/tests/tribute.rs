use super::*;
use sn_payments::providers::{Balance, Tribute, tribute_signature};
use sn_store::InboxOutcome;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "trb-e2e-api-key";
const BUYER: i64 = 5_093_118_802;

async fn tribute_http(api: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Tribute::with_api_base(api.uri())),
            Arc::new(Balance),
        ])),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &json!("https://panel.example.com"), None)
            .await
            .unwrap();
    }
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

async fn notify(h: &Http, raw: &str, key: &str) -> StatusCode {
    let request = Request::post("/api/webhooks/pay/tribute")
        .header(header::CONTENT_TYPE, "application/json")
        .header("trbt-signature", tribute_signature(key, raw.as_bytes()))
        .body(Body::from(raw.to_string()))
        .unwrap();
    router(h.state.clone())
        .oneshot(request)
        .await
        .unwrap()
        .status()
}

fn purchase(id: i64, telegram: i64, minor: i64) -> String {
    purchase_of(11, id, telegram, minor)
}

fn purchase_of(product: i64, id: i64, telegram: i64, minor: i64) -> String {
    json!({
        "name": "new_digital_product",
        "created_at": "2026-11-18T10:00:00Z",
        "sent_at": "2026-11-18T10:00:01Z",
        "payload": {
            "product_id": product, "product_name": "Пополнение $5", "amount": minor, "currency": "usd",
            "telegram_user_id": telegram, "purchase_id": id, "transaction_id": id * 10,
            "purchase_created_at": "2026-11-18T10:00:00Z"
        }
    })
    .to_string()
}

#[tokio::test]
async fn tribute_настройки_каталог_пополнение_и_деньги_без_клиента() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/products"))
        .and(header("Api-Key", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "rows": [{
                "id": 11, "type": "digital", "name": "Пополнение $5", "description": "",
                "amount": 500, "currency": "USD", "status": "approved", "isCustom": false,
                "acceptCards": true, "acceptWalletPay": true, "protectContent": false,
                "created": "2026-11-01T00:00:00Z", "updated": "2026-11-01T00:00:00Z",
                "link": "https://t.me/tribute/app?startapp=p11", "webLink": "https://web.tribute.tg/p/11"
            }],
            "meta": {"total": 1, "offset": 1, "limit": 100}
        })))
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path("/subscriptions"))
        .and(header("Api-Key", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": [{"subscriptionId": 1644, "name": "VPN", "currency": "usd",
                        "periods": [{"periodId": 1547, "period": "monthly", "price": 2.99}]}]
        })))
        .mount(&api)
        .await;
    let Some(h) = tribute_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/tribute",
            &owner,
            json!({"enabled": true, "config": {"api_key": KEY}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, catalog) = h
        .get("/api/admin/payments/providers/tribute/catalog", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{catalog}");
    assert_eq!(catalog["products"][0]["id"], 11);
    assert_eq!(catalog["subscriptions"][0]["periods"][0]["price"], 299);
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/tribute",
            &owner,
            json!({"config": {"products": [catalog["products"][0].clone()]}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/tribute",
            &owner,
            json!({"config": {"products": [{"id": 11, "amount": 500}]}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    assert!(!list.to_string().contains(KEY), "{list}");

    let client = h.client(BUYER).await;
    let token = h.client_token(client).await;
    let (status, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(
        options["methods"],
        json!([{"provider": "tribute", "method": null, "title": "Tribute", "hint": "Оплата в Telegram",
                "amounts": [{"minor": 500, "currency": "USD"}]}])
    );
    let (status, started) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &token,
            json!({"amount": "5", "provider": "tribute"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let (status, _) = h.get("/api/client/topup", &token).await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(
        notify(&h, &purchase(905, BUYER, 500), "guessed-key").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        notify(&h, &purchase(905, BUYER, 500), KEY).await,
        StatusCode::OK
    );
    let (_, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(options["balance"], json!({"minor": 500, "currency": "USD"}));

    assert_eq!(
        notify(&h, &purchase(906, 77_000_002, 500), KEY).await,
        StatusCode::OK
    );
    let (status, credits) = h.get("/api/admin/credits", &owner).await;
    assert_eq!(status, StatusCode::OK, "{credits}");
    assert_eq!(credits.as_array().unwrap().len(), 1);
    assert_eq!(credits[0]["provider"], "tribute");
    let id = credits[0]["id"].as_i64().unwrap();
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/credits/{id}/close"),
            &owner,
            json!({"note": "Нашли клиента по почте и зачислили вручную"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, credits) = h.get("/api/admin/credits", &owner).await;
    assert_eq!(credits, json!([]));

    assert_eq!(
        notify(&h, &purchase_of(77, 907, BUYER, 500), KEY).await,
        StatusCode::OK
    );
    let (_, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(options["balance"], json!({"minor": 500, "currency": "USD"}));
    let (_, credits) = h.get("/api/admin/credits", &owner).await;
    assert_eq!(credits.as_array().unwrap().len(), 1, "{credits}");
    assert_eq!(credits[0]["provider"], "tribute");
    assert!(
        credits[0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("товаров для пополнения")),
        "{credits}"
    );

    let mut conn = h.services().store().conn().await.unwrap();
    let journal: Vec<InboxOutcome> = conn
        .webhooks()
        .recent(20)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .map(|entry| entry.outcome)
        .collect();
    assert_eq!(
        journal,
        vec![
            InboxOutcome::RejectedSignature,
            InboxOutcome::Accepted,
            InboxOutcome::Ignored,
            InboxOutcome::Ignored
        ]
    );
    drop(conn);
    h.db.cleanup().await;
}
