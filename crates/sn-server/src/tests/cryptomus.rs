use super::*;
use sn_domain::payment::PaymentStatus;
use sn_payments::providers::{Balance, Cryptomus, cryptomus_signature};
use sn_store::InboxOutcome;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

const MERCHANT: &str = "8b03432e-385b-4670-8d06-064591096795";
const KEY: &str = "cryptomus-e2e-payment-key";

struct Echo;

impl Respond for Echo {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let order = body["order_id"].as_str().unwrap_or_default().to_string();
        let signed = request
            .headers
            .get("sign")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|sign| sign == cryptomus_signature(&request.body, KEY));
        if !signed {
            return ResponseTemplate::new(401)
                .set_body_json(json!({"state": 1, "message": "Invalid sign"}));
        }
        ResponseTemplate::new(200).set_body_json(json!({
            "state": 0,
            "result": {
                "uuid": format!("cm-{order}"),
                "order_id": order,
                "amount": body["amount"],
                "currency": body["currency"],
                "url": format!("https://pay.cryptomus.com/pay/cm-{order}"),
                "expired_at": 1_763_308_800,
                "payment_status": "check",
                "status": "check",
                "is_final": false
            }
        }))
    }
}

async fn cryptomus_http(api: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Cryptomus::with_api_base(api.uri())),
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

fn event(public: Uuid, status: &str, amount: &str) -> String {
    format!(
        r#"{{"type":"payment","uuid":"cm-{public}","order_id":"{public}","amount":"{amount}","payment_amount":"{amount}","payment_amount_usd":"{amount}","merchant_amount":"4.90","commission":"0.10","is_final":true,"status":"{status}","from":null,"wallet_address_uuid":null,"network":"tron","currency":"USD","payer_currency":"USDT","additional_data":null,"txid":"9a1c\/7e"}}"#
    )
}

fn signed(body: &str, key: &str) -> String {
    let sign = cryptomus_signature(body.as_bytes(), key);
    format!("{},\"sign\":\"{sign}\"}}", &body[..body.len() - 1])
}

async fn notify(h: &Http, raw: String) -> StatusCode {
    let request = Request::post("/api/webhooks/pay/cryptomus")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(raw))
        .unwrap();
    router(h.state.clone())
        .oneshot(request)
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
            json!({"amount": amount, "provider": "cryptomus"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"], "redirect");
    let public: Uuid = body["payment_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        body["pay_url"],
        format!("https://panel.example.com/api/pay/{public}")
    );
    public
}

async fn balance(h: &Http, token: &str) -> Value {
    let (status, options) = h.get("/api/client/topup", token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    options["balance"].clone()
}

#[tokio::test]
async fn cryptomus_счёт_уведомление_повтор_и_чужая_сумма() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/payment"))
        .and(header("merchant", MERCHANT))
        .respond_with(Echo)
        .mount(&api)
        .await;
    let Some(h) = cryptomus_http(&api).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/cryptomus",
            &owner,
            json!({"enabled": true, "config": {"merchant_id": MERCHANT, "api_key": KEY}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    let listed = list.to_string();
    assert!(!listed.contains(KEY), "{listed}");

    let client = h.client(5_093_118_777).await;
    let token = h.client_token(client).await;
    let (status, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(
        options["methods"],
        json!([{"provider": "cryptomus", "method": null, "title": "Криптовалюта", "hint": "USDT и другие монеты"}])
    );
    let first = top_up(&h, &token, "5").await;
    let second = top_up(&h, &token, "7").await;
    {
        let mut conn = h.services().store().conn().await.unwrap();
        let payment = conn.payments().by_public_id(first).await.unwrap();
        assert_eq!(
            payment.provider_txid.as_deref(),
            Some(format!("cm-{first}").as_str())
        );
        assert_eq!(
            payment.pay_url.as_deref(),
            Some(format!("https://pay.cryptomus.com/pay/cm-{first}").as_str())
        );
    }
    let created: Vec<Value> = api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(created.len(), 2);
    assert_eq!(created[0]["amount"], "5.00");
    assert_eq!(created[0]["currency"], "USD");
    assert_eq!(created[0]["order_id"], first.to_string());
    assert_eq!(
        created[0]["url_callback"],
        "https://panel.example.com/api/webhooks/pay/cryptomus"
    );
    assert_eq!(created[1]["amount"], "7.00");

    let paid = event(first, "paid", "5.00");
    assert_eq!(
        notify(&h, signed(&paid, "guessed-key")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(notify(&h, paid.clone()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(balance(&h, &token).await["minor"], 0);
    for _ in 0..2 {
        assert_eq!(notify(&h, signed(&paid, KEY)).await, StatusCode::OK);
    }
    assert_eq!(
        balance(&h, &token).await,
        json!({"minor": 500, "currency": "USD"})
    );
    let spaced = signed(&paid, KEY)
        .replace("\":\"", "\": \"")
        .replace("\",\"", "\", \"");
    assert_ne!(spaced, signed(&paid, KEY));
    assert_eq!(notify(&h, spaced).await, StatusCode::OK);
    assert_eq!(balance(&h, &token).await["minor"], 500);
    let wrong = event(second, "paid", "6.99");
    assert_eq!(notify(&h, signed(&wrong, KEY)).await, StatusCode::OK);
    assert_eq!(balance(&h, &token).await["minor"], 500);
    let waiting = event(second, "confirm_check", "7.00");
    assert_eq!(notify(&h, signed(&waiting, KEY)).await, StatusCode::OK);
    assert_eq!(balance(&h, &token).await["minor"], 500);

    let mut conn = h.services().store().conn().await.unwrap();
    assert_eq!(
        conn.payments().by_public_id(first).await.unwrap().status,
        PaymentStatus::Paid
    );
    assert_eq!(
        conn.payments().by_public_id(second).await.unwrap().status,
        PaymentStatus::Pending
    );
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
            (InboxOutcome::RejectedSignature, false),
            (InboxOutcome::Accepted, true),
            (InboxOutcome::Duplicate, true),
            (InboxOutcome::AmountMismatch, true),
            (InboxOutcome::Ignored, true),
        ]
    );
    drop(conn);
    h.db.cleanup().await;
}
