use super::*;
use sn_domain::payment::PaymentStatus;
use sn_payments::providers::{Balance, Freekassa, freekassa_notice_signature};
use sn_store::InboxOutcome;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SHOP: &str = "777";
const KEY: &str = "fk-e2e-api-key";
const WORD1: &str = "fk-e2e-word-1";
const WORD2: &str = "fk-e2e-word-2";

async fn fk_http(api: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Freekassa::with_api_base(api.uri())),
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

fn notice(amount: &str, order: Uuid, intid: &str, secret: &str) -> String {
    let sign = freekassa_notice_signature(SHOP, amount, secret, &order.to_string());
    format!(
        "MERCHANT_ID={SHOP}&AMOUNT={amount}&intid={intid}&MERCHANT_ORDER_ID={order}&P_EMAIL=client%40example.com&CUR_ID=44&SIGN={sign}"
    )
}

async fn send(h: &Http, request: Request<Body>) -> (StatusCode, String) {
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post(h: &Http, body: String) -> (StatusCode, String) {
    let request = Request::post("/api/webhooks/pay/freekassa")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    send(h, request).await
}

async fn get(h: &Http, query: String) -> (StatusCode, String) {
    let request = Request::get(format!("/api/webhooks/pay/freekassa?{query}"))
        .body(Body::empty())
        .unwrap();
    send(h, request).await
}

async fn top_up(h: &Http, token: &str, amount: &str) -> Uuid {
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            token,
            json!({"amount": amount, "provider": "freekassa"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["payment_id"].as_str().unwrap().parse().unwrap()
}

async fn balance(h: &Http, token: &str) -> Value {
    let (status, options) = h.get("/api/client/topup", token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    options["balance"].clone()
}

fn orders(order: Uuid, status: u8) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "type": "success",
        "pages": 1,
        "orders": [{
            "merchant_order_id": order.to_string(),
            "fk_order_id": 55501,
            "amount": 500,
            "currency": "RUB",
            "status": status
        }]
    }))
}

#[tokio::test]
async fn freekassa_форма_оповещение_повтор_чужая_сумма_и_возврат() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orders/refund"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"type": "success", "id": 77})),
        )
        .mount(&api)
        .await;
    let Some(h) = fk_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
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
            "/api/admin/payments/providers/freekassa",
            &owner,
            json!({"enabled": true, "config": {
                "shop_id": SHOP, "api_key": KEY, "secret_word_1": WORD1,
                "secret_word_2": WORD2, "server_ip": "203.0.113.10"
            }}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    let listed = list.to_string();
    for secret in [KEY, WORD1, WORD2] {
        assert!(!listed.contains(secret), "{listed}");
    }

    let client = h.client(5_093_118_901).await;
    let token = h.client_token(client).await;
    let (_, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(
        options["methods"],
        json!([{"provider": "freekassa", "method": null, "title": "Карта", "hint": "Карты российских банков и СБП"}])
    );
    let first = top_up(&h, &token, "500").await;
    let second = top_up(&h, &token, "700").await;
    let first_id = {
        let mut conn = h.services().store().conn().await.unwrap();
        let payment = conn.payments().by_public_id(first).await.unwrap();
        let url = payment.pay_url.clone().unwrap();
        assert!(
            url.starts_with("https://pay.fk.money/?m=777&oa=500&currency=RUB&o="),
            "{url}"
        );
        payment.id
    };
    assert!(api.received_requests().await.unwrap().is_empty());

    let (status, _) = post(&h, notice("500", first, "55501", WORD1)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for _ in 0..2 {
        let (status, body) = post(&h, notice("500.00", first, "55501", WORD2)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "YES");
    }
    assert_eq!(
        balance(&h, &token).await,
        json!({"minor": 50_000, "currency": "RUB"})
    );
    let (status, body) = get(&h, notice("699", second, "55502", WORD2)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "YES");
    assert_eq!(balance(&h, &token).await["minor"], 50_000);

    Mock::given(method("POST"))
        .and(path("/orders"))
        .respond_with(orders(first, 1))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/orders"))
        .respond_with(orders(first, 6))
        .with_priority(2)
        .mount(&api)
        .await;
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/payments/{}/refund", first_id.0),
            &owner,
            json!({"method": "provider", "subscription_action": "keep"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["refund"]["pending"], true, "{body}");
    h.services().inbox().poll_payment(first_id).await.unwrap();
    assert_eq!(balance(&h, &token).await["minor"], 0);

    let mut conn = h.services().store().conn().await.unwrap();
    let payment = conn.payments().by_public_id(first).await.unwrap();
    assert_eq!(payment.status, PaymentStatus::Refunded);
    assert_eq!(payment.provider_txid.as_deref(), Some("55501"));
    assert_eq!(
        conn.payments().by_public_id(second).await.unwrap().status,
        PaymentStatus::Pending
    );
    let refunds = conn.refunds().for_payment(payment.id).await.unwrap();
    assert_eq!(refunds.len(), 1);
    assert_eq!(refunds[0].amount.minor, 50_000);
    let sent: Vec<Value> = api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/orders/refund")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["orderId"], 55501);
    assert_eq!(sent[0]["shopId"], 777);
    let journal: Vec<(InboxOutcome, bool)> = conn
        .webhooks()
        .recent(20)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .map(|entry| (entry.outcome, entry.has_body))
        .collect();
    assert_eq!(
        journal,
        vec![
            (InboxOutcome::RejectedSignature, false),
            (InboxOutcome::Accepted, true),
            (InboxOutcome::AmountMismatch, true),
        ]
    );
    drop(conn);
    h.db.cleanup().await;
}

fn paid_order(order: Uuid, fk: u64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "type": "success",
        "pages": 1,
        "orders": [{
            "merchant_order_id": order.to_string(),
            "fk_order_id": fk,
            "amount": 500,
            "currency": "RUB",
            "status": 1
        }]
    }))
}

#[tokio::test]
async fn freekassa_повторная_оплата_засчитывается_после_подтверждения_в_api() {
    let api = MockServer::start().await;
    let Some(h) = fk_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
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
            "/api/admin/payments/providers/freekassa",
            &owner,
            json!({"enabled": true, "config": {
                "shop_id": SHOP, "api_key": KEY, "secret_word_1": WORD1,
                "secret_word_2": WORD2, "server_ip": "203.0.113.10"
            }}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let client = h.client(5_093_118_902).await;
    let token = h.client_token(client).await;
    let first = top_up(&h, &token, "500").await;
    let (status, body) = post(&h, notice("500.00", first, "55501", WORD2)).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "YES"));
    assert_eq!(balance(&h, &token).await["minor"], 50_000);
    assert!(api.received_requests().await.unwrap().is_empty());

    Mock::given(method("POST"))
        .and(path("/orders"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"type": "success", "pages": 1, "orders": []})),
        )
        .up_to_n_times(1)
        .mount(&api)
        .await;
    post(&h, notice("500.00", first, "55599", WORD2)).await;
    assert_eq!(
        balance(&h, &token).await["minor"],
        50_000,
        "повтор без подтверждения в Freekassa не зачисляется"
    );

    Mock::given(method("POST"))
        .and(path("/orders"))
        .respond_with(paid_order(first, 55600))
        .mount(&api)
        .await;
    let (status, body) = post(&h, notice("500.00", first, "55600", WORD2)).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "YES"));
    assert_eq!(
        balance(&h, &token).await["minor"],
        100_000,
        "подтверждённая повторная оплата идёт на баланс"
    );
    h.db.cleanup().await;
}
