use super::*;
use sn_domain::payment::PaymentStatus;
use sn_payments::PaymentProvider;
use sn_payments::providers::{
    AuraPay, Balance, CisPay, KassaAi, MulenPay, ParityPay, PayPalych, aurapay_signature,
    cispay_signature, kassaai_notice_signature, paritypay_signature, paypalych_signature,
};
use sn_store::InboxOutcome;
use std::sync::atomic::{AtomicI64, Ordering};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

async fn stand(provider: Arc<dyn PaymentProvider>, config: Value) -> Option<(Http, String)> {
    let id = provider.id().to_string();
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![provider, Arc::new(Balance)])),
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
            &format!("/api/admin/payments/providers/{id}"),
            &owner,
            json!({"enabled": true, "config": config}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    Some((h, owner))
}

fn no_secrets(list: &Value, secrets: &[&str]) {
    let listed = list.to_string();
    for secret in secrets {
        assert!(!listed.contains(secret), "{secret} виден в {listed}");
    }
}

async fn top_up(h: &Http, token: &str, provider: &str, amount: &str) -> Uuid {
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            token,
            json!({"amount": amount, "provider": provider}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"], "redirect");
    body["payment_id"].as_str().unwrap().parse().unwrap()
}

async fn balance(h: &Http, token: &str) -> i64 {
    let (status, options) = h.get("/api/client/topup", token).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    options["balance"]["minor"].as_i64().unwrap()
}

async fn notify(
    h: &Http,
    uri: &str,
    content_type: &str,
    headers: &[(&str, &str)],
    body: String,
) -> (StatusCode, String) {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, content_type);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router(h.state.clone())
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn journal(h: &Http) -> Vec<InboxOutcome> {
    let mut conn = h.services().store().conn().await.unwrap();
    conn.webhooks()
        .recent(50)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .map(|entry| entry.outcome)
        .collect()
}

async fn journal_page(h: &Http, owner: &str, provider: &str, title: &str) -> Value {
    let (status, page) = h
        .get(&format!("/api/admin/webhooks?provider={provider}"), owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    assert!(!items.is_empty());
    for item in items {
        assert_eq!(item["provider_title"], title, "{item}");
    }
    page
}

async fn status_of(h: &Http, public: Uuid) -> PaymentStatus {
    let mut conn = h.services().store().conn().await.unwrap();
    conn.payments().by_public_id(public).await.unwrap().status
}

async fn txid_of(h: &Http, public: Uuid) -> Option<String> {
    let mut conn = h.services().store().conn().await.unwrap();
    conn.payments()
        .by_public_id(public)
        .await
        .unwrap()
        .provider_txid
}

struct Counter(AtomicI64, fn(i64, &wiremock::Request) -> ResponseTemplate);

impl Respond for Counter {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        (self.1)(self.0.fetch_add(1, Ordering::SeqCst), request)
    }
}

const MULEN_KEY: &str = "mp_e2e_api_key";
const MULEN_SECRET: &str = "mp_e2e_secret";

async fn mulen_payment(api: &MockServer, id: i64, public: Uuid, amount: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/v2/payments/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "payment": {"id": id, "uuid": public.to_string(), "amount": amount, "currency": "rub", "status": 3}
        })))
        .mount(api)
        .await;
}

