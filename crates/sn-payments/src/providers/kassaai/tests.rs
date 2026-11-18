use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::ids::PaymentId;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const SHOP: &str = "777";
const KEY: &str = "fk_api_key_3c9d";
const SECRET: &str = "sw2_secret_7f1e";
const HOOK: &str = "https://panel.example.com/api/webhooks/pay/kassa_ai";
const RETURN: &str = "https://panel.example.com/cabinet/payment-wait";
const PANEL_HOOK: &str = "https://203.0.113.7/api/webhooks/pay/kassa_ai";

fn settings() -> ProviderConfig {
    config(&[
        ("shop_id", json!(SHOP)),
        ("api_key", json!(KEY)),
        ("secret_word_2", json!(SECRET)),
    ])
}

fn notice_body(amount: &str, order: &str, sign: &str) -> String {
    format!(
        "MERCHANT_ID={SHOP}&AMOUNT={amount}&intid=652367&MERCHANT_ORDER_ID={order}&P_EMAIL=client%40example.com&P_PHONE=&CUR_ID=44&payer_account=2200xxxxxxxx1234&commission=7.5&SIGN={sign}"
    )
}

fn notice(amount: &str) -> WebhookRequest {
    let order = PAYMENT.to_string();
    let sign = notice_signature(SHOP, amount, SECRET, &order);
    hook(
        &[("Content-Type", "application/x-www-form-urlencoded")],
        notice_body(amount, &order, &sign),
    )
}

async fn parse(request: &WebhookRequest) -> PaymentResult<WebhookOutcome> {
    KassaAi::new().parse_webhook(&settings(), request).await
}

fn fields_of(request: &Request) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_slice::<serde_json::Value>(&request.body)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn text_of(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn signed_correctly(request: &Request) -> bool {
    let fields = fields_of(request);
    let texts: Vec<(String, String)> = fields
        .iter()
        .map(|(k, v)| (k.clone(), text_of(v)))
        .collect();
    let pairs: Vec<(&str, &str)> = texts
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    fields.get("signature").and_then(|s| s.as_str()) == Some(signature(KEY, &pairs).as_str())
}

struct Checked(ResponseTemplate);

impl Respond for Checked {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if signed_correctly(request) {
            self.0.clone()
        } else {
            ResponseTemplate::new(400)
                .set_body_json(json!({"type": "error", "message": "Wrong signature"}))
        }
    }
}

fn order(status: i64, amount: serde_json::Value) -> serde_json::Value {
    json!({
        "type": "success", "pages": 1,
        "orders": [{
            "merchant_order_id": PAYMENT.to_string(), "fk_order_id": 652367,
            "amount": amount, "currency": "RUB", "email": "client@example.com",
            "account": "2200xxxxxxxx1234", "date": "2026-11-18 12:28:24", "status": status
        }]
    })
}

