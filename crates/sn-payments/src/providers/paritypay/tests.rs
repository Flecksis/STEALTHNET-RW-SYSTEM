use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SHOP: &str = "874dfb1e-dbdb-4747-a1c0-005969725b74";
const KEY_ONE: &str = "pp_secret_one_test";
const KEY_TWO: &str = "pp_secret_two_test";
const INVOICE: &str = "9beea835-0937-4b5c-8f5a-c3a0d0e60346";
const HOOK: &str = "https://panel.example.com/api/webhooks/pay/paritypay";
const P1: &str = r#"{"id":"9beea835-0937-4b5c-8f5a-c3a0d0e60346","order_id":"order-1001","shop_id":"874dfb1e-dbdb-4747-a1c0-005969725b74","amount":"1250.00","credited":1209.01,"comment":"Оплата заказа №1001","service":"sbp","custom_fields":null,"expires":"2026-08-24 13:40:00","created":"2026-08-24 12:40:00","status":"PAID"}"#;
const P1_SIGNATURE: &str = "92e0613800563fbdc56a99d463f79928603e277feb5d99b38caf13ecd466e393";
const P2: &str = r#"{"id":"9beea835-0937-4b5c-8f5a-c3a0d0e60346","order_id":"order-1001","shop_id":"874dfb1e-dbdb-4747-a1c0-005969725b74","amount":"1250.00","credited":1209.0,"comment":null,"service":"card","custom_fields":null,"expires":"2026-08-24 13:40:00","created":"2026-08-24 12:40:00","status":"REFUNDED"}"#;
const P2_SIGNATURE: &str = "f3c98221257f2c819a395b7c2bb97fec8757227f0b3996446213ee4b5eb071e1";

fn settings() -> ProviderConfig {
    config(&[
        ("shop_id", json!(SHOP)),
        ("secret_key", json!(KEY_ONE)),
        ("callback_secret", json!(KEY_TWO)),
    ])
}

fn remote(status: &str, amount: serde_json::Value) -> serde_json::Value {
    json!({
        "id": INVOICE, "order_id": PAYMENT.to_string(), "shop_id": SHOP, "amount": amount,
        "comment": "Стандарт", "service": "sbp", "custom_fields": null,
        "expires": "2026-11-18 13:00:00", "created": "2026-11-18 12:00:00", "status": status,
        "link": format!("https://pay.paritypay.net/{INVOICE}")
    })
}

async fn api(body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/invoice/status"))
        .and(query_param("id", INVOICE))
        .and(header("X-ShopId", SHOP))
        .and(header("X-SecretKey", KEY_ONE))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/invoice/status"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"error": "Invoice not found"})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

fn notice(status: &str, amount: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": INVOICE, "order_id": PAYMENT.to_string(), "shop_id": SHOP, "amount": amount,
        "credited": 1209.01, "comment": "Стандарт", "service": "sbp", "custom_fields": null,
        "expires": "2026-11-18 13:00:00", "created": "2026-11-18 12:00:00", "status": status
    }))
    .unwrap()
}

fn signed(body: Vec<u8>) -> WebhookRequest {
    let sign = signature(KEY_TWO, &body);
    hook(&[("X-SIGNATURE", sign.as_str())], body)
}

async fn parse(server: &MockServer, request: &WebhookRequest) -> PaymentResult<WebhookOutcome> {
    ParityPay::with_api_base(server.uri())
        .parse_webhook(&settings(), request)
        .await
}

#[test]
fn подпись_paritypay_по_векторам() {
    assert_eq!(signature(KEY_TWO, P1.as_bytes()), P1_SIGNATURE);
    assert_eq!(signature(KEY_TWO, P2.as_bytes()), P2_SIGNATURE);
    for (body, expected) in [(P1, P1_SIGNATURE), (P2, P2_SIGNATURE)] {
        let parsed = Json::parse(body.as_bytes()).unwrap();
        assert!(verify(KEY_TWO, &parsed, expected));
        assert!(verify(KEY_TWO, &parsed, &expected.to_ascii_uppercase()));
        assert!(!verify(KEY_ONE, &parsed, expected));
        assert!(!verify(KEY_TWO, &parsed, ""));
    }
    let raw = Json::parse(P2.as_bytes()).unwrap();
    let as_sent = sorted_hmac::signature(KEY_TWO, P2.as_bytes(), Style::Raw);
    assert!(
        verify(KEY_TWO, &raw, &as_sent),
        "число 1209.0 как пришло тоже принимается"
    );
}