#[tokio::test]
async fn mulenpay_счёт_уведомление_повтор_чужая_сумма_и_журнал() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/payments"))
        .respond_with(Counter(AtomicI64::new(777), |id, _| {
            ResponseTemplate::new(201).set_body_json(json!({
                "success": true, "id": id, "paymentUrl": format!("https://mulenpay.ru/pay/{id}")
            }))
        }))
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "not found"})))
        .with_priority(10)
        .mount(&api)
        .await;
    let Some((h, owner)) = stand(
        Arc::new(MulenPay::with_api_base(api.uri())),
        json!({"shop_id": "12345", "api_key": MULEN_KEY, "secret_key": MULEN_SECRET, "vat_code": "0"}),
    )
    .await
    else {
        return;
    };
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    no_secrets(&list, &[MULEN_KEY, MULEN_SECRET]);
    let client = h.client(5_093_118_701).await;
    let token = h.client_token(client).await;
    let first = top_up(&h, &token, "mulenpay", "500").await;
    let second = top_up(&h, &token, "mulenpay", "700").await;
    assert_eq!(txid_of(&h, first).await.as_deref(), Some("777"));
    assert_eq!(txid_of(&h, second).await.as_deref(), Some("778"));
    mulen_payment(&api, 777, first, "500.00").await;
    mulen_payment(&api, 778, second, "699.00").await;
    let hook = |id: i64, public: Uuid| {
        json!({"id": id, "amount": 500, "currency": "RUB", "uuid": public.to_string(), "payment_status": "success"})
            .to_string()
    };
    let uri = "/api/webhooks/pay/mulenpay";
    let (status, _) = notify(&h, uri, "application/json", &[], hook(999, first)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(balance(&h, &token).await, 0);
    for body in [
        hook(777, first),
        hook(777, first),
        format!("{}\n", hook(777, first)),
    ] {
        let (status, reply) = notify(&h, uri, "application/json", &[], body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply, r#"{"success":true}"#);
    }
    assert_eq!(balance(&h, &token).await, 50_000);
    let (status, _) = notify(&h, uri, "application/json", &[], hook(778, second)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance(&h, &token).await, 50_000);
    assert_eq!(status_of(&h, first).await, PaymentStatus::Paid);
    assert_eq!(status_of(&h, second).await, PaymentStatus::Pending);
    assert_eq!(
        journal(&h).await,
        vec![
            InboxOutcome::RejectedSignature,
            InboxOutcome::Accepted,
            InboxOutcome::Duplicate,
            InboxOutcome::AmountMismatch,
        ]
    );
    journal_page(&h, &owner, "mulenpay", "MulenPay").await;
    h.db.cleanup().await;
}

const PALYCH_TOKEN: &str = "15|e2eTokenAbCdEf0123456789";
const PALYCH_SHOP: &str = "ShopE2E01";

async fn palych_bill(api: &MockServer, bill: &str, public: Uuid, amount: &str) {
    Mock::given(method("GET"))
        .and(path("/bill/status"))
        .and(query_param("id", bill))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": bill, "order_id": public.to_string(), "status": "SUCCESS", "type": "NORMAL",
            "amount": amount, "currency_in": "RUB", "success": true
        })))
        .mount(api)
        .await;
}

fn palych_postback(public: Uuid, sum: &str, bill: &str, token: &str) -> String {
    let order = public.to_string();
    let sign = paypalych_signature(&[sum, &order], token);
    format!(
        "Status=SUCCESS&InvId={order}&Commission=0&CurrencyIn=RUB&OutSum={sum}&TrsId={bill}&AccountNumber=220220%2A%2A%2A%2A%2A%2A7046&SignatureValue={sign}"
    )
}

