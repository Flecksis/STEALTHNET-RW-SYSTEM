use super::*;
use sn_domain::payment::PaymentStatus;
use sn_payments::providers::{Antilopay, Balance, antilopay_signature, antilopay_verify_signature};
use sn_store::InboxOutcome;
use std::sync::Mutex;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

const MERCHANT: &str =
    include_str!("../../../sn-payments/src/providers/antilopay/testdata/merchant.pem");
const MERCHANT_PUBLIC: &str =
    include_str!("../../../sn-payments/src/providers/antilopay/testdata/merchant.pub.pem");
const PROJECT: &str =
    include_str!("../../../sn-payments/src/providers/antilopay/testdata/project.pem");
const PROJECT_PUBLIC: &str =
    include_str!("../../../sn-payments/src/providers/antilopay/testdata/project.pub.pem");
const STRANGER: &str =
    include_str!("../../../sn-payments/src/providers/antilopay/testdata/stranger.pem");
const SHOP: &str = "P7C10F3A29D54E68";
const SECRET: &str = "sid_e2e_0b51f6c2";
const HOOK: &str = "/api/webhooks/pay/antilopay";

#[derive(Default)]
struct State {
    payments: Vec<Value>,
    refunds: Vec<Value>,
    asked: Vec<String>,
}

#[derive(Clone, Default)]
struct Gate(Arc<Mutex<State>>);

impl Gate {
    fn set(&self, order: Uuid, key: &str, value: Value) {
        let mut state = self.0.lock().unwrap();
        let order = order.to_string();
        let payment = state
            .payments
            .iter_mut()
            .find(|p| p["order_id"] == order.as_str())
            .unwrap();
        payment[key] = value;
    }

    fn set_refund(&self, status: &str) {
        self.0.lock().unwrap().refunds[0]["status"] = json!(status);
    }

    fn refund(&self) -> Value {
        self.0.lock().unwrap().refunds[0].clone()
    }

    fn asked(&self, path: &str) -> usize {
        let state = self.0.lock().unwrap();
        state.asked.iter().filter(|p| *p == path).count()
    }
}

impl Respond for Gate {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
        };
        let reply = |body: Value| ResponseTemplate::new(200).set_body_json(body);
        let refuse = |code: i64| reply(json!({"code": code, "error": "refused"}));
        if header("X-Apay-Secret-Id") != SECRET {
            return refuse(2);
        }
        if header("X-Apay-Sign-Version") != "1"
            || !antilopay_verify_signature(MERCHANT_PUBLIC, &request.body, header("X-Apay-Sign"))
        {
            return refuse(3);
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        if body["project_identificator"] != SHOP {
            return refuse(4);
        }
        let mut state = self.0.lock().unwrap();
        state.asked.push(request.url.path().to_string());
        match request.url.path() {
            "/payment/create" => {
                if state
                    .payments
                    .iter()
                    .any(|p| p["order_id"] == body["order_id"])
                {
                    return refuse(5);
                }
                if body["customer"]["email"].as_str().is_none() {
                    return refuse(11);
                }
                let id = format!("APAY2611{:04}", state.payments.len() + 1);
                let url = format!("https://gate.antilopay.com/payment/{id}");
                state.payments.push(json!({
                    "code": 0, "payment_id": id, "order_id": body["order_id"], "payment_url": url,
                    "ctime": "2026-11-16 17:52:00.104233", "amount": 0, "fee": 0,
                    "original_amount": body["amount"], "status": "PENDING", "currency": "RUB",
                    "product_name": body["product_name"], "description": body["description"],
                    "customer": body["customer"]
                }));
                reply(json!({"code": 0, "payment_id": id, "payment_url": url}))
            }
            "/payment/check" => state
                .payments
                .iter()
                .find(|p| p["order_id"] == body["order_id"])
                .map_or_else(|| refuse(8), |p| reply(p.clone())),
            "/refund/create" => {
                if state
                    .refunds
                    .iter()
                    .any(|r| r["order_id"] == body["order_id"])
                {
                    return refuse(18);
                }
                let id = format!("RDAPAY2611{:04}", state.refunds.len() + 1);
                state.refunds.push(json!({
                    "code": 0, "refund_id": id, "order_id": body["order_id"],
                    "payment_id": body["transaction_id"], "amount": body["amount"],
                    "status": "PENDING"
                }));
                reply(json!({"code": 0, "refund_id": id}))
            }
            "/refund/check" => state
                .refunds
                .iter()
                .find(|r| r["refund_id"] == body["refund_id"] || r["order_id"] == body["order_id"])
                .map_or_else(|| refuse(10), |r| reply(r.clone())),
            "/project/balance" => reply(json!({
                "code": 0, "project_identificator": SHOP,
                "rub": {"available": 5000, "blocked": 0, "withdraw": 0}
            })),
            _ => ResponseTemplate::new(404),
        }
    }
}