async fn orders_api(status: i64, amount: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orders"))
        .and(body_partial_json(json!({"shopId": 777, "orderId": 652367})))
        .respond_with(Checked(
            ResponseTemplate::new(200).set_body_json(order(status, amount)),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/orders"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"type": "success", "pages": 0, "orders": []})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

#[test]
fn подпись_запроса_по_векторам() {
    assert_eq!(
        signature(KEY, &[("shopId", "777"), ("nonce", "1700000000123456789")]),
        "46c1b0c7ae4575ca6dc8901d4623954a0b470f8670bc3df7df4ec6bfe3fb912e"
    );
    assert_eq!(
        signature(
            KEY,
            &[
                ("shopId", "777"),
                ("nonce", "1700000000123456789"),
                ("paymentId", "6a1c0c8e-3f4b-4d2a-9b7e-51c02d3e4f50"),
                ("i", "44"),
                ("email", "client@example.com"),
                ("ip", "127.0.0.1"),
                ("amount", "1500"),
                ("currency", "RUB"),
                (
                    "notification_url",
                    "https://panel.example.com/api/webhooks/pay/kassaai",
                ),
                ("success_url", RETURN),
                ("failure_url", RETURN),
                ("signature", "игнорируется"),
            ]
        ),
        "342bfe804ebb85e97c0dd1d416e105a4316e579307043d642919f1b60beb9a52"
    );
}

#[test]
fn подпись_уведомления_по_векторам() {
    assert_eq!(
        notice_signature("123", "100", "secret2", "test_order"),
        "30c76991785c43f10af2079878acd3d6"
    );
    assert_eq!(
        notice_signature(
            "777",
            "1500.5",
            "sw2_Секрет",
            "6a1c0c8e-3f4b-4d2a-9b7e-51c02d3e4f50"
        ),
        "eefb1ecdb21be51843afce3808db4709"
    );
}

#[test]
fn сумма_в_запросе_без_лишних_нулей() {
    assert_eq!(plain_amount(rub(150_000)), "1500");
    assert_eq!(plain_amount(rub(150_050)), "1500.5");
    assert_eq!(plain_amount(rub(150_055)), "1500.55");
    assert_eq!(plain_amount(rub(5)), "0.05");
}

#[tokio::test]
async fn уведомление_с_верной_подписью_засчитывает_оплату() {
    let outcome = parse(&notice("1500")).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.event_key, "652367:paid");
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid, None);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(
        outcome.reply,
        WebhookReply::Text {
            status: 200,
            body: "YES".into()
        }
    );
    let cents = parse(&notice("1500.50")).await.unwrap();
    assert_eq!(cents.amount, Some(rub(150_050)));
}

#[tokio::test]
async fn без_подписи_или_с_чужой_подписью_уведомление_отклоняется() {
    let order = PAYMENT.to_string();
    let forged = notice_signature(SHOP, "1500", "угаданное слово", &order);
    assert_eq!(
        parse(&hook(&[], notice_body("1500", &order, &forged))).await,
        Err(PaymentError::BadSignature)
    );
    let genuine = notice_signature(SHOP, "1500", SECRET, &order);
    assert_eq!(
        parse(&hook(&[], notice_body("15000", &order, &genuine))).await,
        Err(PaymentError::BadSignature)
    );
    let unsigned = format!("MERCHANT_ID={SHOP}&AMOUNT=1500&intid=1&MERCHANT_ORDER_ID={order}");
    assert_eq!(
        parse(&hook(&[], unsigned)).await,
        Err(PaymentError::BadSignature)
    );
    let upper = notice_body("1500", &order, &genuine.to_ascii_uppercase());
    assert_eq!(
        parse(&hook(&[], upper)).await.unwrap().status,
        ProviderStatus::Paid
    );
    let mut suffixed = notice("1500");
    suffixed.route_suffix = Some("pay".into());
    assert!(matches!(
        parse(&suffixed).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn уведомление_другого_магазина_не_засчитывается() {
    let order = PAYMENT.to_string();
    let sign = notice_signature("778", "1500", SECRET, &order);
    let body = format!("MERCHANT_ID=778&AMOUNT=1500&intid=5&MERCHANT_ORDER_ID={order}&SIGN={sign}");
    assert!(matches!(
        parse(&hook(&[], body)).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn уведомление_формой_multipart_и_запросом_get() {
    let order = PAYMENT.to_string();
    let sign = notice_signature(SHOP, "1500", SECRET, &order);
    let parts = [
        ("MERCHANT_ID", SHOP),
        ("AMOUNT", "1500"),
        ("intid", "652367"),
        ("MERCHANT_ORDER_ID", order.as_str()),
        ("SIGN", sign.as_str()),
    ];
    let mut body = String::new();
    for (name, value) in parts {
        body.push_str(&format!(
            "--XyZ42\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str("--XyZ42--\r\n");
    let multipart = hook(
        &[("Content-Type", "multipart/form-data; boundary=XyZ42")],
        body,
    );
    assert_eq!(
        parse(&multipart).await.unwrap(),
        parse(&notice("1500")).await.unwrap()
    );
    let mut get = hook(&[], "");
    get.query = Some(notice_body("1500", &order, &sign));
    assert_eq!(parse(&get).await.unwrap().status, ProviderStatus::Paid);
    let broken = hook(
        &[],
        "--XyZ42\r\nContent-Disposition: form-data; name=\"AMOUNT\"\r\n\r\n1500",
    );
    assert!(parse(&broken).await.is_err());
}

#[tokio::test]
async fn повтор_уведомления_даёт_то_же_событие() {
    let first = parse(&notice("1500")).await.unwrap();
    let second = parse(&notice("1500")).await.unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn сумма_не_сошлась() {
    let expected = rub(150_000);
    let under = parse(&notice("1499.99")).await.unwrap();
    assert!(matches!(
        check_paid_amount(expected, under.amount),
        AmountCheck::Underpaid { .. }
    ));
    assert_eq!(
        check_paid_amount(expected, parse(&notice("1500.00")).await.unwrap().amount),
        AmountCheck::Ok
    );
    for amount in ["1500.001", "-1", "1,5"] {
        assert!(
            matches!(
                parse(&notice(amount)).await,
                Err(PaymentError::BadPayload(_))
            ),
            "{amount}"
        );
    }
}

async fn create(server: &MockServer, request: &InvoiceRequest) -> PaymentResult<Invoice> {
    KassaAi::with_api_base(server.uri())
        .create_invoice(&settings(), request)
        .await
}

fn hooked(amount: i64, email: Option<&str>) -> InvoiceRequest {
    let mut request = invoice(rub(amount), email);
    request.webhook_url = PANEL_HOOK.into();
    request
}

fn created() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "type": "success", "orderId": 652367,
        "orderHash": "bd4161db429848651499aabcb1d89330",
        "location": "https://pay.freekassa.net/form/652367/bd4161db429848651499aabcb1d89330"
    }))
}

#[tokio::test]
async fn счёт_kassaai() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orders/create"))
        .and(body_partial_json(json!({
            "shopId": 777,
            "paymentId": PAYMENT.to_string(),
            "i": 44,
            "email": "client@example.com",
            "ip": "203.0.113.7",
            "amount": 1500,
            "currency": "RUB",
            "notification_url": PANEL_HOOK,
            "success_url": RETURN,
            "failure_url": RETURN
        })))
        .respond_with(Checked(created()))
        .mount(&server)
        .await;
    let invoice = create(&server, &hooked(150_000, Some("client@example.com")))
        .await
        .unwrap();
    assert_eq!(
        invoice.pay_url,
        "https://pay.freekassa.net/form/652367/bd4161db429848651499aabcb1d89330"
    );
    assert_eq!(invoice.external_id.as_deref(), Some("652367"));
    let sent = &server.received_requests().await.unwrap()[0];
    let raw = String::from_utf8(sent.body.clone()).unwrap();
    assert!(raw.contains("\"amount\":1500,"), "{raw}");
    assert!(signed_correctly(sent));
}

#[tokio::test]
async fn способ_оплаты_и_почта_уходят_в_заказ() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orders/create"))
        .respond_with(Checked(created()))
        .mount(&server)
        .await;
    for (chosen, code) in [
        (None, 44),
        (Some("sbp"), 44),
        (Some("card"), 36),
        (Some("sberpay"), 43),
    ] {
        let mut request = hooked(150_050, None);
        request.method = chosen.map(str::to_string);
        create(&server, &request).await.unwrap();
        let sent = server.received_requests().await.unwrap();
        let fields = fields_of(sent.last().unwrap());
        assert_eq!(fields["i"], json!(code), "{chosen:?}");
        assert_eq!(fields["amount"], json!(1500.5));
        assert_eq!(fields["email"], json!("5093118422@telegram.org"));
    }
    let mut request = hooked(150_000, None);
    request.method = Some("crypto".into());
    assert!(matches!(
        create(&server, &request).await,
        Err(PaymentError::Rejected(_))
    ));
    let mut anonymous = hooked(150_000, None);
    anonymous.client.telegram_id = None;
    create(&server, &anonymous).await.unwrap();
    let sent = server.received_requests().await.unwrap();
    assert_eq!(
        fields_of(sent.last().unwrap())["email"],
        json!(format!("{}@telegram.org", CLIENT.simple()))
    );
}

#[tokio::test]
async fn у_каждого_запроса_новый_nonce_больше_прежнего() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orders/create"))
        .respond_with(Checked(created()))
        .mount(&server)
        .await;
    let provider = KassaAi::with_api_base(server.uri());
    for _ in 0..3 {
        provider
            .create_invoice(&settings(), &hooked(150_000, None))
            .await
            .unwrap();
    }
    let nonces: Vec<i64> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| fields_of(r)["nonce"].as_i64().unwrap())
        .collect();
    assert!(nonces.windows(2).all(|w| w[1] > w[0]), "{nonces:?}");
}

#[tokio::test]
async fn ошибки_kassaai_понятны_оператору() {
    for (status, body, expected) in [
        (
            400,
            json!({"type": "error", "message": "Wrong signature"}),
            PaymentError::Rejected(
                "Kassa.ai не приняла подпись запроса: проверьте API-ключ".into(),
            ),
        ),
        (
            400,
            json!({"type": "error", "message": "Shop not found"}),
            PaymentError::Rejected("Kassa.ai не нашла магазин с таким ID".into()),
        ),
        (
            401,
            json!({}),
            PaymentError::Rejected(util::KEYS_REFUSED.into()),
        ),
        (
            200,
            json!({"type": "error", "message": "Метод оплаты недоступен"}),
            PaymentError::Rejected("Метод оплаты недоступен".into()),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        assert_eq!(create(&server, &hooked(150_000, None)).await, Err(expected));
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "success", "orderId": 1, "location": "http://pay.freekassa.net/form/1/x"
        })))
        .mount(&server)
        .await;
    assert!(matches!(
        create(&server, &hooked(150_000, None)).await,
        Err(PaymentError::BadPayload(_))
    ));
    assert!(matches!(
        create(&server, &invoice(usd(100), None)).await,
        Err(PaymentError::UnsupportedCurrency(_))
    ));
}

#[tokio::test]
async fn ip_покупателя_берётся_по_адресу_панели() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orders/create"))
        .respond_with(Checked(created()))
        .mount(&server)
        .await;
    let provider = KassaAi::with_api_base(server.uri());
    for (hook, allowed) in [
        (
            "https://localhost/api/webhooks/pay/kassa_ai",
            &["127.0.0.1", "::1"][..],
        ),
        (
            "https://[2001:db8::7]/api/webhooks/pay/kassa_ai",
            &["2001:db8::7"][..],
        ),
        ("не адрес", &["127.0.0.1"][..]),
    ] {
        let mut request = hooked(150_000, None);
        request.webhook_url = hook.into();
        provider
            .create_invoice(&settings(), &request)
            .await
            .unwrap();
        let sent = server.received_requests().await.unwrap();
        let ip = fields_of(sent.last().unwrap())["ip"].clone();
        assert!(allowed.iter().any(|a| ip == json!(a)), "{hook}: {ip}");
    }
    let errors = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"msg": "empty_shopId"})))
        .mount(&errors)
        .await;
    assert_eq!(
        create(&errors, &hooked(150_000, None)).await,
        Err(PaymentError::Rejected("empty_shopId".into()))
    );
}