#[tokio::test]
async fn paypalych_счёт_постбэк_повтор_чужая_сумма_возврат_и_журнал() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bill/create"))
        .respond_with(Counter(AtomicI64::new(1), |n, _| {
            ResponseTemplate::new(200).set_body_json(json!({
                "success": "true", "bill_id": format!("Bill{n}"),
                "link_url": format!("https://pally.info/link/Bill{n}"),
                "link_page_url": format!("https://pally.info/transfer/Bill{n}")
            }))
        }))
        .mount(&api)
        .await;
    let Some((h, owner)) = stand(
        Arc::new(PayPalych::with_api_base(api.uri())),
        json!({"api_token": PALYCH_TOKEN, "shop_id": PALYCH_SHOP}),
    )
    .await
    else {
        return;
    };
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    no_secrets(&list, &[PALYCH_TOKEN]);
    let client = h.client(5_093_118_702).await;
    let token = h.client_token(client).await;
    let first = top_up(&h, &token, "pal24", "500").await;
    let second = top_up(&h, &token, "pal24", "700").await;
    assert_eq!(txid_of(&h, first).await.as_deref(), Some("Bill1"));
    palych_bill(&api, "Bill1", first, "500.00").await;
    palych_bill(&api, "Bill2", second, "699.00").await;
    let uri = "/api/webhooks/pay/pal24";
    let form = "application/x-www-form-urlencoded";
    let (status, _) = notify(
        &h,
        uri,
        form,
        &[],
        palych_postback(first, "500.00", "Bill1", "1|guessed"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let genuine = palych_postback(first, "500.00", "Bill1", PALYCH_TOKEN);
    for body in [genuine.clone(), genuine.clone(), format!("{genuine}&")] {
        let (status, _) = notify(&h, uri, form, &[], body).await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(balance(&h, &token).await, 50_000);
    let (status, _) = notify(
        &h,
        uri,
        form,
        &[],
        palych_postback(second, "700.00", "Bill2", PALYCH_TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance(&h, &token).await, 50_000);
    let refund_sign = paypalych_signature(&["200.00", "RUB", "Bill1", "Pay1", "R1"], PALYCH_TOKEN);
    let refund = format!(
        "Id=R1&Amount=200.00&Currency=RUB&Status=SUCCESS&InvId={first}&BillId=Bill1&PaymentId=Pay1&SignatureValue={refund_sign}"
    );
    let (status, _) = notify(&h, &format!("{uri}/refund"), form, &[], refund).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance(&h, &token).await, 30_000);
    assert_eq!(status_of(&h, first).await, PaymentStatus::PartiallyRefunded);
    assert_eq!(
        journal(&h).await,
        vec![
            InboxOutcome::RejectedSignature,
            InboxOutcome::Accepted,
            InboxOutcome::Duplicate,
            InboxOutcome::AmountMismatch,
            InboxOutcome::Accepted,
        ]
    );
    let page = journal_page(&h, &owner, "pal24", "PayPalych").await;
    let shown = page.to_string();
    assert!(!shown.contains("220220******7046"), "{shown}");
    h.db.cleanup().await;
}

const KASSA_KEY: &str = "fk_e2e_api_key";
const KASSA_SECRET: &str = "fk_e2e_secret2";

fn kassa_notice(public: Uuid, amount: &str, intid: &str, secret: &str) -> String {
    let order = public.to_string();
    let sign = kassaai_notice_signature("777", amount, secret, &order);
    format!(
        "MERCHANT_ID=777&AMOUNT={amount}&intid={intid}&MERCHANT_ORDER_ID={order}&P_EMAIL=client%40example.com&CUR_ID=44&SIGN={sign}"
    )
}

#[tokio::test]
async fn kassaai_заказ_уведомление_повтор_чужая_сумма_и_журнал() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orders/create"))
        .respond_with(Counter(AtomicI64::new(652_367), |n, _| {
            ResponseTemplate::new(200).set_body_json(json!({
                "type": "success", "orderId": n, "orderHash": "h",
                "location": format!("https://pay.freekassa.net/form/{n}/h")
            }))
        }))
        .mount(&api)
        .await;
    let Some((h, owner)) = stand(
        Arc::new(KassaAi::with_api_base(api.uri())),
        json!({"shop_id": "777", "api_key": KASSA_KEY, "secret_word_2": KASSA_SECRET}),
    )
    .await
    else {
        return;
    };
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    no_secrets(&list, &[KASSA_KEY, KASSA_SECRET]);
    let client = h.client(5_093_118_703).await;
    let token = h.client_token(client).await;
    let first = top_up(&h, &token, "kassa_ai", "500").await;
    let second = top_up(&h, &token, "kassa_ai", "700").await;
    assert_eq!(txid_of(&h, first).await.as_deref(), Some("652367"));
    let uri = "/api/webhooks/pay/kassa_ai";
    let form = "application/x-www-form-urlencoded";
    let (status, _) = notify(
        &h,
        uri,
        form,
        &[],
        kassa_notice(first, "500", "1001", "guess"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let genuine = kassa_notice(first, "500", "652367", KASSA_SECRET);
    for body in [
        genuine.clone(),
        genuine.clone(),
        format!("{genuine}&us_retry=1"),
    ] {
        let (status, reply) = notify(&h, uri, form, &[], body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply, "YES");
    }
    assert_eq!(balance(&h, &token).await, 50_000);
    let (status, reply) = notify(
        &h,
        uri,
        form,
        &[],
        kassa_notice(second, "699", "652368", KASSA_SECRET),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, "YES");
    assert_eq!(balance(&h, &token).await, 50_000);
    assert_eq!(status_of(&h, first).await, PaymentStatus::Paid);
    assert_eq!(txid_of(&h, first).await.as_deref(), Some("652367"));
    assert_eq!(
        journal(&h).await,
        vec![
            InboxOutcome::RejectedSignature,
            InboxOutcome::Accepted,
            InboxOutcome::Duplicate,
            InboxOutcome::AmountMismatch,
        ]
    );
    let page = journal_page(&h, &owner, "kassa_ai", "Kassa.ai").await;
    assert!(!page.to_string().contains("client@example.com"));
    h.db.cleanup().await;
}

const AURA_SHOP: &str = "9bee9309-2585-4332-b63d-e1d897f7ce84";
const AURA_KEY: &str = "aura_e2e_api_key";
const AURA_SECRET: &str = "aura_e2e_secret_2";

fn aura_event(public: Uuid, invoice: &str, amount: &str) -> String {
    json!({
        "id": invoice, "amount": amount, "status": "PAID", "comment": "Пополнение",
        "created_at": "2026-11-16 14:52:00", "expires_at": "2026-11-16 15:52:00",
        "service": "sbp", "payer_details": "794*****254", "payer_ip": "203.0.113.5",
        "shop_id": AURA_SHOP, "order_id": public.to_string(), "custom_fields": null
    })
    .to_string()
}

#[tokio::test]
async fn aurapay_счёт_уведомление_повтор_чужая_сумма_и_журнал() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoice/create"))
        .respond_with(Counter(AtomicI64::new(1), |n, request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
            ResponseTemplate::new(200).set_body_json(json!({
                "id": format!("00000000-0000-4000-8000-00000000000{n}"),
                "order_id": body["order_id"], "amount": body["amount"], "status": "PENDING",
                "payment_data": {"url": format!("https://payform.aurapay.tech/pay/{n}")}
            }))
        }))
        .mount(&api)
        .await;
    let Some((h, owner)) = stand(
        Arc::new(AuraPay::with_api_base(api.uri())),
        json!({"shop_id": AURA_SHOP, "api_key": AURA_KEY, "secret_key": AURA_SECRET}),
    )
    .await
    else {
        return;
    };
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    no_secrets(&list, &[AURA_KEY, AURA_SECRET]);
    let client = h.client(5_093_118_704).await;
    let token = h.client_token(client).await;
    let first = top_up(&h, &token, "aurapay", "500").await;
    let second = top_up(&h, &token, "aurapay", "700").await;
    let invoice = "00000000-0000-4000-8000-000000000001";
    assert_eq!(txid_of(&h, first).await.as_deref(), Some(invoice));
    let uri = "/api/webhooks/pay/aurapay";
    let paid = aura_event(first, invoice, "500.00");
    let forged = aurapay_signature("guessed", paid.as_bytes());
    let (status, _) = notify(
        &h,
        uri,
        "application/json",
        &[("X-SIGNATURE", forged.as_str())],
        paid.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let sign = aurapay_signature(AURA_SECRET, paid.as_bytes());
    for body in [paid.clone(), paid.clone(), format!("{paid}\n")] {
        let (status, _) = notify(
            &h,
            uri,
            "application/json",
            &[("X-SIGNATURE", sign.as_str())],
            body,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(balance(&h, &token).await, 50_000);
    let short = aura_event(second, "00000000-0000-4000-8000-000000000002", "699.00");
    let sign = aurapay_signature(AURA_SECRET, short.as_bytes());
    let (status, _) = notify(
        &h,
        uri,
        "application/json",
        &[("X-SIGNATURE", sign.as_str())],
        short,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance(&h, &token).await, 50_000);
    assert_eq!(status_of(&h, first).await, PaymentStatus::Paid);
    assert_eq!(
        journal(&h).await,
        vec![
            InboxOutcome::RejectedSignature,
            InboxOutcome::Accepted,
            InboxOutcome::Duplicate,
            InboxOutcome::AmountMismatch,
        ]
    );
    let page = journal_page(&h, &owner, "aurapay", "AuraPay").await;
    assert!(!page.to_string().contains("203.0.113.5"));
    h.db.cleanup().await;
}

async fn journal_paths(h: &Http) -> Vec<(String, bool)> {
    let mut conn = h.services().store().conn().await.unwrap();
    sqlx::query_as("SELECT outcome, legacy_path FROM webhook_inbox ORDER BY id")
        .fetch_all(conn.conn())
        .await
        .unwrap()
}

fn echo_body(request: &wiremock::Request) -> Value {
    serde_json::from_slice(&request.body).unwrap_or_default()
}

const CIS_SHOP: &str = "0193a1b2-7c3d-7e4f-8a9b-0c1d2e3f4a5b";
const CIS_KEY: &str = "cis_sec_e2e_key_7731";

fn cis_id(n: i64) -> String {
    format!("0193a1b2-7c3d-7e4f-8a9b-{n:012}")
}

async fn cis_payment(api: &MockServer, id: &str, public: Uuid, amount: i64) {
    Mock::given(method("GET"))
        .and(path("/payments/status"))
        .and(query_param("id", id))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id, "order_id": public.to_string(), "status": "PAID", "amount": amount,
            "charged_amount": amount, "currency": "RUB", "payment_method": "SBP",
            "is_sandbox": false, "block_reason": null, "store_name": "VPN",
            "created_at": "2026-11-16T14:52:00+00:00", "expires_in_seconds": 0
        })))
        .mount(api)
        .await;
}

