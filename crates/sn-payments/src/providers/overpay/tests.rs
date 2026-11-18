use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use wiremock::matchers::{basic_auth, body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub(crate) const MERCHANT: &str = include_str!("testdata/merchant.pem");
const LEAF: &str = include_str!("testdata/merchant.crt");
const RSA_KEY: &str = include_str!("testdata/merchant-rsa.key");
const CA: &str = include_str!("testdata/ca.pem");
const SERVER_CERT: &str = include_str!("testdata/server.crt");
const SERVER_KEY: &str = include_str!("testdata/server.key");
const ORDER: &str = "4815162342";

fn settings() -> ProviderConfig {
    config(&[
        ("project_id", json!("1017")),
        ("login", json!("shop")),
        ("password", json!("secret")),
        (CERTIFICATE, json!(MERCHANT)),
    ])
}

fn with_pem(pem: &str) -> ProviderConfig {
    let mut settings = settings();
    settings.set(CERTIFICATE, json!(pem));
    settings
}

fn operation(kind: &str, status: &str, amount: &str, minute: u32) -> serde_json::Value {
    json!({
        "type": kind, "status": status, "amount": amount, "currency": "RUB",
        "created": format!("2026-11-18T10:{minute:02}:00.000Z"),
        "iso_message": if status == "success" { "Approved" } else { "Insufficient funds" },
        "iso_response_code": if status == "success" { "000" } else { "051" }
    })
}

fn order_with(
    status: &str,
    charged: &str,
    refunded: &str,
    operations: Vec<serde_json::Value>,
) -> serde_json::Value {
    json!({"orders": [{
        "id": ORDER, "status": status, "amount": "499.00",
        "amount_charged": charged, "amount_refunded": refunded, "currency": "RUB",
        "merchant_order_id": PAYMENT.to_string(), "pan": "411111******1145",
        "created": "2026-11-18T10:00:00.000Z", "updated": "2026-11-18T10:05:00.000Z",
        "operations": operations
    }]})
}

fn order(status: &str, charged: &str, refunded: &str) -> serde_json::Value {
    order_with(status, charged, refunded, Vec::new())
}

async fn api(body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/orders/{ORDER}")))
        .and(basic_auth("shop", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    server
}

fn token() -> String {
    hook_token(&settings()).unwrap()
}

fn notice(body: serde_json::Value) -> WebhookRequest {
    let mut request = hook(&[], serde_json::to_vec(&body).unwrap());
    request.route_suffix = Some(token());
    request
}

fn charged_notice() -> WebhookRequest {
    notice(json!({"id": ORDER, "status": "charged", "merchantTransactionId": PAYMENT.to_string()}))
}

async fn requests(server: &MockServer) -> Vec<wiremock::Request> {
    server.received_requests().await.unwrap()
}

fn refund_request(minor: i64, key: Uuid) -> RefundRequest {
    RefundRequest {
        payment_id: sn_domain::ids::PaymentId(41),
        provider_txid: ORDER.into(),
        amount: rub(minor),
        reason: Some("по просьбе клиента".into()),
        key,
        email: None,
        description: None,
    }
}

#[test]
fn способы_и_подпись_overpay() {
    let overpay = Overpay::new();
    let codes: Vec<&str> = overpay
        .methods(Currency::Rub)
        .iter()
        .map(|m| m.code)
        .collect();
    assert_eq!(codes, [PAGE, CARD, SBP]);
    assert!(overpay.methods(Currency::Usd).is_empty());
    assert_eq!(overpay.method(Currency::Rub, SBP).unwrap().title, "СБП");
    assert_eq!(overpay.method(Currency::Rub, CARD).unwrap().title, "Карта");
    assert_eq!(wire_method(None), Ok(None));
    assert_eq!(wire_method(Some(PAGE)), Ok(None));
    assert_eq!(wire_method(Some(CARD)), Ok(Some("card")));
    assert_eq!(wire_method(Some(SBP)), Ok(Some("fps")));
    assert!(matches!(
        wire_method(Some("externalPaymentPage")),
        Err(PaymentError::Rejected(_))
    ));
    assert_eq!(
        overpay.client_label(Currency::Rub, &settings()).hint,
        "Карты и СБП"
    );
}

#[test]
fn настройки_overpay_проверяются_вместе_с_сертификатом() {
    let overpay = Overpay::new();
    assert_eq!(overpay.validate_settings(&settings()), Ok(()));
    assert!(overpay.is_configured(&settings()));
    let mut no_pem = settings();
    no_pem.set(CERTIFICATE, json!(""));
    assert!(overpay.validate_settings(&no_pem).is_err());
    assert!(!overpay.is_configured(&no_pem));
    let mut plain = settings();
    plain.set("api_url", json!("http://api-pay.overpay.io"));
    assert!(overpay.validate_settings(&plain).is_err());
    let mut other = settings();
    other.set("api_url", json!("https://api2-pay.overpay.io/"));
    assert_eq!(overpay.validate_settings(&other), Ok(()));
    assert_eq!(overpay.api(&other), "https://api2-pay.overpay.io");
    assert_eq!(overpay.api(&settings()), API);
    let mut letters = settings();
    letters.set("project_id", json!("proj-1"));
    assert!(overpay.validate_settings(&letters).is_err());
    let schema = overpay.settings_schema();
    let pem = schema.iter().find(|f| f.key == CERTIFICATE).unwrap();
    assert!(pem.is_secret() && pem.large && pem.required);
    assert!(
        pem.hint
            .contains("openssl pkcs12 -in cert.p12 -out cert.pem -nodes")
    );
    assert!(pem.hint.contains("-legacy"));
    let url = schema.iter().find(|f| f.key == "api_url").unwrap();
    assert_eq!(url.default, Some(API));
    assert!(MERCHANT.chars().count() > 2_000);
}

#[test]
fn сертификат_из_p12_подключается_в_любом_виде() {
    let overpay = Overpay::new();
    let pkcs1 = hook_token(&with_pem(&format!("{LEAF}{CA}{RSA_KEY}"))).unwrap();
    for (name, pem, expected) in [
        ("p12", MERCHANT.to_string(), token()),
        ("leaf_first", format!("{LEAF}{CA}{RSA_KEY}"), pkcs1.clone()),
        ("ca_first", format!("{CA}{RSA_KEY}{LEAF}"), pkcs1),
        (
            "without_breaks",
            MERCHANT.replace(['\n', '\r'], ""),
            token(),
        ),
        ("with_spaces", MERCHANT.replace('\n', " "), token()),
        ("escaped", MERCHANT.replace('\n', "\\n"), token()),
    ] {
        let settings = with_pem(&pem);
        assert_eq!(overpay.validate_settings(&settings), Ok(()), "{name}");
        assert_eq!(hook_token(&settings).unwrap(), expected, "{name}");
    }
}

#[test]
fn негодный_сертификат_объясняется_понятно() {
    let overpay = Overpay::new();
    let expect = |pem: &str, words: &str| match overpay.validate_settings(&with_pem(pem)) {
        Err(PaymentError::NotConfigured(message)) => {
            assert!(message.contains(words), "{message}")
        }
        other => panic!("{other:?}"),
    };
    expect(&format!("{CA}{RSA_KEY}"), "не подходит к закрытому ключу");
    expect(RSA_KEY, "нет сертификата");
    expect(LEAF, "нет закрытого ключа");
    expect(
        &format!(
            "{LEAF}-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIFHDBO\n-----END ENCRYPTED PRIVATE KEY-----\n"
        ),
        "-nodes",
    );
    expect(
        &format!(
            "{LEAF}-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-256-CBC,AB01\n\nMIIE\n-----END RSA PRIVATE KEY-----\n"
        ),
        "-nodes",
    );
    expect("сертификат", "не разобран");
    expect(
        &LEAF.replace("-----END CERTIFICATE-----", ""),
        "не разобран",
    );
    expect(&format!("{LEAF}{RSA_KEY}{RSA_KEY}"), "больше одного");
    expect(
        &format!("{}{RSA_KEY}", LEAF.replace("MII", "MI%")),
        "не разобран",
    );
    expect(
        &format!("{}{RSA_KEY}", LEAF.replace("MII", "MIIZZZZ")),
        "не подходит",
    );
}

#[test]
fn токен_уведомлений_зависит_от_ключа_и_проекта() {
    let first = token();
    assert_eq!(first.len(), 32);
    assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
    let mut other_project = settings();
    other_project.set("project_id", json!("1018"));
    assert_ne!(hook_token(&other_project).unwrap(), first);
    let mut other_password = settings();
    other_password.set("password", json!("changed"));
    assert_eq!(hook_token(&other_password).unwrap(), first);
}

#[tokio::test]
async fn счёт_overpay_создаётся_по_спецификации() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/orders/preflight"))
        .and(basic_auth("shop", "secret"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({
            "amount": "499.00",
            "currency": "RUB",
            "projectId": "1017",
            "merchantTransactionId": PAYMENT.to_string(),
            "livetimeMinutes": 60,
            "description": "Стандарт · 3 месяца",
            "options": {
                "webhookUrl": format!("https://panel.example.com/api/webhooks/pay/test/{}", token()),
                "autoCharge": true
            },
            "returnUrl": "https://panel.example.com/cabinet/payment-wait",
            "client": {"email": "buyer@example.com"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": ORDER, "resultUrl": "https://pay.overpay.io/form/6c1f"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let invoice = Overpay::with_api_base(server.uri())
        .create_invoice(
            &settings(),
            &invoice(rub(49_900), Some(" buyer@example.com ")),
        )
        .await
        .unwrap();
    assert_eq!(invoice.pay_url, "https://pay.overpay.io/form/6c1f");
    assert_eq!(invoice.external_id.as_deref(), Some(ORDER));
    assert_eq!(invoice.payload, json!({"id": ORDER}));
}

#[tokio::test]
async fn способ_оплаты_и_срок_счёта_уходят_в_overpay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/orders/preflight"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 77, "resultUrl": "https://pay.overpay.io/form/77"
        })))
        .mount(&server)
        .await;
    let overpay = Overpay::with_api_base(server.uri());
    for (code, minutes) in [
        (None, 1),
        (Some(PAGE), 60 * 24 * 200),
        (Some(CARD), 30),
        (Some(SBP), 30),
    ] {
        let mut request = invoice(rub(49_900), None);
        request.method = code.map(str::to_string);
        request.expires_in = chrono::Duration::minutes(minutes);
        let created = overpay.create_invoice(&settings(), &request).await.unwrap();
        assert_eq!(created.external_id.as_deref(), Some("77"));
    }
    let bodies: Vec<serde_json::Value> = requests(&server)
        .await
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies.len(), 4);
    assert_eq!(bodies[0]["livetimeMinutes"], 5);
    assert_eq!(bodies[1]["livetimeMinutes"], 144_000);
    assert!(bodies[0].get("paymentMethods").is_none());
    assert!(bodies[1].get("paymentMethods").is_none());
    assert_eq!(bodies[2]["paymentMethods"], json!(["card"]));
    assert_eq!(bodies[3]["paymentMethods"], json!(["fps"]));
    assert!(bodies.iter().all(|b| b.get("client").is_none()));
    let mut unknown = invoice(rub(49_900), None);
    unknown.method = Some("qiwi".into());
    assert_eq!(
        overpay.create_invoice(&settings(), &unknown).await,
        Err(PaymentError::Rejected(
            "этот способ оплаты недоступен".into()
        ))
    );
    assert!(matches!(
        overpay
            .create_invoice(&settings(), &invoice(usd(699), None))
            .await,
        Err(PaymentError::UnsupportedCurrency(_))
    ));
    let mut no_panel = invoice(rub(49_900), None);
    no_panel.webhook_url = String::new();
    assert!(matches!(
        overpay.create_invoice(&settings(), &no_panel).await,
        Err(PaymentError::NotConfigured(_))
    ));
    assert_eq!(requests(&server).await.len(), 4);
}

#[tokio::test]
async fn ошибки_overpay_при_создании_счёта_понятны() {
    for (reply, expected) in [
        (
            ResponseTemplate::new(400).set_body_string(
                "<html><head><title>400 No required SSL certificate was sent</title></head></html>",
            ),
            PaymentError::Rejected(
                "Overpay не принял сертификат клиента, проверьте его в настройках".into(),
            ),
        ),
        (
            ResponseTemplate::new(400).set_body_json(json!({"message": "amount is invalid"})),
            PaymentError::Rejected("amount is invalid".into()),
        ),
        (
            ResponseTemplate::new(422)
                .set_body_json(json!({"error": {"message": "project is disabled"}})),
            PaymentError::Rejected("project is disabled".into()),
        ),
        (
            ResponseTemplate::new(401),
            PaymentError::Rejected(
                "платёжная система не приняла ключи доступа, проверьте настройки".into(),
            ),
        ),
        (
            ResponseTemplate::new(502),
            PaymentError::Http { status: 502 },
        ),
        (
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": ORDER, "resultUrl": "http://pay.overpay.io/form/1"})),
            PaymentError::BadPayload("Overpay не вернул ссылку на оплату".into()),
        ),
        (
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "abc", "resultUrl": "https://pay.overpay.io/form/1"})),
            PaymentError::BadPayload("Overpay не вернул номер заказа".into()),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/orders/preflight"))
            .respond_with(reply)
            .mount(&server)
            .await;
        assert_eq!(
            Overpay::with_api_base(server.uri())
                .create_invoice(&settings(), &invoice(rub(49_900), None))
                .await,
            Err(expected)
        );
    }
}