#[tokio::test]
async fn опрос_статуса_kassaai() {
    for (code, expected, key) in [
        (0, ProviderStatus::Pending, "652367:0"),
        (1, ProviderStatus::Paid, "652367:paid"),
        (6, ProviderStatus::Refunded, "refund:652367"),
        (8, ProviderStatus::Failed, "652367:8"),
        (9, ProviderStatus::Canceled, "652367:9"),
    ] {
        let server = orders_api(code, json!(1500)).await;
        let outcome = KassaAi::with_api_base(server.uri())
            .fetch_status(&settings(), "652367")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, expected, "{code}");
        assert_eq!(outcome.event_key, key, "{code}");
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.provider_txid.as_deref(), Some("652367"));
        assert_eq!(outcome.amount, Some(rub(150_000)));
    }
    let server = orders_api(1, json!(1499.5)).await;
    let under = KassaAi::with_api_base(server.uri())
        .fetch_status(&settings(), "652367")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        check_paid_amount(rub(150_000), under.amount),
        AmountCheck::Underpaid { .. }
    ));
    assert_eq!(
        KassaAi::with_api_base(server.uri())
            .fetch_status(&settings(), "1")
            .await,
        Err(PaymentError::Rejected("заказ не найден в Kassa.ai".into()))
    );
    assert!(
        KassaAi::with_api_base(server.uri())
            .fetch_status(&settings(), "abc")
            .await
            .is_err()
    );
}