fn cis_hook(id: &str, public: Uuid, amount: i64, stamp: &str) -> String {
    json!({
        "id": id, "store_id": CIS_SHOP, "order_id": public.to_string(), "payment_method": "SBP",
        "status": "PAID", "amount": amount, "currency": "RUB", "charged_amount": amount,
        "merchant_revenue": amount - 1_000, "is_sandbox": false, "description": null,
        "subscription_id": null, "paid_at": "2026-11-16T15:00:00+00:00", "timestamp": stamp
    })
    .to_string()
}

#[tokio::test]
async fn cispay_платёж_уведомление_повтор_чужая_сумма_старый_адрес_и_журнал() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/payments"))
        .respond_with(Counter(AtomicI64::new(1), |n, request| {
            let body = echo_body(request);
            ResponseTemplate::new(201).set_body_json(json!({
                "id": cis_id(n), "order_id": body["order_id"], "status": "PENDING",
                "amount": body["amount"], "charged_amount": body["amount"],
                "payment_url": format!("https://cispay.app/pay/{}", cis_id(n)),
                "created_at": "2026-11-16T14:52:00+00:00"
            }))
        }))
        .mount(&api)
        .await;
    let Some((h, owner)) = stand(
        Arc::new(CisPay::with_api_base(api.uri())),
        json!({"shop_id": CIS_SHOP, "api_key": CIS_KEY}),
    )
    .await
    else {
        return;
    };
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    no_secrets(&list, &[CIS_KEY]);
    let client = h.client(5_093_118_705).await;
    let token = h.client_token(client).await;
    let first = top_up(&h, &token, "cispay", "500").await;
    let second = top_up(&h, &token, "cispay", "700").await;
    let third = top_up(&h, &token, "cispay", "300").await;
    assert_eq!(txid_of(&h, first).await, Some(cis_id(1)));
    cis_payment(&api, &cis_id(1), first, 50_000).await;
    cis_payment(&api, &cis_id(2), second, 69_900).await;
    cis_payment(&api, &cis_id(3), third, 30_000).await;
    let uri = "/api/webhooks/pay/cispay";
    let json = "application/json";
    let paid = cis_hook(&cis_id(1), first, 50_000, "2026-11-16T15:00:01+00:00");
    let forged = cispay_signature("cis_sec_guess", paid.as_bytes());
    let (status, _) = notify(
        &h,
        uri,
        json,
        &[("X-Signature", forged.as_str())],
        paid.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let again = cis_hook(&cis_id(1), first, 50_000, "2026-11-16T15:01:01+00:00");
    for body in [paid.clone(), paid, again] {
        let sign = cispay_signature(CIS_KEY, body.as_bytes());
        let (status, _) = notify(&h, uri, json, &[("X-Signature", sign.as_str())], body).await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(balance(&h, &token).await, 50_000);
    let short = cis_hook(&cis_id(2), second, 70_000, "2026-11-16T15:02:01+00:00");
    let sign = cispay_signature(CIS_KEY, short.as_bytes());
    let (status, _) = notify(&h, uri, json, &[("X-Signature", sign.as_str())], short).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance(&h, &token).await, 50_000);
    let old = cis_hook(&cis_id(3), third, 30_000, "2026-11-16T15:03:01+00:00");
    let sign = cispay_signature(CIS_KEY, old.as_bytes());
    let (status, _) = notify(
        &h,
        "/api/webhooks/cispay",
        json,
        &[("X-Signature", sign.as_str())],
        old,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "старый адрес старой панели");
    assert_eq!(balance(&h, &token).await, 80_000);
    assert_eq!(status_of(&h, first).await, PaymentStatus::Paid);
    assert_eq!(status_of(&h, second).await, PaymentStatus::Pending);
    assert_eq!(
        journal_paths(&h).await,
        vec![
            ("rejected_signature".to_string(), false),
            ("accepted".to_string(), false),
            ("duplicate".to_string(), false),
            ("amount_mismatch".to_string(), false),
            ("accepted".to_string(), true),
        ]
    );
    journal_page(&h, &owner, "cispay", "CisPay").await;
    h.db.cleanup().await;
}