async fn antilopay_http() -> Option<(Http, MockServer, Gate)> {
    let db = test_store().await?;
    let api = MockServer::start().await;
    let gate = Gate::default();
    Mock::given(method("POST"))
        .respond_with(gate.clone())
        .mount(&api)
        .await;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Antilopay::with_api_base(api.uri())),
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
    let h = Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    };
    Some((h, api, gate))
}

fn bare(pem: &str) -> String {
    pem.lines().filter(|l| !l.starts_with("-----")).collect()
}

fn config() -> Value {
    json!({
        "project_id": SHOP, "secret_id": SECRET, "secret_key": bare(MERCHANT),
        "callback_key": bare(PROJECT_PUBLIC)
    })
}

fn event(id: &str, order: Uuid, status: &str, note: &str) -> String {
    json!({
        "type": "payment", "payment_id": id, "order_id": order.to_string(),
        "ctime": "2026-11-16 17:52:00.104233", "amount": 480.0, "original_amount": 500.0,
        "fee": 20.0, "status": status, "currency": "RUB", "product_name": "Пополнение",
        "description": note, "pay_method": "SBER_PAY", "pay_data": "+7 9161234567",
        "customer_ip": "203.0.113.45",
        "customer": {"email": "ivan.petrov@example.com", "phone": null, "ip": null, "address": null, "fullname": null}
    })
    .to_string()
}

fn refund_event(refund: &Value, status: &str) -> String {
    json!({
        "type": "refund", "refund_id": refund["refund_id"], "payment_id": refund["payment_id"],
        "amount": refund["amount"], "ctime": "2026-11-16 18:10:00.550102", "status": status
    })
    .to_string()
}

async fn notify(h: &Http, body: &str, key: Option<&str>) -> StatusCode {
    let mut request = Request::post(HOOK).header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        request = request
            .header(
                "X-Apay-Callback",
                antilopay_signature(key, body.as_bytes()).unwrap(),
            )
            .header("X-Apay-Callback-Version", "1");
    }
    router(h.state.clone())
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
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
            json!({"amount": amount, "provider": "antilopay"}),
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

