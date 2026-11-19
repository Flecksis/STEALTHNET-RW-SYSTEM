use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SHOP: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const KEY: &str = "ak_live_7d1f9c2e";
const SECRET: &str = "test_secret_key_2";
const INVOICE: &str = "9beea835-0937-4b5c-8f5a-c3a0d0e60346";
const V1: &str = r#"{"id":"11111111-2222-4333-8444-555555555555","amount":"100.00","status":"PAID","comment":"Order 42","created_at":"2026-11-18 12:00:00","expires_at":"2026-11-18 13:00:00","service":"sbp","payer_details":"791*****123","payer_ip":"203.0.113.10","shop_id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","order_id":"42","custom_fields":null}"#;
const V1_SIGNATURE: &str = "758c03976952b07a5806075e20e7f985a5ff81bdb6ea230e018a75e56e01a0be";
const V2: &str = r#"{"id":"22222222-3333-4444-8555-666666666666","amount":"299.00","status":"PAID","comment":"Оплата тарифа","created_at":"2026-11-18 12:00:00","expires_at":"2026-11-18 13:00:00","service":"card","payer_details":"220000******0000","payer_ip":"203.0.113.11","shop_id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","order_id":"ORD-7","custom_fields":"uid=5&back=https:\/\/example.com\/x","subscription_id":"SUB-7"}"#;
const V2_SIGNATURE: &str = "d2199ea0671c919699d3b3120fa1cdcac9eb164edb8874574185ba485b35534f";
const V3: &str = r#"{"event":"PAYMENT_ERROR","id":"33333333-4444-4555-8666-777777777777","subscription_id":"SUB-7","shop_id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","invoice_id":null,"service":"card","amount":"299.00","period":1,"interval":"month","status":"WAITING_PAYMENT","next_pay_at":"2026-12-19 12:00:00","payer_details":"220000******0000","reason":null,"created_at":"2026-11-18 12:00:00"}"#;
const V3_SIGNATURE: &str = "327c1067581250153ae70570662dc794de2650ab7af7ca3dad0c06b81bf1304f";
const V4: &str = r#"{"id":"44444444-5555-4666-8777-888888888888","order_id":"PO-1","shop_id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","service":"card_ru","amount":"1000.00","amount_to_payout":"990.00","commission":"10.00","debited":"1000.00","wallet_to":"2200000000000000","created_at":"2026-11-18 12:00:00","status":"SUCCESS"}"#;
const V4_SIGNATURE: &str = "72416bf1e2077eb6d6f01f71adee167f17265d4d9701aaab3b298296dd875444";
const HOOK: &str = "https://panel.example.com/api/webhooks/pay/aurapay";

fn settings() -> ProviderConfig {
    config(&[
        ("shop_id", json!(SHOP)),
        ("api_key", json!(KEY)),
        ("secret_key", json!(SECRET)),
    ])
}

fn event(status: &str, amount: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": INVOICE, "amount": amount, "status": status, "comment": "Стандарт",
        "created_at": "2026-11-18 12:00:00", "expires_at": "2026-11-18 13:00:00",
        "service": "sbp", "payer_details": "794*****254", "payer_ip": "203.0.113.5",
        "shop_id": SHOP, "order_id": PAYMENT.to_string(), "custom_fields": null
    }))
    .unwrap()
}

fn signed(body: impl Into<Vec<u8>>) -> WebhookRequest {
    let body = body.into();
    let sign = signature(SECRET, &body);
    hook(&[("X-SIGNATURE", sign.as_str())], body)
}

fn remote(status: &str, amount: serde_json::Value) -> serde_json::Value {
    json!({
        "id": INVOICE, "order_id": PAYMENT.to_string(), "shop_id": SHOP, "amount": amount,
        "comment": "Стандарт", "service": "sbp", "status": status,
        "expires_at": "2026-11-18 13:00:00", "created_at": "2026-11-18 12:00:00",
        "payment_data": {"url": "https://payform.aurapay.tech/pay/9beea835"}
    })
}

async fn api(body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoice/status"))
        .and(header("X-ApiKey", KEY))
        .and(header("X-ShopId", SHOP))
        .and(body_json(json!({"id": INVOICE})))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/invoice/status"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"error": "Invoice not found", "data": []})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

async fn parse(server: &MockServer, request: &WebhookRequest) -> PaymentResult<WebhookOutcome> {
    AuraPay::with_api_base(server.uri())
        .parse_webhook(&settings(), request)
        .await
}

fn created(order: &str, amount: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": INVOICE, "order_id": order, "shop_id": SHOP, "amount": amount,
        "comment": "Стандарт · 3 месяца", "service": null,
        "expires_at": "2026-11-18 13:00:00", "created_at": "2026-11-18 12:00:00",
        "status": "PENDING",
        "payment_data": {"url": "https://payform.aurapay.tech/pay/9beea835"}
    }))
}

