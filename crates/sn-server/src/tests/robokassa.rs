use super::*;
use sn_domain::payment::PaymentStatus;
use sn_payments::providers::{Balance, Robokassa, robokassa_invoice_number, robokassa_signature};
use sn_store::InboxOutcome;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LOGIN: &str = "stealth-e2e";
const PASS1: &str = "e2ePass1Main";
const PASS2: &str = "e2ePass2Main";
const PASS3: &str = "e2ePass3Refund";
const OP_KEY: &str = "14D2B521-4EAB-492A-9D91-00D12FF24D57-1NvHQWRwf";
const REQUEST: &str = "cf15fd52-d2d1-4fc4-b9c0-25310e3bdded";

async fn robo_http(api: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Robokassa::with_api_base(api.uri())),
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

fn form(out_sum: &str, inv: &str, password: &str) -> String {
    let sign = robokassa_signature("md5", &format!("{out_sum}:{inv}:{password}")).to_uppercase();
    format!(
        "OutSum={out_sum}&InvId={inv}&Fee=17.500000&EMail=client%40example.com&SignatureValue={sign}&PaymentMethod=BankCard&IncCurrLabel=BankCardPSR"
    )
}

async fn notify(h: &Http, request: Request<Body>) -> (StatusCode, String) {
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post(h: &Http, body: String) -> (StatusCode, String) {
    let request = Request::post("/api/webhooks/pay/robokassa")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    notify(h, request).await
}

async fn get(h: &Http, query: String) -> (StatusCode, String) {
    let request = Request::get(format!("/api/webhooks/pay/robokassa?{query}"))
        .body(Body::empty())
        .unwrap();
    notify(h, request).await
}

async fn top_up(h: &Http, token: &str, amount: &str) -> Uuid {
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            token,
            json!({"amount": amount, "provider": "robokassa"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"], "redirect");
    body["payment_id"].as_str().unwrap().parse().unwrap()
}

async fn balance(h: &Http, token: &str) -> Value {
    let (status, options) = h.get("/api/client/topup", token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    options["balance"].clone()
}

#[tokio::test]
async fn robokassa_ссылка_уведомление_повтор_чужая_сумма_и_возврат() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Merchant/WebService/Service.asmx/OpStateExt"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><OperationStateResponse xmlns=\"http://merchant.roboxchange.com/WebService/\"><Result><Code>0</Code></Result><State><Code>100</Code></State><Info><IncCurrLabel>BankCardPSR</IncCurrLabel><IncSum>500.000000</IncSum><OutSum>500.000000</OutSum><OpKey>{OP_KEY}</OpKey></Info></OperationStateResponse>"
        )))
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/RefundService/Refund/Create"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"success": true, "message": null, "requestId": REQUEST})),
        )
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path("/RefundService/Refund/GetState"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"requestId": REQUEST, "amount": 500.0, "label": "finished"})),
        )
        .mount(&api)
        .await;
    let Some(h) = robo_http(&api).await else {
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
            "/api/admin/payments/providers/robokassa",
            &owner,
            json!({"enabled": true, "config": {
                "merchant_login": LOGIN, "password1": PASS1,
                "password2": PASS2, "password3": PASS3
            }}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    let listed = list.to_string();
    for secret in [PASS1, PASS2, PASS3] {
        assert!(!listed.contains(secret), "{listed}");
    }

    let client = h.client(5_093_118_701).await;
    let token = h.client_token(client).await;
    let (status, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(
        options["methods"],
        json!([{"provider": "robokassa", "method": null, "title": "Карта", "hint": "Карты российских банков и СБП"}])
    );
    let first = top_up(&h, &token, "500").await;
    let second = top_up(&h, &token, "700").await;
    let first_inv = robokassa_invoice_number(first).to_string();
    let second_inv = robokassa_invoice_number(second).to_string();
    let first_id = {
        let mut conn = h.services().store().conn().await.unwrap();
        let payment = conn.payments().by_public_id(first).await.unwrap();
        assert_eq!(payment.provider_txid.as_deref(), Some(first_inv.as_str()));
        let url = payment.pay_url.clone().unwrap();
        assert!(
            url.starts_with(&format!("{}/Merchant/Index.aspx?", api.uri())),
            "{url}"
        );
        assert!(url.contains(&format!("InvId={first_inv}")), "{url}");
        assert!(url.contains("OutSum=500.00"), "{url}");
        payment.id
    };

    let (status, _) = post(&h, form("500.000000", &first_inv, PASS1)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(balance(&h, &token).await["minor"], 0);
    for _ in 0..2 {
        let (status, body) = post(&h, form("500.000000", &first_inv, PASS2)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("OK{first_inv}"));
    }
    assert_eq!(
        balance(&h, &token).await,
        json!({"minor": 50_000, "currency": "RUB"})
    );
    let (status, body) = get(&h, form("699.000000", &second_inv, PASS2)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, format!("OK{second_inv}"));
    assert_eq!(balance(&h, &token).await["minor"], 50_000);

    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/payments/{}/refund", first_id.0),
            &owner,
            json!({"method": "provider", "subscription_action": "keep", "reason": "по просьбе клиента"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["refund"]["pending"], true, "{body}");
    h.services().inbox().poll_payment(first_id).await.unwrap();
    assert_eq!(balance(&h, &token).await["minor"], 0);

    let mut conn = h.services().store().conn().await.unwrap();
    let refunded = conn.payments().by_public_id(first).await.unwrap();
    assert_eq!(refunded.status, PaymentStatus::Refunded);
    let refunds = conn.refunds().for_payment(refunded.id).await.unwrap();
    assert_eq!(refunds.len(), 1);
    assert_eq!(refunds[0].amount.minor, 50_000);
    assert_eq!(
        conn.payments().by_public_id(second).await.unwrap().status,
        PaymentStatus::Pending
    );
    let created: Vec<String> = api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/RefundService/Refund/Create")
        .map(|r| serde_json::from_slice::<String>(&r.body).unwrap())
        .collect();
    assert_eq!(created.len(), 1);
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