fn refund_request(amount: i64) -> RefundRequest {
    RefundRequest {
        payment_id: PaymentId(41),
        provider_txid: "652367".into(),
        amount: rub(amount),
        reason: None,
        key: Uuid::from_u128(9),
        email: None,
        description: None,
    }
}

#[tokio::test]
async fn возврат_kassaai_полный_и_частичный() {
    let server = orders_api(1, json!(1500)).await;
    Mock::given(method("POST"))
        .and(path("/orders/refund"))
        .and(body_partial_json(json!({"shopId": 777, "orderId": 652367})))
        .respond_with(Checked(
            ResponseTemplate::new(200).set_body_json(json!({"type": "success", "id": 123})),
        ))
        .mount(&server)
        .await;
    let provider = KassaAi::with_api_base(server.uri());
    for (amount, sent) in [(150_000, json!(1500)), (50_050, json!(500.5))] {
        let outcome = provider
            .refund(&settings(), &refund_request(amount))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            RefundOutcome {
                provider_refund_id: Some("652367:refund:123".into()),
                completed: true
            }
        );
        let requests = server.received_requests().await.unwrap();
        let refund = requests
            .iter()
            .rev()
            .find(|r| r.url.path() == "/orders/refund")
            .unwrap();
        assert_eq!(fields_of(refund)["orderAmount"], sent);
        assert!(signed_correctly(refund));
    }
    assert!(matches!(
        provider.refund(&settings(), &refund_request(150_001)).await,
        Err(PaymentError::Rejected(_))
    ));
    let twice = orders_api(6, json!(1500)).await;
    assert_eq!(
        KassaAi::with_api_base(twice.uri())
            .refund(&settings(), &refund_request(150_000))
            .await,
        Err(PaymentError::Rejected(REFUND_TWICE.into()))
    );
    let unpaid = orders_api(0, json!(1500)).await;
    assert!(matches!(
        KassaAi::with_api_base(unpaid.uri())
            .refund(&settings(), &refund_request(150_000))
            .await,
        Err(PaymentError::Rejected(_))
    ));
    for request in twice.received_requests().await.unwrap() {
        assert_ne!(request.url.path(), "/orders/refund");
    }
}

