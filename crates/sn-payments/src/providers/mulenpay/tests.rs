use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::ids::PaymentId;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SHOP: &str = "12345";
const KEY: &str = "mp_api_9f2c1d7e";
const SECRET: &str = "mp_secret_51ab";
const HOOK: &str = "https://panel.example.com/api/webhooks/pay/mulenpay";

fn settings() -> ProviderConfig {
    config(&[
        ("shop_id", json!(SHOP)),
        ("api_key", json!(KEY)),
        ("secret_key", json!(SECRET)),
    ])
}

fn remote(id: i64, status: i64, amount: &str, currency: &str) -> serde_json::Value {
    json!({
        "success": true,
        "payment": {
            "id": id, "uuid": PAYMENT.to_string(), "amount": amount,
            "currency": currency, "description": "Стандарт", "status": status
        }
    })
}

async fn api_payment(id: i64, body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/payments/{id}")))
        .and(header("Authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"error": "not found", "status": 404})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

fn callback(id: i64, status: &str) -> WebhookRequest {
    hook(
        &[("Content-Type", "application/json")],
        serde_json::to_vec(&json!({
            "id": id, "amount": 1500.50, "currency": "RUB",
            "uuid": PAYMENT.to_string(), "payment_status": status
        }))
        .unwrap(),
    )
}

async fn parse(server: &MockServer, request: &WebhookRequest) -> PaymentResult<WebhookOutcome> {
    MulenPay::with_api_base(server.uri())
        .parse_webhook(&settings(), request)
        .await
}

#[test]
fn подпись_счёта_mulenpay_по_векторам() {
    assert_eq!(
        signature("rub", "1000.50", "5", "secret"),
        "ce023dc594d7c46a4c1a8c1cf01964fceb2d5d6f"
    );
    assert_eq!(
        signature("rub", "1500.00", "12345", "s3cr3t_Ключ"),
        "f1325d3c6febe5ea3dedfc373ddec684bf87106d"
    );
}

#[tokio::test]
async fn счёт_mulenpay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/payments"))
        .and(header("Authorization", format!("Bearer {KEY}").as_str()))
        .and(body_json(json!({
            "currency": "rub",
            "amount": "1500.00",
            "uuid": PAYMENT.to_string(),
            "shopId": 12345,
            "description": "Стандарт · 3 месяца",
            "language": "ru",
            "items": [{
                "description": "Стандарт · 3 месяца",
                "quantity": 1,
                "price": 1500.00,
                "vat_code": 6,
                "payment_subject": 4,
                "payment_mode": 4
            }],
            "sign": signature("rub", "1500.00", SHOP, SECRET),
            "website_url": "https://panel.example.com/cabinet/payment-wait"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "success": true, "paymentUrl": "https://mulenpay.ru/payment/widget/abc", "id": 777
        })))
        .mount(&server)
        .await;
    let mut taxed = settings();
    taxed.set("vat_code", json!("6"));
    let invoice = MulenPay::with_api_base(server.uri())
        .create_invoice(&taxed, &invoice(rub(150_000), None))
        .await
        .unwrap();
    assert_eq!(invoice.pay_url, "https://mulenpay.ru/payment/widget/abc");
    assert_eq!(invoice.external_id.as_deref(), Some("777"));
    assert_eq!(invoice.expires_at, None);
    let sent = &server.received_requests().await.unwrap()[0];
    let raw = String::from_utf8(sent.body.clone()).unwrap();
    assert!(raw.contains("\"amount\":\"1500.00\""), "{raw}");
    assert!(raw.contains("\"price\":1500.00"), "{raw}");
}

#[tokio::test]
async fn ответ_на_счёт_проверяется() {
    let cases = [
        (
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": "unauthorized", "status": 401})),
            PaymentError::Rejected(util::KEYS_REFUSED.into()),
        ),
        (
            ResponseTemplate::new(422).set_body_json(
                json!({"error": "param is missing or the value is empty: items", "status": 422}),
            ),
            PaymentError::Rejected("param is missing or the value is empty: items".into()),
        ),
        (
            ResponseTemplate::new(422)
                .set_body_json(json!({"error": "invalid sign", "status": 422})),
            PaymentError::Rejected(
                "MulenPay не приняла подпись счёта: проверьте секретный ключ и ID магазина".into(),
            ),
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({"success": false})),
            PaymentError::Rejected("MulenPay не создала счёт".into()),
        ),
    ];
    for (response, expected) in cases {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/payments"))
            .respond_with(response)
            .mount(&server)
            .await;
        let result = MulenPay::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(rub(150_000), None))
            .await;
        assert_eq!(result, Err(expected));
    }
    for body in [
        json!({"success": true, "id": 777}),
        json!({"success": true, "paymentUrl": "http://mulenpay.ru/p", "id": 777}),
        json!({"success": true, "paymentUrl": "https://mulenpay.ru/p"}),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/payments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(body))
            .mount(&server)
            .await;
        let result = MulenPay::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(rub(150_000), None))
            .await;
        assert!(
            matches!(result, Err(PaymentError::BadPayload(_))),
            "{result:?}"
        );
    }
    let server = MockServer::start().await;
    assert!(matches!(
        MulenPay::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(usd(1_000), None))
            .await,
        Err(PaymentError::UnsupportedCurrency(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn уведомление_проверяется_запросом_к_api() {
    let server = api_payment(777, remote(777, 3, "1500.00", "rub")).await;
    let outcome = parse(&server, &callback(777, "success")).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.event_key, "777:paid");
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some("777"));
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(
        outcome.reply,
        WebhookReply::Json {
            status: 200,
            body: json!({"success": true})
        }
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/v2/payments/777");
}

#[tokio::test]
async fn статус_берётся_из_api_а_не_из_тела() {
    let server = api_payment(777, remote(777, 0, "1500.00", "rub")).await;
    let outcome = parse(&server, &callback(777, "success")).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    let canceled = api_payment(778, remote(778, 2, "1500.00", "rub")).await;
    let outcome = parse(&canceled, &callback(778, "success")).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Canceled);
}

#[tokio::test]
async fn чужой_номер_платежа_отклоняется() {
    let server = api_payment(777, remote(777, 3, "1500.00", "rub")).await;
    assert_eq!(
        parse(&server, &callback(9999, "success")).await,
        Err(PaymentError::BadSignature)
    );
    let other = api_payment(777, remote(778, 3, "1500.00", "rub")).await;
    assert!(matches!(
        parse(&other, &callback(777, "success")).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn уведомление_без_номера_или_с_лишней_частью_адреса() {
    let server = api_payment(777, remote(777, 3, "1500.00", "rub")).await;
    for body in [
        json!({"uuid": PAYMENT.to_string(), "payment_status": "success"}),
        json!({"id": "../777", "payment_status": "success"}),
        json!({"id": -1}),
    ] {
        let request = hook(&[], serde_json::to_vec(&body).unwrap());
        assert!(
            matches!(
                parse(&server, &request).await,
                Err(PaymentError::BadPayload(_))
            ),
            "{body}"
        );
    }
    let form = hook(&[], "id=777&payment_status=success&currency=RUB");
    assert_eq!(
        parse(&server, &form).await.unwrap().status,
        ProviderStatus::Paid
    );
    let quoted = hook(&[], r#"{"id":"777","payment_status":"success"}"#);
    assert_eq!(
        parse(&server, &quoted).await.unwrap().status,
        ProviderStatus::Paid
    );
    let mut suffixed = callback(777, "success");
    suffixed.route_suffix = Some("extra".into());
    assert!(matches!(
        parse(&server, &suffixed).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn статусы_mulenpay() {
    for (code, expected, key) in [
        (0, ProviderStatus::Pending, "777:0"),
        (1, ProviderStatus::Pending, "777:1"),
        (2, ProviderStatus::Canceled, "777:canceled"),
        (3, ProviderStatus::Paid, "777:paid"),
        (4, ProviderStatus::Failed, "777:failed"),
        (5, ProviderStatus::Pending, "777:5"),
        (6, ProviderStatus::Pending, "777:6"),
    ] {
        let server = api_payment(777, remote(777, code, "1500.00", "rub")).await;
        let outcome = parse(&server, &callback(777, "success")).await.unwrap();
        assert_eq!(outcome.status, expected, "{code}");
        assert_eq!(outcome.event_key, key, "{code}");
    }
}

#[tokio::test]
async fn сумма_не_сошлась_и_чужая_валюта() {
    let expected = rub(150_000);
    let under = api_payment(777, remote(777, 3, "1499.99", "rub")).await;
    let outcome = parse(&under, &callback(777, "success")).await.unwrap();
    assert!(matches!(
        check_paid_amount(expected, outcome.amount),
        AmountCheck::Underpaid { .. }
    ));
    let dollars = api_payment(777, remote(777, 3, "1500.00", "usd")).await;
    let outcome = parse(&dollars, &callback(777, "success")).await.unwrap();
    assert!(matches!(
        check_paid_amount(expected, outcome.amount),
        AmountCheck::CurrencyMismatch { .. }
    ));
    let broken = api_payment(777, remote(777, 3, "1500.001", "rub")).await;
    assert!(matches!(
        parse(&broken, &callback(777, "success")).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn повтор_уведомления_даёт_то_же_событие() {
    let server = api_payment(777, remote(777, 3, "1500.00", "rub")).await;
    let first = parse(&server, &callback(777, "success")).await.unwrap();
    let second = parse(&server, &callback(777, "success")).await.unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn ключ_не_принят_при_проверке_уведомления() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": "unauthorized", "status": 401})),
        )
        .mount(&server)
        .await;
    let result = parse(&server, &callback(777, "success")).await;
    assert_eq!(
        result,
        Err(PaymentError::Rejected(util::KEYS_REFUSED.into()))
    );
    assert!(!matches!(result, Err(PaymentError::BadSignature)));
}

#[tokio::test]
async fn опрос_статуса_mulenpay() {
    let server = api_payment(777, remote(777, 3, "1500.00", "rub")).await;
    let provider = MulenPay::with_api_base(server.uri());
    let outcome = provider
        .fetch_status(&settings(), "777")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(
        provider.fetch_status(&settings(), "778").await,
        Err(PaymentError::Rejected("платёж не найден в MulenPay".into()))
    );
    assert!(matches!(
        provider.fetch_status(&settings(), "abc").await,
        Err(PaymentError::BadPayload(_))
    ));
}

fn refund_request(amount: i64) -> RefundRequest {
    RefundRequest {
        payment_id: PaymentId(41),
        provider_txid: "777".into(),
        amount: rub(amount),
        reason: None,
        key: Uuid::from_u128(5),
        email: None,
        description: None,
    }
}

#[tokio::test]
async fn возврат_mulenpay_только_на_всю_сумму() {
    let server = api_payment(777, remote(777, 3, "1500.00", "rub")).await;
    Mock::given(method("PUT"))
        .and(path("/v2/payments/777/refund"))
        .and(header("Authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
        .mount(&server)
        .await;
    let provider = MulenPay::with_api_base(server.uri());
    assert_eq!(
        provider.refund(&settings(), &refund_request(50_000)).await,
        Err(PaymentError::Rejected(PARTIAL_REFUND.into()))
    );
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method == wiremock::http::Method::PUT)
    );
    let outcome = provider
        .refund(&settings(), &refund_request(150_000))
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RefundOutcome {
            provider_refund_id: Some("777:refund".into()),
            completed: true
        }
    );
    let unpaid = api_payment(777, remote(777, 1, "1500.00", "rub")).await;
    assert!(matches!(
        MulenPay::with_api_base(unpaid.uri())
            .refund(&settings(), &refund_request(150_000))
            .await,
        Err(PaymentError::Rejected(_))
    ));
    let refused = api_payment(777, remote(777, 3, "1500.00", "rub")).await;
    Mock::given(method("PUT"))
        .and(path("/v2/payments/777/refund"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"success": false, "message": "Недостаточно средств"})),
        )
        .mount(&refused)
        .await;
    assert_eq!(
        MulenPay::with_api_base(refused.uri())
            .refund(&settings(), &refund_request(150_000))
            .await,
        Err(PaymentError::Rejected("Недостаточно средств".into()))
    );
}

#[tokio::test]
async fn проверка_подключения_mulenpay_по_балансам_магазина() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/shops/12345/balances"))
        .and(header("Authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {"balances": [
                {"shop_id": 12345, "currency": "rub", "balance": 1000.5, "hold": 200.0, "available": 800.5}
            ]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/shops/12345/balances"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": "unauthorized", "status": 401})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "success": false,
            "message": "Магазин не найден или не принадлежит текущему пользователю"
        })))
        .with_priority(20)
        .mount(&server)
        .await;
    let provider = MulenPay::with_api_base(server.uri());
    let context = CheckContext {
        webhook_url: Some(HOOK.into()),
    };
    let ok = provider.check_connection(&settings(), &context).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
    assert_eq!(
        ok.summary,
        "API-ключ принят, магазин 12345 найден в MulenPay"
    );
    assert!(
        ok.details[0].starts_with("Доступно к выводу: "),
        "{:?}",
        ok.details
    );
    assert!(ok.details.iter().any(|d| d.contains(HOOK)));
    assert!(ok.details.last().unwrap().contains("Секретный ключ"));
    let mut wrong = settings();
    wrong.set("api_key", json!("mp_api_wrong"));
    assert_eq!(
        provider.check_connection(&wrong, &context).await.status,
        CheckStatus::KeysRejected
    );
    let mut other_shop = settings();
    other_shop.set("shop_id", json!("54321"));
    let shop = provider.check_connection(&other_shop, &context).await;
    assert_eq!(shop.status, CheckStatus::SettingsRejected);
    assert!(shop.summary.contains("54321"));
    let bare = provider
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert!(
        bare.details
            .iter()
            .any(|d| d.contains("Адрес панели не задан"))
    );
    let down = failing_api(502).await;
    assert_eq!(
        MulenPay::with_api_base(down.uri())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::Unreachable
    );
    assert_eq!(
        MulenPay::with_api_base(closed_api())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::Unreachable
    );
}

#[test]
fn настройки_mulenpay() {
    let provider = MulenPay::new();
    assert!(provider.is_configured(&settings()));
    assert!(provider.accepts_http_webhooks());
    assert!(provider.verifies_webhooks_by_api());
    assert!(provider.supports_refunds());
    assert!(!provider.supports_refund_polling());
    assert!(provider.methods(Currency::Rub).is_empty());
    assert_eq!(provider.currencies(), &[Currency::Rub]);
    assert_eq!(provider.client_label(Currency::Rub, &settings()), LABEL);
    assert!(crate::english::text(LABEL.title).is_some());
    assert!(crate::english::text(LABEL.hint).is_some());
    let schema = provider.settings_schema();
    assert_eq!(
        schema
            .iter()
            .filter(|f| f.is_secret())
            .map(|f| f.key)
            .collect::<Vec<_>>(),
        ["api_key", "secret_key"]
    );
    let vat = schema.iter().find(|f| f.key == "vat_code").unwrap();
    assert_eq!(vat.default, Some("0"));
    assert_eq!(vat.labels.map(<[&str]>::len), Some(8));
    for (key, value) in [
        ("shop_id", json!("12a")),
        ("shop_id", json!("0")),
        ("vat_code", json!("9")),
        ("api_key", json!("with space")),
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