#[tokio::test]
async fn уведомление_overpay_засчитывается_только_после_запроса_заказа() {
    let server = api(order("charged", "499.00", "0")).await;
    let overpay = Overpay::with_api_base(server.uri());
    let outcome = overpay
        .parse_webhook(&settings(), &charged_notice())
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(49_900)));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some(ORDER));
    assert_eq!(outcome.event_key, format!("order.charged:{ORDER}"));
    assert_eq!(outcome.reply, WebhookReply::ok());
    let pending = api(order("processing", "0", "0")).await;
    let forged = Overpay::with_api_base(pending.uri())
        .parse_webhook(&settings(), &charged_notice())
        .await
        .unwrap();
    assert_eq!(forged.status, ProviderStatus::Pending);
    let unknown = overpay
        .parse_webhook(
            &settings(),
            &notice(json!({"id": "1", "status": "charged"})),
        )
        .await;
    assert_eq!(unknown, Err(PaymentError::BadSignature));
    let traversal = overpay
        .parse_webhook(
            &settings(),
            &notice(json!({"id": "../1", "status": "charged"})),
        )
        .await;
    assert!(matches!(traversal, Err(PaymentError::BadPayload(_))));
    let no_id = overpay
        .parse_webhook(&settings(), &notice(json!({"status": "charged"})))
        .await;
    assert!(matches!(no_id, Err(PaymentError::BadPayload(_))));
}