async fn balance(h: &Http, token: &str) -> i64 {
    let (status, options) = h.get("/api/client/topup", token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    options["balance"]["minor"].as_i64().unwrap()
}

async fn payment(h: &Http, public: Uuid) -> sn_domain::payment::Payment {
    let mut conn = h.services().store().conn().await.unwrap();
    conn.payments().by_public_id(public).await.unwrap()
}

#[tokio::test]
async fn antilopay_весь_путь_оплаты_на_заглушке() {
    let Some((h, _api, gate)) = antilopay_http().await else {
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

    let route = "/api/admin/payments/providers/antilopay";
    let mut swapped = config();
    swapped["callback_key"] = json!(bare(PROJECT));
    let (status, body) = h
        .write(Method::PUT, route, &owner, json!({"config": swapped}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let check_route = format!("{route}/check");
    let (status, check) = h.write(Method::POST, &check_route, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{check}");
    assert_eq!(check["status"], "settings_rejected", "{check}");
    assert!(check.to_string().contains("секретный ключ"), "{check}");
    let (status, body) = h
        .write(Method::PUT, route, &owner, json!({"enabled": true}))
        .await;
    assert!(
        status.is_client_error() || status.is_server_error(),
        "{body}"
    );
    assert!(body.to_string().contains("секретный ключ"), "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            route,
            &owner,
            json!({"enabled": true, "config": config()}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for path in ["/api/admin/payments/providers", route] {
        let (_, shown) = h.get(path, &owner).await;
        let shown = shown.to_string();
        assert!(shown.contains(SHOP), "{shown}");
        for secret in [SECRET, &bare(MERCHANT)[..40], &bare(PROJECT_PUBLIC)[..40]] {
            assert!(!shown.contains(secret), "{secret} виден в {shown}");
        }
    }
    let (status, check) = h.write(Method::POST, &check_route, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{check}");
    assert_eq!(check["status"], "ok", "{check}");
    assert_eq!(gate.asked("/payment/create"), 0);

    let client = h.client(5_093_118_802).await;
    let token = h.client_token(client).await;
    let (_, options) = h.get("/api/client/topup", &token).await;
    assert_eq!(
        options["methods"],
        json!([{"provider": "antilopay", "method": null, "title": "Карта или СБП", "hint": "Способ выбирается на странице оплаты"}])
    );
    let first = top_up(&h, &token, "500").await;
    let first_id = {
        let issued = payment(&h, first).await;
        assert_eq!(issued.provider_txid.as_deref(), Some("APAY26110001"));
        assert_eq!(
            issued.pay_url.as_deref(),
            Some("https://gate.antilopay.com/payment/APAY26110001")
        );
        issued.id
    };
    let (status, headers, _) = h
        .send(Method::GET, &format!("/api/pay/{first}"), None, None)
        .await;
    assert!(status.is_redirection(), "{status}");
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("https://gate.antilopay.com/payment/APAY26110001")
    );

    let paid = event("APAY26110001", first, "SUCCESS", "Пополнение");
    assert_eq!(
        notify(
            &h,
            &event("APAY26110001", first, "SUCCESS", "без подписи"),
            None
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        notify(
            &h,
            &event("APAY26110001", first, "SUCCESS", "чужой ключ"),
            Some(STRANGER)
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    let before = gate.asked("/payment/check");
    assert_eq!(
        notify(
            &h,
            &event("APAY26110001", first, "SUCCESS", "x"),
            Some(MERCHANT)
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(gate.asked("/payment/check"), before);
    assert_eq!(
        notify(&h, &paid, Some(PROJECT)).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "в Antilopay платёж ещё не оплачен"
    );
    assert_eq!(gate.asked("/payment/check"), before + 1);
    assert_eq!(balance(&h, &token).await, 0);
    assert_eq!(payment(&h, first).await.status, PaymentStatus::Pending);

    gate.set(first, "status", json!("SUCCESS"));
    for _ in 0..2 {
        assert_eq!(notify(&h, &paid, Some(PROJECT)).await, StatusCode::OK);
        assert_eq!(balance(&h, &token).await, 50_000);
    }
    let resent = event("APAY26110001", first, "SUCCESS", "ещё раз");
    assert_eq!(notify(&h, &resent, Some(PROJECT)).await, StatusCode::OK);
    assert_eq!(balance(&h, &token).await, 50_000);
    assert_eq!(payment(&h, first).await.status, PaymentStatus::Paid);

    let cheap = top_up(&h, &token, "700").await;
    gate.set(cheap, "status", json!("SUCCESS"));
    gate.set(cheap, "original_amount", json!(7.0));
    assert_eq!(
        notify(
            &h,
            &event("APAY26110002", cheap, "SUCCESS", "дешевле"),
            Some(PROJECT)
        )
        .await,
        StatusCode::OK
    );
    let unknown = event("APAY26119999", Uuid::from_u128(77), "SUCCESS", "нет такого");
    assert_eq!(
        notify(&h, &unknown, Some(PROJECT)).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(balance(&h, &token).await, 50_000);

    let silent = top_up(&h, &token, "300").await;
    let silent_id = payment(&h, silent).await.id;
    h.services().inbox().poll_payment(silent_id).await.unwrap();
    assert_eq!(balance(&h, &token).await, 50_000);
    gate.set(silent, "status", json!("SUCCESS"));
    h.services().inbox().poll_payment(silent_id).await.unwrap();
    assert_eq!(balance(&h, &token).await, 80_000);
    assert_eq!(
        payment(&h, silent).await.provider_txid.as_deref(),
        Some("APAY26110003")
    );

    let failed = top_up(&h, &token, "400").await;
    gate.set(failed, "status", json!("FAIL"));
    assert_eq!(
        notify(
            &h,
            &event("APAY26110004", failed, "FAIL", "не прошла"),
            Some(PROJECT)
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(payment(&h, failed).await.status, PaymentStatus::Failed);
    gate.set(failed, "status", json!("SUCCESS"));
    assert_eq!(
        notify(
            &h,
            &event("APAY26110004", failed, "SUCCESS", "со второй попытки"),
            Some(PROJECT)
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(balance(&h, &token).await, 120_000);

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
    let refund = gate.refund();
    assert_eq!(refund["payment_id"], "APAY26110001");
    assert_eq!(refund["amount"], 500.0);
    assert!(Uuid::parse_str(refund["order_id"].as_str().unwrap()).is_ok());
    let done = refund_event(&refund, "COMPLETE");
    assert_eq!(
        notify(&h, &done, Some(PROJECT)).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "возврат ещё в работе"
    );
    assert_eq!(payment(&h, first).await.status, PaymentStatus::Paid);
    gate.set_refund("COMPLETE");
    for _ in 0..2 {
        assert_eq!(notify(&h, &done, Some(PROJECT)).await, StatusCode::OK);
    }
    assert_eq!(payment(&h, first).await.status, PaymentStatus::Refunded);
    assert_eq!(balance(&h, &token).await, 70_000);
    h.services().inbox().poll_payment(first_id).await.unwrap();

    let mut conn = h.services().store().conn().await.unwrap();
    let refunds = conn.refunds().for_payment(first_id).await.unwrap();
    assert_eq!(refunds.len(), 1);
    assert_eq!(refunds[0].amount.minor, 50_000);
    let journal: Vec<(InboxOutcome, bool)> = conn
        .webhooks()
        .recent(50)
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
            (InboxOutcome::RejectedSignature, false),
            (InboxOutcome::Error, false),
            (InboxOutcome::Accepted, true),
            (InboxOutcome::Duplicate, true),
            (InboxOutcome::AmountMismatch, true),
            (InboxOutcome::RejectedPayload, false),
            (InboxOutcome::Accepted, true),
            (InboxOutcome::Accepted, true),
            (InboxOutcome::Error, false),
            (InboxOutcome::Accepted, true),
        ]
    );
    let stored: Vec<String> = sqlx::query_scalar(
        "SELECT convert_from(body, 'UTF8') || headers::text FROM webhook_inbox WHERE body IS NOT NULL",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert!(!stored.is_empty());
    for row in &stored {
        for hidden in [
            "ivan.petrov",
            "9161234567",
            "203.0.113.45",
            "x-apay-callback",
        ] {
            assert!(
                !row.to_lowercase().contains(hidden),
                "{hidden} остался в {row}"
            );
        }
    }
    drop(conn);
    let (status, page) = h
        .get("/api/admin/webhooks?provider=antilopay", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    for item in page["items"].as_array().unwrap() {
        assert_eq!(item["provider_title"], "Antilopay", "{item}");
    }
    h.db.cleanup().await;
}