#[tokio::test]
async fn проверка_подключения_kassaai() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/balance"))
        .respond_with(Checked(ResponseTemplate::new(200).set_body_json(json!({
            "type": "success", "balance": [{"currency": "RUB", "value": 743.43}, {"currency": "USD", "value": 0}]
        }))))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/currencies"))
        .respond_with(Checked(ResponseTemplate::new(200).set_body_json(json!({
            "type": "success", "currencies": [
                {"id": 44, "name": "СБП (API)", "currency": "RUB", "is_enabled": 1, "is_favorite": 0},
                {"id": 36, "name": "Card RUB API", "currency": "RUB", "is_enabled": 0, "is_favorite": 0}
            ]
        }))))
        .mount(&server)
        .await;
    let provider = KassaAi::with_api_base(server.uri());
    let context = CheckContext {
        webhook_url: Some(HOOK.into()),
    };
    let ok = provider.check_connection(&settings(), &context).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
    assert!(
        ok.details[0].starts_with("Баланс магазина: "),
        "{:?}",
        ok.details
    );
    assert!(
        ok.details
            .iter()
            .any(|d| d.contains("не включены способы: Карта, SberPay")),
        "{:?}",
        ok.details
    );
    assert!(
        ok.details
            .iter()
            .any(|d| d.contains(HOOK) && d.contains("168.119.157.136"))
    );
    assert!(ok.details.last().unwrap().contains("Секретное слово 2"));
    let mut wrong = settings();
    wrong.set("api_key", json!("fk_wrong"));
    let rejected = provider.check_connection(&wrong, &context).await;
    assert_eq!(
        rejected.status,
        CheckStatus::KeysRejected,
        "{}",
        rejected.summary
    );
    let refused = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({})))
        .mount(&refused)
        .await;
    let keys = KassaAi::with_api_base(refused.uri())
        .check_connection(&settings(), &context)
        .await;
    assert_eq!(keys.status, CheckStatus::KeysRejected);
    assert_eq!(keys.summary, "Kassa.ai не приняла ID магазина и API-ключ");
    let down = failing_api(502).await;
    assert_eq!(
        KassaAi::with_api_base(down.uri())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::Unreachable
    );
    assert_eq!(
        KassaAi::with_api_base(closed_api())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::Unreachable
    );
}

#[test]
fn настройки_и_способы_kassaai() {
    let provider = KassaAi::new();
    assert!(provider.is_configured(&settings()));
    assert!(provider.accepts_http_webhooks());
    assert!(provider.supports_refunds());
    assert!(!provider.supports_refund_polling());
    assert!(!provider.verifies_webhooks_by_api());
    let codes: Vec<&str> = provider
        .methods(Currency::Rub)
        .iter()
        .map(|m| m.code)
        .collect();
    assert_eq!(codes, ["sbp", "card", "sberpay"]);
    assert!(provider.methods(Currency::Usd).is_empty());
    assert_eq!(
        provider.client_label(Currency::Rub, &settings()).title,
        "СБП"
    );
    for method in provider.methods(Currency::Rub) {
        assert!(
            crate::english::text(method.hint).is_some(),
            "{}",
            method.hint
        );
    }
    assert_eq!(
        provider
            .settings_schema()
            .iter()
            .filter(|f| f.is_secret())
            .map(|f| f.key)
            .collect::<Vec<_>>(),
        ["api_key", "secret_word_2"]
    );
    for (key, value) in [
        ("shop_id", json!("shop")),
        ("shop_id", json!("0")),
        ("api_key", json!("a b")),
        ("secret_word_2", json!("слово")),
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