#[tokio::test]
async fn уведомление_без_токена_отклоняется_без_запроса_в_overpay() {
    let server = api(order("charged", "499.00", "0")).await;
    let overpay = Overpay::with_api_base(server.uri());
    let mut wrong = charged_notice();
    wrong.route_suffix = Some("0".repeat(32));
    let mut missing = charged_notice();
    missing.route_suffix = None;
    for request in [wrong, missing] {
        assert_eq!(
            overpay.parse_webhook(&settings(), &request).await,
            Err(PaymentError::BadSignature)
        );
    }
    assert!(requests(&server).await.is_empty());
    assert!(overpay.verifies_webhooks_by_api());
    assert!(overpay.accepts_http_webhooks());
}

#[tokio::test]
async fn уведомление_overpay_принимается_методом_get_и_формой() {
    let server = api(order("charged", "499.00", "0")).await;
    let overpay = Overpay::with_api_base(server.uri());
    let query = format!("id={ORDER}&status=charged&merchantTransactionId={PAYMENT}");
    let mut as_get = hook(&[], query.clone());
    as_get.query = Some(query.clone());
    as_get.route_suffix = Some(token());
    let mut only_query = hook(&[], "");
    only_query.query = Some(query);
    only_query.route_suffix = Some(token());
    let mut numeric = notice(json!({"id": 4815162342u64, "status": "charged"}));
    numeric.query = None;
    for request in [as_get, only_query, numeric] {
        let outcome = overpay.parse_webhook(&settings(), &request).await.unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.provider_txid.as_deref(), Some(ORDER));
    }
}

