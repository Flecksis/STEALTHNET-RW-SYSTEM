use super::*;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use sn_domain::payment::PaymentStatus;
use sn_payments::providers::{Balance, Wata};
use sn_store::InboxOutcome;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

const PUBLIC: &str = include_str!("../../../sn-payments/src/providers/wata/testdata/public.pem");
const SIGNER: &str = include_str!("../../../sn-payments/src/providers/wata/testdata/signer.pem");
const TOKEN: &str = "wata-e2e-token.jwt.sig";
const TX: &str = "3a16a4f0-27b0-09d1-16da-ba8d5c63eae3";
const FOREIGN_TX: &str = "9a16a4f0-27b0-09d1-16da-ba8d5c63eae9";
const REFUND_TX: &str = "b8c19d45-1234-4123-a5b6-890123456789";
const TERMINAL: &str = "3b16a2f1-dead-4e5d-abff-90865d1e13b1";

struct Link;

impl Respond for Link {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let order = body["orderId"].as_str().unwrap_or_default().to_string();
        ResponseTemplate::new(200).set_body_json(json!({
            "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
            "type": "OneTime",
            "amount": body["amount"],
            "currency": body["currency"],
            "status": "Opened",
            "url": format!("https://payment.wata.pro/pay-form/{order}"),
            "terminalName": "shop",
            "terminalPublicId": TERMINAL,
            "creationTime": "2026-11-16T14:52:00Z",
            "orderId": order,
            "expirationDateTime": body["expirationDateTime"],
            "isArbitraryAmountAllowed": false,
            "arbitraryAmounts": []
        }))
    }
}

async fn wata_http(api: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Wata::with_api_base(api.uri())),
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

fn signature(body: &[u8]) -> String {
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(SIGNER.as_bytes()).unwrap();
    let signed = jsonwebtoken::crypto::sign(body, &key, jsonwebtoken::Algorithm::RS512).unwrap();
    STANDARD.encode(URL_SAFE_NO_PAD.decode(signed).unwrap())
}

fn event(kind: &str, status: &str, id: &str, order: Uuid, amount: &str) -> String {
    let original = if kind == "Refund" {
        format!("\"{TX}\"")
    } else {
        "null".into()
    };
    format!(
        "{{\"transactionType\":\"SBP\",\"kind\":\"{kind}\",\"transactionId\":\"{id}\",\"originalTransactionId\":{original},\"transactionStatus\":\"{status}\",\"terminalPublicId\":\"{TERMINAL}\",\"amount\":{amount},\"currency\":\"RUB\",\"orderId\":\"{order}\",\"paymentTime\":\"2026-11-16T15:00:00Z\",\"commission\":10,\"email\":\"client@example.com\"}}"
    )
}

fn transaction(id: &str, kind: &str, order: Uuid, amount: f64) -> Value {
    json!({
        "id": id,
        "terminalPublicId": TERMINAL,
        "type": "SBP",
        "kind": kind,
        "originalTransactionId": if kind == "Refund" { json!(TX) } else { json!(null) },
        "amount": amount,
        "currency": "RUB",
        "status": "Paid",
        "orderId": order.to_string(),
        "creationTime": "2026-11-16T14:59:00Z"
    })
}

async fn notify(h: &Http, raw: &str, signed: bool) -> StatusCode {
    let mut request =
        Request::post("/api/webhooks/pay/wata").header(header::CONTENT_TYPE, "application/json");
    if signed {
        request = request.header("X-Signature", signature(raw.as_bytes()));
    }
    router(h.state.clone())
        .oneshot(request.body(Body::from(raw.to_string())).unwrap())
        .await
        .unwrap()
        .status()
}

async fn top_up(h: &Http, token: &str, amount: &str) -> Uuid {
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            token,
            json!({"amount": amount, "provider": "wata"}),
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

#[tokio::test]
async fn wata_ссылка_подписанное_уведомление_чужой_терминал_и_возврат() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": PUBLIC })))
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/links"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(Link)
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/transactions/{FOREIGN_TX}")))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(
                json!({"error": {"code": null, "message": "Transaction not found"}}),
            ),
        )
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/transactions/refunds"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "transactionId": REFUND_TX, "originalTransactionId": TX,
            "transactionStatus": "Pending", "kind": "Refund",
            "errorCode": null, "errorDescription": null
        })))
        .mount(&api)
        .await;
    let Some(h) = wata_http(&api).await else {
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
            "/api/admin/payments/providers/wata",
            &owner,
            json!({"enabled": true, "config": {"api_token": TOKEN}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    assert!(!list.to_string().contains(TOKEN));

    let client = h.client(5_093_118_801).await;
    let token = h.client_token(client).await;
    let (_, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(
        options["methods"],
        json!([{"provider": "wata", "method": null, "title": "Карта", "hint": "Карты российских банков и СБП"}])
    );
    let first = top_up(&h, &token, "500").await;
    let first_id = {
        let mut conn = h.services().store().conn().await.unwrap();
        let payment = conn.payments().by_public_id(first).await.unwrap();
        assert_eq!(
            payment.pay_url.as_deref(),
            Some(format!("https://payment.wata.pro/pay-form/{first}").as_str())
        );
        assert_eq!(payment.provider_txid, None);
        payment.id
    };
    Mock::given(method("GET"))
        .and(path(format!("/transactions/{TX}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(transaction(TX, "Payment", first, 500.0)),
        )
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/transactions/{REFUND_TX}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(transaction(REFUND_TX, "Refund", first, 500.0)),
        )
        .mount(&api)
        .await;

    let paid = event("Payment", "Paid", TX, first, "500.00");
    assert_eq!(notify(&h, &paid, false).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        notify(
            &h,
            &event("Payment", "Paid", FOREIGN_TX, first, "500.00"),
            true
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(balance(&h, &token).await["minor"], 0);
    assert_eq!(
        notify(&h, &event("Payment", "Created", TX, first, "500.00"), true).await,
        StatusCode::OK
    );
    for _ in 0..2 {
        assert_eq!(notify(&h, &paid, true).await, StatusCode::OK);
    }
    assert_eq!(
        balance(&h, &token).await,
        json!({"minor": 50_000, "currency": "RUB"})
    );
    {
        let mut conn = h.services().store().conn().await.unwrap();
        let payment = conn.payments().by_public_id(first).await.unwrap();
        assert_eq!(payment.provider_txid.as_deref(), Some(TX));
    }

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
    let refunded = event("Refund", "Paid", REFUND_TX, first, "500.00");
    assert_eq!(notify(&h, &refunded, true).await, StatusCode::OK);
    assert_eq!(balance(&h, &token).await["minor"], 0);

    let mut conn = h.services().store().conn().await.unwrap();
    let payment = conn.payments().by_public_id(first).await.unwrap();
    assert_eq!(payment.status, PaymentStatus::Refunded);
    let refunds = conn.refunds().for_payment(payment.id).await.unwrap();
    assert_eq!(refunds.len(), 1);
    assert_eq!(refunds[0].amount.minor, 50_000);
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
            (InboxOutcome::RejectedPayload, false),
            (InboxOutcome::Ignored, true),
            (InboxOutcome::Accepted, true),
            (InboxOutcome::Accepted, true),
        ]
    );
    drop(conn);
    h.db.cleanup().await;
}