#[tokio::test]
async fn оплата_засчитывается_после_проверки_счёта() {
    let server = api(remote("PAID", json!(1500.0))).await;
    let outcome = parse(&server, &signed(notice("PAID", "1500.00")))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.event_key, format!("{INVOICE}:paid"));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some(INVOICE));
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.reply, WebhookReply::ok());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn статус_берётся_из_api_а_не_из_уведомления() {
    let server = api(remote("NEW", json!(1500))).await;
    let outcome = parse(&server, &signed(notice("PAID", "1500.00")))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
}

#[tokio::test]
async fn без_подписи_или_с_чужой_подписью_уведомление_отклоняется() {
    let server = api(remote("PAID", json!(1500))).await;
    let body = notice("PAID", "1500.00");
    assert_eq!(
        parse(&server, &hook(&[], body.clone())).await,
        Err(PaymentError::BadSignature)
    );
    let forged = signature(KEY_ONE, &body);
    assert_eq!(
        parse(
            &server,
            &hook(&[("X-SIGNATURE", forged.as_str())], body.clone())
        )
        .await,
        Err(PaymentError::BadSignature)
    );
    let genuine = signature(KEY_TWO, &body);
    assert_eq!(
        parse(
            &server,
            &hook(
                &[("X-SIGNATURE", genuine.as_str())],
                notice("PAID", "15000.00")
            )
        )
        .await,
        Err(PaymentError::BadSignature)
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn чужая_касса_и_чужой_заказ_не_засчитываются() {
    let server = api(remote("PAID", json!(1500))).await;
    let mut value: serde_json::Value = serde_json::from_slice(&notice("PAID", "1500.00")).unwrap();
    value["shop_id"] = json!("9ff64ae9-4f44-4b0a-a318-8996e94eb0ae");
    assert!(matches!(
        parse(&server, &signed(serde_json::to_vec(&value).unwrap())).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut other = remote("PAID", json!(1500));
    other["order_id"] = json!("чужой заказ");
    let server = api(other).await;
    assert!(matches!(
        parse(&server, &signed(notice("PAID", "1500.00"))).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut foreign = remote("PAID", json!(1500));
    foreign["shop_id"] = json!("9ff64ae9-4f44-4b0a-a318-8996e94eb0ae");
    let server = api(foreign).await;
    assert!(matches!(
        parse(&server, &signed(notice("PAID", "1500.00"))).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn статусы_paritypay() {
    for (status, expected, key) in [
        ("PAID", ProviderStatus::Paid, format!("{INVOICE}:paid")),
        (
            "EXPIRED",
            ProviderStatus::Canceled,
            format!("{INVOICE}:expired"),
        ),
        ("ERROR", ProviderStatus::Failed, format!("{INVOICE}:error")),
        (
            "REFUNDED",
            ProviderStatus::Refunded,
            format!("refund:{INVOICE}"),
        ),
        ("NEW", ProviderStatus::Pending, format!("{INVOICE}:new")),
    ] {
        let server = api(remote(status, json!(1500))).await;
        let outcome = parse(&server, &signed(notice(status, "1500.00")))
            .await
            .unwrap();
        assert_eq!(outcome.status, expected, "{status}");
        assert_eq!(outcome.event_key, key, "{status}");
    }
}

#[tokio::test]
async fn сумма_не_сошлась_и_повтор() {
    let server = api(remote("PAID", json!(1499.99))).await;
    let outcome = parse(&server, &signed(notice("PAID", "1499.99")))
        .await
        .unwrap();
    assert!(matches!(
        check_paid_amount(rub(150_000), outcome.amount),
        AmountCheck::Underpaid { .. }
    ));
    let server = api(remote("PAID", json!(1500))).await;
    let first = parse(&server, &signed(notice("PAID", "1500.00")))
        .await
        .unwrap();
    let second = parse(&server, &signed(notice("PAID", "1500.00")))
        .await
        .unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn подписки_и_выплаты_пропускаются() {
    let server = MockServer::start().await;
    let subscription = serde_json::to_vec(&json!({
        "id": "aeca1ac8-72e3-408b-81e3-0a6028751e3f", "shop_subscription_id": "order-1002",
        "shop_id": SHOP, "amount": "990.00", "client_email": null, "interval": "1m",
        "status": "cancelled", "description": "Премиум"
    }))
    .unwrap();
    let outcome = parse(&server, &signed(subscription)).await.unwrap();
    assert_eq!(outcome.payment, None);
    assert!(outcome.event_key.starts_with("subscription:"));
    let payoff = serde_json::to_vec(&json!({
        "id": "3569a18b-86ad-46c4-ad5a-aef3372dbc40", "debit": "100.00", "amount": "100.00",
        "status": "ERROR", "service": "card", "shop_id": SHOP, "order_id": "p-1",
        "commission": "5.00", "amount_to_payoff": "95.00"
    }))
    .unwrap();
    let outcome = parse(&server, &signed(payoff)).await.unwrap();
    assert!(outcome.event_key.starts_with("payoff:"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

fn created(amount: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(remote("NEW", amount))
}

async fn create(server: &MockServer, chosen: Option<&str>) -> PaymentResult<Invoice> {
    let mut request = invoice(rub(150_000), None);
    request.webhook_url = HOOK.into();
    request.method = chosen.map(str::to_string);
    ParityPay::with_api_base(server.uri())
        .create_invoice(&settings(), &request)
        .await
}

#[tokio::test]
async fn счёт_paritypay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/invoice/create"))
        .and(header("X-ShopId", SHOP))
        .and(header("X-SecretKey", KEY_ONE))
        .and(body_json(json!({
            "order_id": PAYMENT.to_string(),
            "amount": 1500.00,
            "comment": "Стандарт · 3 месяца",
            "expire": 60,
            "callback_url": HOOK,
            "success_url": "https://panel.example.com/cabinet/payment-wait",
            "fail_url": "https://panel.example.com/cabinet/payment-wait"
        })))
        .respond_with(created(json!(1500)))
        .mount(&server)
        .await;
    let invoice = create(&server, None).await.unwrap();
    assert_eq!(
        invoice.pay_url,
        format!("https://pay.paritypay.net/{INVOICE}")
    );
    assert_eq!(invoice.external_id.as_deref(), Some(INVOICE));
    for (chosen, wire) in [
        (Some("sbp"), Some("sbp")),
        (Some("card"), Some("card")),
        (Some("page"), None),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(created(json!(1500)))
            .mount(&server)
            .await;
        create(&server, chosen).await.unwrap();
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(
            body.get("service").and_then(|s| s.as_str()),
            wire,
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
    let mut other_shop = remote("NEW", json!(1500));
    other_shop["shop_id"] = json!("9ff64ae9-4f44-4b0a-a318-8996e94eb0ae");
    let mut other_order = remote("NEW", json!(1500));
    other_order["order_id"] = json!("other");
    let mut plain_http = remote("NEW", json!(1500));
    plain_http["link"] = json!("http://pay.paritypay.net/x");
    for body in [
        remote("NEW", json!(1499)),
        other_shop,
        other_order,
        plain_http,
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let result = create(&server, None).await;
        assert!(
            matches!(result, Err(PaymentError::BadPayload(_))),
            "{result:?}"
        );
    }
    for (status, error, expected) in [
        (
            422,
            "Order id is not unique",
            "в ParityPay уже есть счёт с таким номером заказа",
        ),
        (
            400,
            "Service 'foo' is not a valid.",
            "Service 'foo' is not a valid.",
        ),
        (401, "SecretKey is incorrect", util::KEYS_REFUSED),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({"error": error})))
            .mount(&server)
            .await;
        assert_eq!(
            create(&server, None).await,
            Err(PaymentError::Rejected(expected.into())),
            "{error}"
        );
    }
}

#[test]
fn срок_счёта_в_минутах() {
    let mut request = invoice(rub(100), None);
    for (seconds, minutes) in [(3600, 60), (61, 2), (0, 1), (60 * 60 * 24 * 60, 43_200)] {
        request.expires_in = chrono::Duration::seconds(seconds);
        assert_eq!(expire(&request), minutes, "{seconds}");
    }
}

#[tokio::test]
async fn опрос_статуса_paritypay() {
    let server = api(remote("PAID", json!(1500))).await;
    let provider = ParityPay::with_api_base(server.uri());
    let outcome = provider
        .fetch_status(&settings(), INVOICE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(
        provider
            .fetch_status(&settings(), "0b9d77aa-1c2d-4e3f-8a4b-5c6d7e8f9012")
            .await,
        Err(PaymentError::Rejected("счёт не найден в ParityPay".into()))
    );
}

#[tokio::test]
async fn проверка_подключения_paritypay() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/shop/balance"))
        .and(header("X-SecretKey", KEY_ONE))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "balance": 154300.25, "balance_hold": 4200, "currency": "RUB"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"error": "SecretKey is incorrect"})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    let provider = ParityPay::with_api_base(server.uri());
    let context = CheckContext {
        webhook_url: Some(HOOK.into()),
    };
    let ok = provider.check_connection(&settings(), &context).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
    assert!(
        ok.details[0].starts_with("Баланс кассы: "),
        "{:?}",
        ok.details
    );
    assert!(ok.details.iter().any(|d| d.contains(HOOK)));
    assert!(ok.details.last().unwrap().contains("ключ №2"));
    let mut wrong = settings();
    wrong.set("secret_key", json!("pp_wrong"));
    assert_eq!(
        provider.check_connection(&wrong, &context).await.status,
        CheckStatus::KeysRejected
    );
    for (error, status) in [
        ("ShopId is incorrect", CheckStatus::SettingsRejected),
        ("Shop is inactive", CheckStatus::SettingsRejected),
        ("Магазин не найден", CheckStatus::SettingsRejected),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": error})))
            .mount(&server)
            .await;
        assert_eq!(
            ParityPay::with_api_base(server.uri())
                .check_connection(&settings(), &context)
                .await
                .status,
            status,
            "{error}"
        );
    }
    let down = failing_api(502).await;
    assert_eq!(
        ParityPay::with_api_base(down.uri())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::Unreachable
    );
}

#[test]
fn настройки_и_способы_paritypay() {
    let provider = ParityPay::new();
    assert!(provider.is_configured(&settings()));
    assert!(provider.accepts_http_webhooks());
    assert!(!provider.supports_refunds());
    assert_eq!(provider.currencies(), &[Currency::Rub]);
    let codes: Vec<&str> = provider
        .methods(Currency::Rub)
        .iter()
        .map(|m| m.code)
        .collect();
    assert_eq!(codes, ["sbp", "card", "page"]);
    assert!(provider.methods(Currency::Usd).is_empty());
    for text in provider
        .methods(Currency::Rub)
        .iter()
        .flat_map(|m| [m.title, m.hint])
        .chain([LABEL.title, LABEL.hint])
    {
        assert!(crate::english::text(text).is_some(), "{text}");
    }
    assert_eq!(
        provider
            .settings_schema()
            .iter()
            .filter(|f| f.is_secret())
            .map(|f| f.key)
            .collect::<Vec<_>>(),
        ["secret_key", "callback_secret"]
    );
    for (key, value) in [
        ("shop_id", json!("12345")),
        ("callback_secret", json!("a b")),
    ] {
        let mut broken = settings();
        broken.set(key, value.clone());
        assert!(
            provider.validate_settings(&broken).is_err(),
            "{key} {value}"
        );
    }
    assert!(!provider.is_configured(&ProviderConfig::default()));
}