#[tokio::test]
async fn статусы_overpay() {
    for (status, expected) in [
        ("charged", ProviderStatus::Paid),
        ("declined", ProviderStatus::Failed),
        ("rejected", ProviderStatus::Failed),
        ("error", ProviderStatus::Failed),
        ("reversed", ProviderStatus::Canceled),
        ("refunded", ProviderStatus::Refunded),
        ("chargeback", ProviderStatus::Refunded),
        ("new", ProviderStatus::Pending),
        ("preflight", ProviderStatus::Pending),
        ("prepared", ProviderStatus::Pending),
        (
            "prepared_for_holder_metadata_collecting",
            ProviderStatus::Pending,
        ),
        ("processing", ProviderStatus::Pending),
        ("authorized", ProviderStatus::Pending),
        ("refund_in_progress", ProviderStatus::Pending),
        ("representment", ProviderStatus::Pending),
        ("credited", ProviderStatus::Pending),
    ] {
        let server = api(order(status, "499.00", "0")).await;
        let outcome = Overpay::with_api_base(server.uri())
            .parse_webhook(&settings(), &charged_notice())
            .await
            .unwrap();
        assert_eq!(outcome.status, expected, "{status}");
        assert_eq!(outcome.provider_txid.as_deref(), Some(ORDER), "{status}");
    }
}