async fn create(server: &MockServer, method: Option<&str>) -> PaymentResult<Invoice> {
    let mut request = invoice(rub(150_000), Some("client@example.com"));
    request.webhook_url = HOOK.into();
    request.method = method.map(str::to_string);
    AuraPay::with_api_base(server.uri())
        .create_invoice(&settings(), &request)
        .await
}

#[test]
fn подпись_aurapay_по_векторам() {
    for (body, expected) in [
        (V1, V1_SIGNATURE),
        (V2, V2_SIGNATURE),
        (V3, V3_SIGNATURE),
        (V4, V4_SIGNATURE),
    ] {
        assert_eq!(signature(SECRET, body.as_bytes()), expected, "{body}");
        let parsed = Json::parse(body.as_bytes()).unwrap();
        assert!(verify(SECRET, &parsed, expected));
        assert!(verify(SECRET, &parsed, &expected.to_ascii_uppercase()));
        assert!(!verify("другой ключ", &parsed, expected));
        assert!(!verify(SECRET, &parsed, &expected[..62]));
        assert!(!verify(SECRET, &parsed, ""));
    }
    assert_eq!(signature(SECRET, b"[1,2]"), "");
    assert_eq!(signature(SECRET, b"not json"), "");
}

#[tokio::test]
async fn уведомление_с_верной_подписью_засчитывает_оплату() {
    let server = api(remote("PAID", json!(1500.0))).await;
    let outcome = parse(&server, &signed(event("PAID", "1500.00")))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.event_key, format!("{INVOICE}:paid"));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some(INVOICE));
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.reply, WebhookReply::ok());
    assert_eq!(outcome.signal, None);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn статус_и_сумма_берутся_из_api_а_не_из_уведомления() {
    let server = api(remote("PENDING", json!(1500))).await;
    let outcome = parse(&server, &signed(event("PAID", "1500.00")))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    let server = api(remote("PAID", json!(15.0))).await;
    let outcome = parse(&server, &signed(event("PAID", "1500.00")))
        .await
        .unwrap();
    assert_eq!(outcome.amount, Some(rub(1_500)));
    let mut other = remote("PAID", json!(1500));
    other["order_id"] = json!(Uuid::from_u128(9).to_string());
    let server = api(other).await;
    assert!(matches!(
        parse(&server, &signed(event("PAID", "1500.00"))).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut foreign = remote("PAID", json!(1500));
    foreign["shop_id"] = json!("9bee9309-2585-4332-b63d-e1d897f7ce84");
    let server = api(foreign).await;
    assert!(matches!(
        parse(&server, &signed(event("PAID", "1500.00"))).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut swapped = remote("PAID", json!(1500));
    swapped["id"] = json!("0b9d77aa-1c2d-4e3f-8a4b-5c6d7e8f9012");
    let server = api(swapped).await;
    assert!(matches!(
        parse(&server, &signed(event("PAID", "1500.00"))).await,
        Err(PaymentError::BadPayload(_))
    ));
    let unknown = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoice/status"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"error": "Invoice not found", "data": []})),
        )
        .mount(&unknown)
        .await;
    assert!(matches!(
        parse(&unknown, &signed(event("PAID", "1500.00"))).await,
        Err(PaymentError::BadPayload(_))
    ));
    let down = failing_api(502).await;
    assert_eq!(
        parse(&down, &signed(event("PAID", "1500.00"))).await,
        Err(PaymentError::Http { status: 502 })
    );
}

#[tokio::test]
async fn без_подписи_или_с_чужой_подписью_уведомление_отклоняется() {
    let server = api(remote("PAID", json!(1500))).await;
    let body = event("PAID", "1500.00");
    assert_eq!(
        parse(&server, &hook(&[], body.clone())).await,
        Err(PaymentError::BadSignature)
    );
    let forged = signature("угаданный ключ", &body);
    assert_eq!(
        parse(
            &server,
            &hook(&[("X-SIGNATURE", forged.as_str())], body.clone())
        )
        .await,
        Err(PaymentError::BadSignature)
    );
    let genuine = signature(SECRET, &body);
    let tampered = event("PAID", "15000.00");
    assert_eq!(
        parse(
            &server,
            &hook(&[("X-SIGNATURE", genuine.as_str())], tampered)
        )
        .await,
        Err(PaymentError::BadSignature)
    );
    assert_eq!(
        parse(
            &server,
            &hook(&[("X-SIGNATURE", genuine.as_str())], "id=1&status=PAID")
        )
        .await,
        Err(PaymentError::BadSignature)
    );
    let mut suffixed = signed(body);
    suffixed.route_suffix = Some("pay".into());
    assert!(matches!(
        parse(&server, &suffixed).await,
        Err(PaymentError::BadPayload(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn порядок_полей_и_пробелы_в_теле_не_мешают_подписи() {
    let server = api(remote("PAID", json!(1500))).await;
    let compact = event("PAID", "1500.00");
    let sign = signature(SECRET, &compact);
    let value: serde_json::Value = serde_json::from_slice(&compact).unwrap();
    let pretty = serde_json::to_vec_pretty(&value).unwrap();
    let outcome = parse(&server, &hook(&[("X-SIGNATURE", sign.as_str())], pretty))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
}

#[tokio::test]
async fn повтор_того_же_уведомления_даёт_то_же_событие() {
    let server = api(remote("PAID", json!(1500))).await;
    let first = parse(&server, &signed(event("PAID", "1500.00")))
        .await
        .unwrap();
    let second = parse(&server, &signed(event("PAID", "1500.00")))
        .await
        .unwrap();
    assert_eq!(first.event_key, second.event_key);
    assert_eq!(first, second);
}

#[tokio::test]
async fn статусы_aurapay() {
    for (status, expected, key) in [
        ("PAID", ProviderStatus::Paid, format!("{INVOICE}:paid")),
        (
            "EXPIRED",
            ProviderStatus::Canceled,
            format!("{INVOICE}:expired"),
        ),
        (
            "PENDING",
            ProviderStatus::Pending,
            format!("{INVOICE}:pending"),
        ),
        (
            "REFUNDED",
            ProviderStatus::Refunded,
            format!("refund:{INVOICE}"),
        ),
        ("ERROR", ProviderStatus::Failed, format!("{INVOICE}:error")),
        ("paid", ProviderStatus::Paid, format!("{INVOICE}:paid")),
        (
            "SOMETHING_NEW",
            ProviderStatus::Pending,
            format!("{INVOICE}:something_new"),
        ),
    ] {
        let server = api(remote(status, json!(1500))).await;
        let outcome = parse(&server, &signed(event(status, "1500.00")))
            .await
            .unwrap();
        assert_eq!(outcome.status, expected, "{status}");
        assert_eq!(outcome.event_key, key, "{status}");
    }
}

#[tokio::test]
async fn сумма_не_сошлась() {
    let expected = rub(150_000);
    let checked = |amount: serde_json::Value| async move {
        let server = api(remote("PAID", amount)).await;
        parse(&server, &signed(event("PAID", "1500.00"))).await
    };
    let under = checked(json!("1499.99")).await.unwrap();
    assert!(matches!(
        check_paid_amount(expected, under.amount),
        AmountCheck::Underpaid { .. }
    ));
    let over = checked(json!(1575)).await.unwrap();
    assert!(matches!(
        check_paid_amount(expected, over.amount),
        AmountCheck::Overpaid { .. }
    ));
    let exact = checked(json!("1500.00")).await.unwrap();
    assert_eq!(check_paid_amount(expected, exact.amount), AmountCheck::Ok);
    for amount in [json!("1500.001"), json!(""), json!("1 500.00"), json!(-1)] {
        assert!(
            matches!(
                checked(amount.clone()).await,
                Err(PaymentError::BadPayload(_))
            ),
            "{amount}"
        );
    }
}

#[tokio::test]
async fn уведомление_другой_кассы_не_засчитывается() {
    let server = api(remote("PAID", json!(1500))).await;
    let mut value: serde_json::Value = serde_json::from_slice(&event("PAID", "1500.00")).unwrap();
    value["shop_id"] = json!("9bee9309-2585-4332-b63d-e1d897f7ce84");
    let outcome = parse(&server, &signed(serde_json::to_vec(&value).unwrap())).await;
    assert!(matches!(outcome, Err(PaymentError::BadPayload(_))));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn подписки_и_выплаты_пропускаются() {
    let server = MockServer::start().await;
    let subscription = Json::parse(V3.as_bytes()).unwrap();
    let mut body = subscription.clone();
    body.push("shop_id", Json::from(SHOP));
    let outcome = parse(&server, &signed(body.to_bytes())).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    assert_eq!(outcome.payment, None);
    assert_eq!(outcome.event_key, "subscription:PAYMENT_ERROR");
    let payout = parse(&server, &signed(V4.as_bytes().to_vec()))
        .await
        .unwrap();
    assert_eq!(payout.payment, None);
    assert_eq!(payout.amount, None);
    assert!(payout.event_key.starts_with("payout:"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn счёт_aurapay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoice/create"))
        .and(header("X-ApiKey", KEY))
        .and(header("X-ShopId", SHOP))
        .and(body_json(json!({
            "amount": 1500.00,
            "order_id": PAYMENT.to_string(),
            "comment": "Стандарт · 3 месяца",
            "lifetime": 60,
            "callback_url": HOOK,
            "success_url": "https://panel.example.com/cabinet/payment-wait",
            "fail_url": "https://panel.example.com/cabinet/payment-wait"
        })))
        .respond_with(created(&PAYMENT.to_string(), json!(1500.00)))
        .mount(&server)
        .await;
    let invoice = create(&server, None).await.unwrap();
    assert_eq!(invoice.pay_url, "https://payform.aurapay.tech/pay/9beea835");
    assert_eq!(invoice.external_id.as_deref(), Some(INVOICE));
    assert_eq!(invoice.expires_at, None);
    let sent = &server.received_requests().await.unwrap()[0];
    let raw = String::from_utf8(sent.body.clone()).unwrap();
    assert!(raw.contains("\"amount\":1500.00"), "{raw}");
}

#[tokio::test]
async fn способ_оплаты_уходит_в_счёт() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoice/create"))
        .respond_with(created(&PAYMENT.to_string(), json!(1500)))
        .mount(&server)
        .await;
    for (chosen, expected) in [
        (Some("sbp"), Some("sbp")),
        (Some("card"), Some("card")),
        (Some("page"), None),
        (None, None),
    ] {
        create(&server, chosen).await.unwrap();
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent.last().unwrap().body).unwrap();
        assert_eq!(
            body.get("service").and_then(|s| s.as_str()),
            expected,
            "{chosen:?}"
        );
    }
    assert!(matches!(
        create(&server, Some("crypto")).await,
        Err(PaymentError::Rejected(_))
    ));
}

#[tokio::test]
async fn ответ_на_счёт_проверяется() {
    for (response, expect_bad) in [
        (created("other-order", json!(1500.00)), true),
        (created(&PAYMENT.to_string(), json!(1499.00)), true),
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "id": INVOICE, "order_id": PAYMENT.to_string(), "amount": 1500,
                "payment_data": {"url": "http://payform.aurapay.tech/pay/1"}
            })),
            true,
        ),
        (
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": "Order id is not unique", "data": []})),
            false,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/invoice/create"))
            .respond_with(response)
            .mount(&server)
            .await;
        let result = create(&server, None).await;
        if expect_bad {
            assert!(
                matches!(result, Err(PaymentError::BadPayload(_))),
                "{result:?}"
            );
        } else {
            assert_eq!(
                result,
                Err(PaymentError::Rejected(
                    "в AuraPay уже есть счёт с таким номером заказа".into()
                ))
            );
        }
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoice/create"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": "Api key not correct", "data": []})),
        )
        .mount(&server)
        .await;
    assert_eq!(
        create(&server, None).await,
        Err(PaymentError::Rejected("AuraPay не приняла API-ключ".into()))
    );
    let mut usd = invoice(usd(1_000), None);
    usd.webhook_url = HOOK.into();
    assert!(matches!(
        AuraPay::with_api_base(server.uri())
            .create_invoice(&settings(), &usd)
            .await,
        Err(PaymentError::UnsupportedCurrency(_))
    ));
}

#[test]
fn срок_счёта_в_минутах() {
    let mut request = invoice(rub(100), None);
    for (seconds, minutes) in [
        (3600, 60),
        (61, 2),
        (1, 1),
        (0, 1),
        (60 * 60 * 24 * 60, 43_200),
    ] {
        request.expires_in = chrono::Duration::seconds(seconds);
        assert_eq!(lifetime(&request), minutes, "{seconds}");
    }
}

#[tokio::test]
async fn опрос_статуса_aurapay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoice/status"))
        .and(header("X-ApiKey", KEY))
        .and(body_json(json!({"id": INVOICE})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": INVOICE, "order_id": PAYMENT.to_string(), "shop_id": SHOP,
            "amount": 1500.0, "status": "PAID", "service": "sbp",
            "payment_data": {"url": "https://payform.aurapay.tech/pay/9beea835"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/invoice/status"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"error": "Invoice not found", "data": []})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    let provider = AuraPay::with_api_base(server.uri());
    let outcome = provider
        .fetch_status(&settings(), INVOICE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(
        provider
            .fetch_status(&settings(), "0b9d77aa-1c2d-4e3f-8a4b-5c6d7e8f9012")
            .await,
        Err(PaymentError::Rejected("счёт не найден в AuraPay".into()))
    );
    assert!(provider.fetch_status(&settings(), "../x").await.is_err());
}

#[tokio::test]
async fn проверка_подключения_aurapay_по_балансу_кассы() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/shop/balance"))
        .and(header("X-ApiKey", KEY))
        .and(header("X-ShopId", SHOP))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"balance": 1250.5, "balance_hold": 0})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/shop/balance"))
        .and(header("X-ShopId", SHOP))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": "Api key not correct", "data": []})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/shop/balance"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": "Shop Id not correct", "data": []})),
        )
        .with_priority(20)
        .mount(&server)
        .await;
    let provider = AuraPay::with_api_base(server.uri());
    let context = CheckContext {
        webhook_url: Some(HOOK.into()),
    };
    let ok = provider.check_connection(&settings(), &context).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
    assert!(
        ok.details
            .iter()
            .any(|d| d.starts_with("Баланс кассы: 1") && d.ends_with('₽')),
        "{:?}",
        ok.details
    );
    assert!(ok.details.iter().any(|d| d.contains(HOOK)));
    assert!(ok.details.last().unwrap().contains("Секретный ключ"));
    let mut wrong_key = settings();
    wrong_key.set("api_key", json!("ak_wrong"));
    let keys = provider.check_connection(&wrong_key, &context).await;
    assert_eq!(keys.status, CheckStatus::KeysRejected);
    let mut wrong_shop = settings();
    wrong_shop.set("shop_id", json!("9bee9309-2585-4332-b63d-e1d897f7ce84"));
    let shop = provider.check_connection(&wrong_shop, &context).await;
    assert_eq!(shop.status, CheckStatus::SettingsRejected);
    let bare = provider
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert!(
        bare.details
            .iter()
            .any(|d| d.contains("Адрес панели не задан"))
    );
    let down = failing_api(502).await;
    let unreachable = AuraPay::with_api_base(down.uri())
        .check_connection(&settings(), &context)
        .await;
    assert_eq!(unreachable.status, CheckStatus::Unreachable);
    let closed = AuraPay::with_api_base(closed_api())
        .check_connection(&settings(), &context)
        .await;
    assert_eq!(closed.status, CheckStatus::Unreachable);
}

#[test]
fn настройки_и_способы_aurapay() {
    let provider = AuraPay::new();
    assert!(provider.is_configured(&settings()));
    assert!(provider.accepts_http_webhooks());
    assert!(!provider.supports_refunds());
    assert!(!provider.verifies_webhooks_by_api());
    assert_eq!(provider.currencies(), &[Currency::Rub]);
    let codes: Vec<&str> = provider
        .methods(Currency::Rub)
        .iter()
        .map(|m| m.code)
        .collect();
    assert_eq!(codes, ["sbp", "card", "page"]);
    assert!(provider.methods(Currency::Usd).is_empty());
    assert_eq!(
        provider.client_label(Currency::Rub, &settings()).title,
        "Карта или СБП"
    );
    for text in provider
        .methods(Currency::Rub)
        .iter()
        .flat_map(|m| [m.title, m.hint])
        .chain([LABEL.title, LABEL.hint])
    {
        assert!(crate::english::text(text).is_some(), "{text}");
    }
    let schema = provider.settings_schema();
    assert_eq!(
        schema
            .iter()
            .filter(|f| f.is_secret())
            .map(|f| f.key)
            .collect::<Vec<_>>(),
        ["api_key", "secret_key"]
    );
    let mut not_uuid = settings();
    not_uuid.set("shop_id", json!("12345"));
    assert!(provider.validate_settings(&not_uuid).is_err());
    let mut spaced = settings();
    spaced.set("secret_key", json!("ab cd"));
    assert!(provider.validate_settings(&spaced).is_err());
    assert!(!provider.is_configured(&ProviderConfig::default()));
}