const PARITY_SHOP: &str = "874dfb1e-dbdb-4747-a1c0-005969725b74";
const PARITY_ONE: &str = "pp_e2e_secret_one";
const PARITY_TWO: &str = "pp_e2e_secret_two";

fn parity_id(n: i64) -> String {
    format!("9beea835-0937-4b5c-8f5a-{n:012}")
}

async fn parity_invoice(api: &MockServer, id: &str, public: Uuid, amount: f64) {
    Mock::given(method("GET"))
        .and(path("/v2/invoice/status"))
        .and(query_param("id", id))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id, "order_id": public.to_string(), "shop_id": PARITY_SHOP, "amount": amount,
            "comment": "Пополнение", "service": "sbp", "custom_fields": null,
            "expires": "2026-11-16 15:52:00", "created": "2026-11-16 14:52:00", "status": "PAID",
            "link": format!("https://pay.paritypay.net/{id}")
        })))
        .mount(api)
        .await;
}

fn parity_hook(id: &str, public: Uuid, amount: &str, credited: f64) -> String {
    json!({
        "id": id, "order_id": public.to_string(), "shop_id": PARITY_SHOP, "amount": amount,
        "credited": credited, "comment": "Пополнение", "service": "sbp", "custom_fields": null,
        "expires": "2026-11-16 15:52:00", "created": "2026-11-16 14:52:00", "status": "PAID"
    })
    .to_string()
}