#[tokio::test]
async fn недоплата_и_чужая_валюта_overpay() {
    let expected = rub(49_900);
    let mut dollars = order("charged", "499.00", "0");
    dollars["orders"][0]["currency"] = json!("USD");
    for (body, check) in [
        (order("charged", "498.99", "0"), "under"),
        (dollars, "currency"),
    ] {
        let server = api(body).await;
        let outcome = Overpay::with_api_base(server.uri())
            .parse_webhook(&settings(), &charged_notice())
            .await
            .unwrap();
        let result = check_paid_amount(expected, outcome.amount);
        match check {
            "under" => assert!(matches!(result, AmountCheck::Underpaid { .. })),
            _ => assert!(matches!(result, AmountCheck::CurrencyMismatch { .. })),
        }
    }
    for body in [
        order("charged", "499.001", "0"),
        {
            let mut no_charged = order("charged", "0", "0");
            no_charged["orders"][0]
                .as_object_mut()
                .unwrap()
                .remove("amount_charged");
            no_charged
        },
        {
            let mut euro = order("charged", "499.00", "0");
            euro["orders"][0]["currency"] = json!("EUR");
            euro
        },
        json!({}),
    ] {
        let server = api(body.clone()).await;
        assert!(
            matches!(
                Overpay::with_api_base(server.uri())
                    .parse_webhook(&settings(), &charged_notice())
                    .await,
                Err(PaymentError::BadPayload(_))
            ),
            "{body}"
        );
    }
    let mut foreign = order("charged", "499.00", "0");
    foreign["orders"][0]["merchant_order_id"] = json!(null);
    let server = api(foreign).await;
    let outcome = Overpay::with_api_base(server.uri())
        .parse_webhook(&settings(), &charged_notice())
        .await
        .unwrap();
    assert_eq!(
        outcome.payment,
        Some(PaymentRef::ProviderTxid(ORDER.into()))
    );
    let mut swapped = order("charged", "499.00", "0");
    swapped["orders"][0]["id"] = json!("42");
    let server = api(swapped).await;
    assert_eq!(
        Overpay::with_api_base(server.uri())
            .parse_webhook(&settings(), &charged_notice())
            .await,
        Err(PaymentError::BadPayload(
            "Overpay вернул другой заказ".into()
        ))
    );
}

#[tokio::test]
async fn уведомление_о_возврате_и_чарджбэке_overpay() {
    let first = order_with(
        "charged",
        "499.00",
        "100.00",
        vec![
            operation("charge", "success", "499.00", 1),
            operation("refund", "success", "100.00", 2),
        ],
    );
    let second = order_with(
        "charged",
        "499.00",
        "250.00",
        vec![
            operation("refund", "success", "150.00", 3),
            operation("charge", "success", "499.00", 1),
            operation("refund", "success", "100.00", 2),
        ],
    );
    let full = order_with(
        "refunded",
        "499.00",
        "499.00",
        vec![operation("refund", "success", "399.00", 4)],
    );
    let full_without_sum = order("refunded", "499.00", "0");
    let disputed = order_with(
        "chargeback",
        "499.00",
        "0",
        vec![operation("chargeback", "success", "200.00", 5)],
    );
    let disputed_all = order("chargeback", "499.00", "0");
    for (body, status, key) in [
        (
            first,
            ProviderStatus::PartiallyRefunded {
                refunded: rub(10_000),
            },
            format!("refund:{ORDER}:10000"),
        ),
        (
            second,
            ProviderStatus::PartiallyRefunded {
                refunded: rub(15_000),
            },
            format!("refund:{ORDER}:25000"),
        ),
        (
            full,
            ProviderStatus::Refunded,
            format!("refund:{ORDER}:49900"),
        ),
        (
            full_without_sum,
            ProviderStatus::Refunded,
            format!("refund:{ORDER}:49900"),
        ),
        (
            disputed,
            ProviderStatus::PartiallyRefunded {
                refunded: rub(20_000),
            },
            format!("chargeback:{ORDER}"),
        ),
        (
            disputed_all,
            ProviderStatus::Refunded,
            format!("chargeback:{ORDER}"),
        ),
    ] {
        let server = api(body).await;
        let outcome = Overpay::with_api_base(server.uri())
            .parse_webhook(&settings(), &charged_notice())
            .await
            .unwrap();
        assert_eq!(outcome.status, status, "{key}");
        assert_eq!(outcome.event_key, key);
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    }
}

#[tokio::test]
async fn проверка_статуса_overpay() {
    let server = api(order_with(
        "charged",
        "499.00",
        "100.00",
        vec![operation("refund", "success", "100.00", 2)],
    ))
    .await;
    let overpay = Overpay::with_api_base(server.uri());
    let outcome = overpay
        .fetch_status(&settings(), ORDER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(49_900)));
    assert!(matches!(
        overpay.fetch_status(&settings(), "1").await,
        Err(PaymentError::Rejected(_))
    ));
    assert!(matches!(
        overpay.fetch_status(&settings(), "order-1").await,
        Err(PaymentError::BadPayload(_))
    ));
    let empty = api(json!({"orders": []})).await;
    assert!(matches!(
        Overpay::with_api_base(empty.uri())
            .fetch_status(&settings(), ORDER)
            .await,
        Err(PaymentError::Rejected(_))
    ));
}

async fn refund_api(after: serde_json::Value) -> MockServer {
    let server = api(order_with(
        "charged",
        "499.00",
        "0",
        vec![operation("charge", "success", "499.00", 1)],
    ))
    .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/orders/{ORDER}/refund")))
        .and(basic_auth("shop", "secret"))
        .and(body_json(json!({"amount": "100.00"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(after))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn возврат_через_overpay() {
    let done = refund_api(order_with(
        "charged",
        "499.00",
        "100.00",
        vec![
            operation("charge", "success", "499.00", 1),
            operation("refund", "success", "100.00", 2),
        ],
    ))
    .await;
    assert_eq!(
        Overpay::with_api_base(done.uri())
            .refund(&settings(), &refund_request(10_000, Uuid::new_v4()))
            .await,
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("{ORDER}:10000")),
            completed: true,
        })
    );
    let waiting = refund_api(order("refund_in_progress", "499.00", "0")).await;
    assert_eq!(
        Overpay::with_api_base(waiting.uri())
            .refund(&settings(), &refund_request(10_000, Uuid::new_v4()))
            .await,
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("{ORDER}:10000")),
            completed: false,
        })
    );
    let declined = refund_api(order_with(
        "charged",
        "499.00",
        "0",
        vec![
            operation("charge", "success", "499.00", 1),
            operation("refund", "failure", "100.00", 2),
        ],
    ))
    .await;
    assert_eq!(
        Overpay::with_api_base(declined.uri())
            .refund(&settings(), &refund_request(10_000, Uuid::new_v4()))
            .await,
        Err(PaymentError::Rejected(
            "Overpay отклонил возврат: Insufficient funds".into()
        ))
    );
    assert!(matches!(
        Overpay::with_api_base(done.uri())
            .refund(&settings(), &refund_request(50_000, Uuid::new_v4()))
            .await,
        Err(PaymentError::Rejected(message)) if message.contains("499.00")
    ));
    let mut dollars = refund_request(10_000, Uuid::new_v4());
    dollars.amount = usd(100);
    assert!(matches!(
        Overpay::with_api_base(done.uri())
            .refund(&settings(), &dollars)
            .await,
        Err(PaymentError::UnsupportedCurrency(_))
    ));
    let puts = requests(&done)
        .await
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .count();
    assert_eq!(puts, 1);
}