#[tokio::test]
async fn paritypay_счёт_уведомление_повтор_чужая_сумма_старый_адрес_и_журнал() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/invoice/create"))
        .respond_with(Counter(AtomicI64::new(1), |n, request| {
            let body = echo_body(request);
            ResponseTemplate::new(200).set_body_json(json!({
                "id": parity_id(n), "order_id": body["order_id"], "shop_id": PARITY_SHOP,
                "amount": body["amount"], "comment": body["comment"], "service": null,
                "custom_fields": null, "expires": "2026-11-16 15:52:00",
                "created": "2026-11-16 14:52:00", "status": "NEW",
                "link": format!("https://pay.paritypay.net/{}", parity_id(n))
            }))
        }))
        .mount(&api)
        .await;
    let Some((h, owner)) = stand(
        Arc::new(ParityPay::with_api_base(api.uri())),
        json!({"shop_id": PARITY_SHOP, "secret_key": PARITY_ONE, "callback_secret": PARITY_TWO}),
    )
    .await
    else {
        return;
    };
    let (_, list) = h.get("/api/admin/payments/providers", &owner).await;
    no_secrets(&list, &[PARITY_ONE, PARITY_TWO]);
    let client = h.client(5_093_118_706).await;
    let token = h.client_token(client).await;
    let first = top_up(&h, &token, "paritypay", "500").await;
    let second = top_up(&h, &token, "paritypay", "700").await;
    let third = top_up(&h, &token, "paritypay", "300").await;
    assert_eq!(txid_of(&h, first).await, Some(parity_id(1)));
    parity_invoice(&api, &parity_id(1), first, 500.0).await;
    parity_invoice(&api, &parity_id(2), second, 699.0).await;
    parity_invoice(&api, &parity_id(3), third, 300.0).await;
    let uri = "/api/webhooks/pay/paritypay";
    let json = "application/json";
    let paid = parity_hook(&parity_id(1), first, "500.00", 483.5);
    let forged = paritypay_signature(PARITY_ONE, paid.as_bytes());
    let (status, _) = notify(
        &h,
        uri,
        json,
        &[("X-SIGNATURE", forged.as_str())],
        paid.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let again = format!("{paid}\n");
    for body in [paid.clone(), paid, again] {
        let sign = paritypay_signature(PARITY_TWO, body.as_bytes());
        let (status, _) = notify(&h, uri, json, &[("X-SIGNATURE", sign.as_str())], body).await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(balance(&h, &token).await, 50_000);
    let short = parity_hook(&parity_id(2), second, "700.00", 676.0);
    let sign = paritypay_signature(PARITY_TWO, short.as_bytes());
    let (status, _) = notify(&h, uri, json, &[("X-SIGNATURE", sign.as_str())], short).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance(&h, &token).await, 50_000);
    let old = parity_hook(&parity_id(3), third, "300.00", 290.1);
    let sign = paritypay_signature(PARITY_TWO, old.as_bytes());
    let (status, _) = notify(
        &h,
        "/api/webhooks/paritypay",
        json,
        &[("X-SIGNATURE", sign.as_str())],
        old,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "старый адрес старой панели");
    assert_eq!(balance(&h, &token).await, 80_000);
    assert_eq!(
        journal_paths(&h).await,
        vec![
            ("rejected_signature".to_string(), false),
            ("accepted".to_string(), false),
            ("duplicate".to_string(), false),
            ("amount_mismatch".to_string(), false),
            ("accepted".to_string(), true),
        ]
    );
    journal_page(&h, &owner, "paritypay", "ParityPay").await;
    h.db.cleanup().await;
}