#[tokio::test]
async fn отказ_overpay_в_возврате_показывается_оператору() {
    let server = api(order("charged", "499.00", "0")).await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/orders/{ORDER}/refund")))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"message": "Refund is not allowed"})),
        )
        .mount(&server)
        .await;
    assert_eq!(
        Overpay::with_api_base(server.uri())
            .refund(&settings(), &refund_request(10_000, Uuid::new_v4()))
            .await,
        Err(PaymentError::Rejected("Refund is not allowed".into()))
    );
}

#[tokio::test]
async fn повтор_возврата_с_тем_же_ключом_не_возвращает_деньги_дважды() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/orders/{ORDER}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(order("charged", "499.00", "0")))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/orders/{ORDER}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_with(
            "charged",
            "499.00",
            "100.00",
            vec![operation("refund", "success", "100.00", 2)],
        )))
        .with_priority(2)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/orders/{ORDER}/refund")))
        .respond_with(ResponseTemplate::new(504))
        .expect(1)
        .mount(&server)
        .await;
    let overpay = Overpay::with_api_base(server.uri());
    let key = Uuid::new_v4();
    assert_eq!(
        overpay
            .refund(&settings(), &refund_request(10_000, key))
            .await,
        Err(PaymentError::Http { status: 504 })
    );
    assert_eq!(
        overpay
            .refund(&settings(), &refund_request(10_000, key))
            .await,
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("{ORDER}:10000")),
            completed: true,
        })
    );
}

#[tokio::test]
async fn повтор_с_тем_же_ключом_после_сбоя_отправляет_возврат_снова() {
    let server = api(order("charged", "499.00", "0")).await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/orders/{ORDER}/refund")))
        .respond_with(ResponseTemplate::new(503))
        .expect(2)
        .mount(&server)
        .await;
    let overpay = Overpay::with_api_base(server.uri());
    let key = Uuid::new_v4();
    for _ in 0..2 {
        assert_eq!(
            overpay
                .refund(&settings(), &refund_request(10_000, key))
                .await,
            Err(PaymentError::Http { status: 503 })
        );
    }
}

#[test]
fn память_попыток_возврата_ограничена() {
    let attempts = Attempts::default();
    let first = Uuid::new_v4();
    attempts.remember(first, ORDER, 100);
    assert_eq!(attempts.target(first, ORDER), Some(100));
    assert_eq!(attempts.target(first, "1"), None);
    for _ in 0..ATTEMPTS_CAPACITY {
        attempts.remember(Uuid::new_v4(), ORDER, 1);
    }
    assert!(locked(&attempts.targets).len() <= ATTEMPTS_CAPACITY);
    assert_eq!(attempts.target(first, ORDER), None);
}

#[tokio::test]
async fn опрос_возврата_overpay() {
    let refund_id = format!("{ORDER}:10000");
    for (body, status) in [
        (
            order_with(
                "charged",
                "499.00",
                "100.00",
                vec![operation("refund", "success", "100.00", 2)],
            ),
            ProviderStatus::PartiallyRefunded {
                refunded: rub(10_000),
            },
        ),
        (
            order("refunded", "499.00", "499.00"),
            ProviderStatus::Refunded,
        ),
        (
            order("refund_in_progress", "499.00", "0"),
            ProviderStatus::Pending,
        ),
        (
            order_with(
                "refund_in_progress",
                "499.00",
                "0",
                vec![operation("refund", "failure", "100.00", 2)],
            ),
            ProviderStatus::Pending,
        ),
        (
            order_with(
                "charged",
                "499.00",
                "0",
                vec![
                    operation("charge", "success", "499.00", 1),
                    operation("refund", "error", "100.00", 2),
                ],
            ),
            ProviderStatus::RefundDeclined,
        ),
        (
            order_with(
                "charged",
                "499.00",
                "0",
                vec![operation("charge", "success", "499.00", 1)],
            ),
            ProviderStatus::Pending,
        ),
    ] {
        let server = api(body).await;
        let outcome = Overpay::with_api_base(server.uri())
            .fetch_refund(&settings(), &refund_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, status);
        assert_eq!(outcome.event_key, format!("refund:{refund_id}"));
        assert_eq!(
            outcome.event_key.rsplit_once("refund:").map(|(_, id)| id),
            Some(refund_id.as_str())
        );
        assert_eq!(outcome.provider_txid.as_deref(), Some(ORDER));
    }
    let overpay = Overpay::new();
    for bad in [
        "refund-1",
        "4815162342",
        "4815162342:abc",
        "x:100",
        "4815162342:0",
    ] {
        assert!(
            matches!(
                overpay.fetch_refund(&settings(), bad).await,
                Err(PaymentError::BadPayload(_))
            ),
            "{bad}"
        );
    }
}

struct TlsServer {
    url: String,
    seen: tokio::task::JoinHandle<Vec<Option<Vec<u8>>>>,
}

async fn tls_server(connections: usize, body: serde_json::Value) -> TlsServer {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(CA.as_bytes()).unwrap())
        .unwrap();
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(SERVER_CERT.as_bytes()).unwrap()],
            PrivateKeyDer::from_pem_slice(SERVER_KEY.as_bytes()).unwrap(),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    let payload = serde_json::to_vec(&body).unwrap();
    let seen = tokio::spawn(async move {
        let mut seen = Vec::new();
        for _ in 0..connections {
            let (tcp, _) = listener.accept().await.unwrap();
            let Ok(mut tls) = acceptor.accept(tcp).await else {
                seen.push(None);
                continue;
            };
            seen.push(
                tls.get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|chain| chain.first())
                    .map(|leaf| leaf.to_vec()),
            );
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match tls.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&chunk[..n]),
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            let _ = tls.write_all(head.as_bytes()).await;
            let _ = tls.write_all(&payload).await;
            let _ = tls.shutdown().await;
        }
        seen
    });
    TlsServer { url, seen }
}

#[tokio::test]
async fn сертификат_клиента_уходит_в_overpay_при_рукопожатии() {
    let server = tls_server(2, order("charged", "499.00", "0")).await;
    let root = reqwest::Certificate::from_pem(CA.as_bytes()).unwrap();
    let outcome = Overpay::trusting(server.url.clone(), root.clone())
        .fetch_status(&settings(), ORDER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    let anonymous = reqwest::Client::builder()
        .add_root_certificate(root)
        .build()
        .unwrap()
        .get(format!("{}/api/orders/{ORDER}", server.url))
        .send()
        .await;
    assert!(anonymous.is_err());
    let seen = server.seen.await.unwrap();
    let leaf = CertificateDer::from_pem_slice(LEAF.as_bytes())
        .unwrap()
        .to_vec();
    assert_eq!(seen[0].as_deref(), Some(leaf.as_slice()));
    assert_eq!(seen[1], None);
}

#[tokio::test]
async fn клиент_с_сертификатом_собирается_один_раз() {
    let overpay = Overpay::new();
    let first = overpay.client(&settings());
    assert!(first.is_ok());
    let digest = locked(&overpay.built).as_ref().unwrap().digest;
    assert!(overpay.client(&settings()).is_ok());
    assert_eq!(locked(&overpay.built).as_ref().unwrap().digest, digest);
    let broken = with_pem("-----BEGIN CERTIFICATE-----\n");
    assert!(overpay.client(&broken).is_err());
    assert_ne!(locked(&overpay.built).as_ref().unwrap().digest, digest);
}
