use super::*;
use crate::test_support::*;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rsa::RsaPrivateKey;
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use serde_json::{Value, json};
use sn_domain::ids::PaymentId;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use wiremock::matchers::{method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

pub(crate) const MERCHANT: &str = include_str!("testdata/merchant.pem");
pub(crate) const PROJECT_PUBLIC: &str = include_str!("testdata/project.pub.pem");
const MERCHANT_PUBLIC: &str = include_str!("testdata/merchant.pub.pem");
const PROJECT: &str = include_str!("testdata/project.pem");
const STRANGER: &str = include_str!("testdata/stranger.pem");
const SHOP: &str = "P7C10F3A29D54E68";
const SECRET: &str = "sid_5f1c0a7be2d94433";
const TX: &str = "APAY26110001";
const RD: &str = "RDAPAY26110001";
const HOOK: &str = "https://panel.example.com/api/webhooks/pay/antilopay";
const SHORT_SECRET: &str = "MIIBCgIBADANBgkqhkiG9w0BAQEFAASB9TCB8gIBAAIxAOVFcNC554zuUsugSVldIeVQ3U/tKikgYi5JNOYNm7n3w0nl3hGgBCum0qy0I+DsjwIDAQABAjEA3BhXfuiYksBPF0+eWn1/hqakbpAcgzKjfOX5MW00hkwRBQKo7sWTc7CCXlPFCtChAhkA9CLl5xHYcLhYTfFdwLbRx7ZAJpVzYMO5AhkA8GmeTd9ccF15g7Mk+fYir9Xz0c23zmaHAhgazmooARGe7t7jUAlXS7ulDiHlGqvciCECGGHbW+hupn3GTpPg84gbO/s+p0Qa7FpDOQIYDpHngs6L9Sw25Gk2CnYKuwtvDMAtvk3e";
const SHORT_PUBLIC: &str = "MEwwDQYJKoZIhvcNAQEBBQADOwAwOAIxAOVFcNC554zuUsugSVldIeVQ3U/tKikgYi5JNOYNm7n3w0nl3hGgBCum0qy0I+DsjwIDAQAB";
const VECTOR: &str = r#"{"type":"payment","payment_id":"APAY26110001","order_id":"6a1c0c8e-3f4b-4d2a-9b7e-51c02d3e4f50","amount":1440.00,"original_amount":1500.00,"fee":60.00,"status":"SUCCESS","currency":"RUB"}"#;
const VECTOR_512: &str =
    "s/K2ktSbdMsXnzIyj/Z6N6XejiMlUwTFNNnxpBZhAG2vnyvArXJc36jupgVXagp9jX756fNNfiiDtpc1S3ONHQ==";
const VECTOR_2048: &str = "nf6se/8BMm4vtF10KO5//DfcBg8xSTQWOaYT7IfolM7VJQABuXiEF5Fom0F8V4QPk6URLPmYlU/y2qSSMWu4asqT/VNoAUCAXElSRZTBAJWROX2BBFomhHrhzPx4mrrxauS/clDPEUu1AOoUW+VtbWxymboZBF+Rxd5bhSg9i8uiTQXfRoH7AUK8CVgG6Ix4Wk+JxcxjgLpw8j5p+6bbjBZyuOuFenUjwFFyDXc+qEACEBaKSG0y7RuIKWpXqFpaJF1kP8sBA7vPbPSJjCLnuXgj5qxDWDg8th7ke1unpoK2HZMgQk+Wk2crh8AUirYwLEPjIInHlHF6OMlAcsiz6Q==";

fn settings() -> ProviderConfig {
    config(&[
        ("project_id", json!(SHOP)),
        ("secret_id", json!(SECRET)),
        ("secret_key", json!(MERCHANT)),
        ("callback_key", json!(PROJECT_PUBLIC)),
    ])
}

fn bare(pem: &str) -> String {
    pem.lines().filter(|l| !l.starts_with("-----")).collect()
}

struct Authentic;

impl Match for Authentic {
    fn matches(&self, request: &Request) -> bool {
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
        };
        header("X-Apay-Secret-Id") == SECRET
            && header("X-Apay-Sign-Version") == "1"
            && header("Content-Type") == "application/json"
            && verify_signature(MERCHANT_PUBLIC, &request.body, header("X-Apay-Sign"))
    }
}

async fn answers(server: &MockServer, route: &str, status: u16, body: Value) {
    Mock::given(method("POST"))
        .and(path(route))
        .and(Authentic)
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

async fn api(route: &str, body: Value) -> MockServer {
    let server = MockServer::start().await;
    answers(&server, route, 200, body).await;
    server
}

fn sent(requests: &[Request], route: &str) -> Vec<Value> {
    requests
        .iter()
        .filter(|r| r.url.path() == route)
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

fn checked(status: &str, original: f64) -> Value {
    json!({
        "code": 0, "payment_id": TX, "order_id": PAYMENT.to_string(),
        "payment_url": format!("https://gate.antilopay.com/payment/{TX}"),
        "ctime": "2026-11-24 16:05:41.220417", "amount": original - 60.0,
        "original_amount": original, "fee": 60.0, "status": status, "currency": "RUB",
        "product_name": "Стандарт", "description": "Стандарт", "pay_method": "SBP",
        "customer": {"email": "client@example.com", "phone": null, "ip": null, "address": null, "fullname": null}
    })
}

fn returned_payment(refunds: Value) -> Value {
    let mut body = checked("SUCCESS", 1500.0);
    body["refunds"] = refunds;
    body
}

fn notice(status: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "type": "payment", "payment_id": TX, "order_id": PAYMENT.to_string(),
        "ctime": "2026-11-24 16:05:41.220417", "amount": 1440.0, "original_amount": 1500.0,
        "fee": 60.0, "status": status, "currency": "RUB", "product_name": "Стандарт",
        "description": "Стандарт", "pay_method": "SBP", "pay_data": "+7 9001234567",
        "customer_ip": "203.0.113.7", "customer": {"email": "client@example.com"}
    }))
    .unwrap()
}

fn refund_notice(status: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "type": "refund", "refund_id": RD, "payment_id": TX, "amount": 500.0,
        "ctime": "2026-11-24 17:12:08.904113", "status": status
    }))
    .unwrap()
}

fn refunded(status: &str) -> Value {
    json!({
        "code": 0, "refund_id": RD, "order_id": Uuid::from_u128(5).to_string(),
        "payment_id": TX, "amount": 500.0, "status": status
    })
}

fn signed(body: Vec<u8>) -> WebhookRequest {
    let sign = signature(PROJECT, &body).unwrap();
    hook(
        &[
            ("Content-Type", "application/json"),
            ("X-Apay-Callback", sign.as_str()),
            ("X-Apay-Callback-Version", "1"),
        ],
        body,
    )
}

fn refund_request() -> RefundRequest {
    RefundRequest {
        payment_id: PaymentId(41),
        provider_txid: TX.into(),
        amount: rub(50_000),
        reason: None,
        key: Uuid::from_u128(5),
        email: None,
        description: None,
    }
}

#[test]
fn подпись_совпадает_с_openssl_для_ключей_512_и_2048_бит() {
    assert_eq!(
        signature(PROJECT, VECTOR.as_bytes()).as_deref(),
        Some(VECTOR_512)
    );
    assert_eq!(
        signature(MERCHANT, VECTOR.as_bytes()).as_deref(),
        Some(VECTOR_2048)
    );
    for (public, vector) in [(PROJECT_PUBLIC, VECTOR_512), (MERCHANT_PUBLIC, VECTOR_2048)] {
        let check = |body: &str, header: &str| verify_signature(public, body.as_bytes(), header);
        assert!(check(VECTOR, vector));
        assert!(check(VECTOR, &format!("  {vector}\r\n")));
        let wrapped: Vec<&str> = vector
            .as_bytes()
            .chunks(60)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        assert!(check(VECTOR, &wrapped.join("\r\n")));
        assert!(check(VECTOR, vector.trim_end_matches('=')));
        assert!(!check(&VECTOR.replace("1500.00", "1500.01"), vector));
        assert!(!check(
            &VECTOR.replace(",\"status\"", ", \"status\""),
            vector
        ));
        assert!(!check(&format!("{VECTOR}\n"), vector));
        for broken in ["", "не base64", "AAAA", "MQ==", &vector[4..]] {
            assert!(!check(VECTOR, broken), "{broken}");
        }
    }
    assert!(!verify_signature(
        PROJECT_PUBLIC,
        VECTOR.as_bytes(),
        VECTOR_2048
    ));
    let foreign = signature(STRANGER, VECTOR.as_bytes()).unwrap();
    assert!(!verify_signature(
        PROJECT_PUBLIC,
        VECTOR.as_bytes(),
        &foreign
    ));
    let doubled = STANDARD.encode([0u8; 64]);
    assert!(!verify_signature(
        PROJECT_PUBLIC,
        VECTOR.as_bytes(),
        &doubled
    ));
    let ones = STANDARD.encode([0xffu8; 64]);
    assert!(!verify_signature(PROJECT_PUBLIC, VECTOR.as_bytes(), &ones));
}

#[test]
fn ключ_принимается_в_любом_виде_в_котором_его_вставят() {
    let one_line = MERCHANT.replace('\n', "");
    let escaped = MERCHANT.trim_end().replace('\n', "\\n");
    let windows = MERCHANT.replace('\n', "\r\n");
    let spaced = format!("  {}  ", MERCHANT.trim_end().replace('\n', " "));
    let pkcs1 = STANDARD.encode(
        RsaPrivateKey::from_pkcs8_der(&STANDARD.decode(bare(MERCHANT)).unwrap())
            .unwrap()
            .to_pkcs1_der()
            .unwrap()
            .as_bytes(),
    );
    let wrapped =
        format!("-----BEGIN RSA PRIVATE KEY-----\n{pkcs1}\n-----END RSA PRIVATE KEY-----");
    let mislabeled = format!(
        "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----",
        bare(MERCHANT)
    );
    assert!(bare(MERCHANT).starts_with("MII") && pkcs1.starts_with("MII"));
    for secret in [
        MERCHANT,
        &one_line,
        &escaped,
        &windows,
        &spaced,
        &pkcs1,
        &wrapped,
        &mislabeled,
        &bare(MERCHANT),
        bare(MERCHANT).trim_end_matches('='),
    ] {
        assert_eq!(
            signature(secret, VECTOR.as_bytes()).as_deref(),
            Some(VECTOR_2048),
            "{}",
            util::clip(secret, 40)
        );
    }
    assert!(bare(PROJECT_PUBLIC).starts_with("MF"));
    for public in [
        bare(PROJECT_PUBLIC),
        PROJECT_PUBLIC.replace('\n', ""),
        PROJECT_PUBLIC.replace('\n', "\\n"),
    ] {
        assert!(verify_signature(&public, VECTOR.as_bytes(), VECTOR_512));
    }
    for broken in [
        "",
        "не ключ",
        "MII",
        "-----BEGIN PRIVATE KEY-----",
        "-----BEGIN ",
    ] {
        assert_eq!(signature(broken, b"{}"), None, "{broken}");
        assert!(!verify_signature(broken, b"{}", VECTOR_512), "{broken}");
    }
    assert_eq!(signature(MERCHANT_PUBLIC, b"{}"), None);
    assert!(!verify_signature(PROJECT, VECTOR.as_bytes(), VECTOR_512));
}

#[test]
fn настройки_antilopay_проверяются_с_понятной_причиной() {
    let provider = Antilopay::new();
    assert!(provider.is_configured(&settings()));
    assert!(!provider.is_configured(&ProviderConfig::default()));
    let fails = |key: &str, value: Value| {
        let mut broken = settings();
        let filled = value != json!("");
        broken.set(key, value);
        assert_eq!(provider.is_configured(&broken), filled, "{key}");
        match provider.validate_settings(&broken) {
            Err(PaymentError::NotConfigured(reason)) => reason,
            other => panic!("{key}: {other:?}"),
        }
    };
    assert!(fails("project_id", json!("мой проект")).contains("ID проекта"));
    assert!(fails("project_id", json!("P".repeat(65))).contains("ID проекта"));
    assert!(fails("secret_id", json!("два слова")).contains("Secret ID"));
    assert!(fails("secret_key", json!(bare(MERCHANT_PUBLIC))).contains("публичный ключ"));
    assert!(fails("secret_key", json!("MIIabc")).contains("не разобран"));
    assert!(fails("callback_key", json!(bare(PROJECT))).contains("секретный ключ"));
    assert!(fails("callback_key", json!("MFabc")).contains("не разобран"));
    assert!(fails("secret_key", json!(SHORT_SECRET)).contains("не разобран"));
    assert!(fails("callback_key", json!(SHORT_PUBLIC)).contains("не разобран"));
    assert!(fails("vat", json!("20")).contains("НДС"));
    for key in ["project_id", "secret_id", "secret_key", "callback_key"] {
        assert!(fails(key, json!("")).contains(key));
    }
    for rate in ["none", "10", "22"] {
        let mut with_vat = settings();
        with_vat.set("vat", json!(rate));
        assert!(provider.validate_settings(&with_vat).is_ok(), "{rate}");
    }
    assert!(provider.is_configured(&settings()));

    let schema = provider.settings_schema();
    let field = |key: &str| schema.iter().find(|f| f.key == key).unwrap();
    for key in ["secret_id", "secret_key", "callback_key"] {
        assert!(field(key).is_secret(), "{key}");
    }
    for key in ["secret_key", "callback_key"] {
        assert!(field(key).large, "{key}");
    }
    assert!(!field("project_id").is_secret());
    let view = serde_json::to_string(&settings().public_view(&schema)).unwrap();
    for secret in [SECRET, &bare(MERCHANT)[..40], &bare(PROJECT_PUBLIC)[..40]] {
        assert!(!view.contains(secret), "{secret}");
    }

    assert_eq!(provider.currencies(), &[Currency::Rub]);
    assert!(provider.methods(Currency::Usd).is_empty());
    let methods = provider.methods(Currency::Rub);
    let codes: Vec<&str> = methods.iter().map(|m| m.code).collect();
    assert_eq!(codes, ["sbp", "card", "sberpay", "card_by", "erip", "page"]);
    let label = provider.client_label(Currency::Rub, &settings());
    for russian in methods
        .iter()
        .flat_map(|m| [m.title, m.hint])
        .chain([label.title, label.hint])
        .filter(|t| t.chars().any(|c| ('а'..='я').contains(&c)))
    {
        assert!(crate::english::text(russian).is_some(), "{russian}");
    }
    assert_eq!(
        provider.api_url(&settings()).as_deref(),
        Some("https://lk.antilopay.com/api/v1")
    );
    assert!(provider.accepts_http_webhooks());
    assert!(provider.polls_by_payment_id() && provider.status_by_order());
    assert!(provider.supports_refunds() && provider.supports_refund_polling());
    assert!(!provider.supports_recurring());
}

#[tokio::test]
async fn счёт_antilopay_подписан_и_несёт_нужные_поля() {
    let url = format!("https://gate.antilopay.com/payment/{TX}");
    let server = api(
        "/payment/create",
        json!({"code": 0, "payment_id": TX, "payment_url": url}),
    )
    .await;
    let provider = Antilopay::with_api_base(format!("{}/", server.uri()));
    let created = provider
        .create_invoice(
            &settings(),
            &invoice(rub(150_005), Some("client@example.com")),
        )
        .await
        .unwrap();
    assert_eq!(created.pay_url, url);
    assert_eq!(created.external_id.as_deref(), Some(TX));
    assert_eq!(created.expires_at, None);
    assert_eq!(created.payload, json!({"payment_id": TX, "method": null}));
    let requests = server.received_requests().await.unwrap();
    let raw = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(raw.contains("\"amount\":1500.05,"), "{raw}");
    assert!(!raw.contains(": ") && !raw.contains('\n'), "{raw}");
    assert_eq!(
        sent(&requests, "/payment/create"),
        vec![json!({
            "project_identificator": SHOP,
            "amount": 1500.05,
            "order_id": PAYMENT.to_string(),
            "currency": "RUB",
            "product_name": "Стандарт · 3 месяца",
            "product_type": "services",
            "description": "Стандарт · 3 месяца",
            "customer": {"email": "client@example.com"},
            "success_url": "https://panel.example.com/cabinet/payment-wait",
            "fail_url": "https://panel.example.com/cabinet/payment-wait",
            "params": {"skip_success_page": true}
        })]
    );
}

#[tokio::test]
async fn способ_ндс_почта_и_адрес_возврата_в_счёте() {
    let server = api(
        "/payment/create",
        json!({"code": 0, "payment_id": TX, "payment_url": "https://gate.antilopay.com/payment/x"}),
    )
    .await;
    let provider = Antilopay::with_api_base(server.uri());
    for (code, wire) in [
        ("sbp", "SBP"),
        ("card", "CARD_RU"),
        ("sberpay", "SBER_PAY"),
        ("card_by", "CARD_BY"),
        ("erip", "ERIP"),
    ] {
        let mut request = invoice(rub(50_000), None);
        request.method = Some(code.into());
        let created = provider
            .create_invoice(&settings(), &request)
            .await
            .unwrap();
        assert_eq!(created.payload["method"], wire);
    }
    let mut page = invoice(rub(50_000), None);
    page.method = Some("page".into());
    page.return_url = Some("http://panel.example.com/cabinet".into());
    let mut taxed = settings();
    taxed.set("vat", json!("22"));
    provider.create_invoice(&taxed, &page).await.unwrap();
    let mut long = invoice(rub(50_000), Some(" client @example.com"));
    long.return_url = Some(format!("https://panel.example.com/{}", "a".repeat(500)));
    long.description = "д".repeat(300);
    long.webhook_url = "https://203.0.113.5/api/webhooks/pay/antilopay".into();
    provider.create_invoice(&settings(), &long).await.unwrap();

    let bodies = sent(
        &server.received_requests().await.unwrap(),
        "/payment/create",
    );
    let methods: Vec<&Value> = bodies[..5].iter().map(|b| &b["prefer_methods"]).collect();
    assert_eq!(
        methods,
        [
            &json!(["SBP"]),
            &json!(["CARD_RU"]),
            &json!(["SBER_PAY"]),
            &json!(["CARD_BY"]),
            &json!(["ERIP"])
        ]
    );
    assert_eq!(
        bodies[0]["customer"],
        json!({"email": "client7@panel.example.com"})
    );
    assert_eq!(bodies[0].get("vat"), None);
    assert_eq!(bodies[5].get("prefer_methods"), None);
    assert_eq!(bodies[5]["vat"], 22);
    for key in ["success_url", "fail_url", "params"] {
        assert_eq!(bodies[5].get(key), None, "{key}");
        assert_eq!(bodies[6].get(key), None, "{key}");
    }
    assert_eq!(
        bodies[6]["customer"],
        json!({"email": "client7@telegram.org"})
    );
    let length = |key: &str| bodies[6][key].as_str().unwrap().chars().count();
    assert_eq!((length("product_name"), length("description")), (128, 255));

    let mut unknown = invoice(rub(50_000), None);
    unknown.method = Some("paypal".into());
    assert!(matches!(
        provider.create_invoice(&settings(), &unknown).await,
        Err(PaymentError::Rejected(_))
    ));
    assert_eq!(
        provider
            .create_invoice(&settings(), &invoice(usd(699), None))
            .await,
        Err(PaymentError::UnsupportedCurrency("USD".into()))
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 7);
}

#[tokio::test]
async fn отказы_antilopay_при_создании_счёта() {
    let reply = |status: u16, body: Value| async move {
        let server = MockServer::start().await;
        answers(&server, "/payment/create", status, body).await;
        Antilopay::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(rub(50_000), None))
            .await
    };
    let refused = |code: i64| json!({"code": code, "error": "bad field"});
    for (code, part) in [
        (4, "проект"),
        (5, "уже есть"),
        (6, "не подтверждён"),
        (7, "нет доступных способов"),
        (9, "не разрешает"),
        (11, "почту"),
        (13, "сумму"),
        (14, "адрес возврата"),
        (17, "НДС"),
        (20, "меньше минимальной"),
        (21, "нет доступных способов"),
        (22, "IP-адреса сервера"),
        (25, "не подключён"),
        (26, "лимит"),
        (31, "больше максимальной"),
        (32, "IP-адрес покупателя"),
        (15, "bad field"),
        (777, "bad field"),
    ] {
        for status in [200, 400] {
            match reply(status, refused(code)).await {
                Err(PaymentError::Rejected(reason)) => {
                    assert!(reason.contains(part), "{code}: {reason}")
                }
                other => panic!("{code}: {other:?}"),
            }
        }
    }
    for code in 1..=3 {
        for status in [200, 401, 403] {
            let err = reply(status, refused(code)).await.unwrap_err();
            assert!(util::keys_refused(&err), "{code}: {err:?}");
        }
    }
    for code in [500, 1000] {
        for status in [200, 500] {
            let err = reply(status, refused(code)).await.unwrap_err();
            assert!(err.is_retryable(), "{code}: {err:?}");
        }
    }
    let silent = reply(200, json!({"code": 12345})).await;
    assert!(
        matches!(&silent, Err(PaymentError::Rejected(r)) if r.contains("12345")),
        "{silent:?}"
    );
    for (status, body) in [
        (200, json!({"payment_id": TX})),
        (200, json!([])),
        (200, json!({"code": "нет"})),
    ] {
        let err = reply(status, body).await.unwrap_err();
        assert!(matches!(err, PaymentError::Transport(_)), "{err:?}");
    }
    for body in [
        json!({"code": 0, "payment_url": "https://gate.antilopay.com/x"}),
        json!({"code": 0, "payment_id": "../x", "payment_url": "https://gate.antilopay.com/x"}),
        json!({"code": 0, "payment_id": TX}),
        json!({"code": 0, "payment_id": TX, "payment_url": "http://gate.antilopay.com/x"}),
    ] {
        let err = reply(200, body.clone()).await.unwrap_err();
        assert!(matches!(err, PaymentError::BadPayload(_)), "{body}");
    }
    assert_eq!(
        reply(502, json!("Bad Gateway")).await,
        Err(PaymentError::Http { status: 502 })
    );
    assert_eq!(
        reply(
            503,
            json!({"code": 0, "payment_id": TX, "payment_url": "https://gate.antilopay.com/x"})
        )
        .await,
        Err(PaymentError::Http { status: 503 })
    );
    let closed = Antilopay::with_api_base(closed_api())
        .create_invoice(&settings(), &invoice(rub(50_000), None))
        .await;
    assert!(matches!(closed, Err(PaymentError::Transport(_))));
}

#[tokio::test]
async fn оплата_засчитывается_по_ответу_api_и_повтор_даёт_то_же_событие() {
    let server = api("/payment/check", checked("SUCCESS", 1500.0)).await;
    let provider = Antilopay::with_api_base(server.uri());
    let mut inflated: Value = serde_json::from_slice(&notice("SUCCESS")).unwrap();
    inflated["original_amount"] = json!(99_999.0);
    let body = serde_json::to_vec(&inflated).unwrap();
    let outcome = provider
        .parse_webhook(&settings(), &signed(body.clone()))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some(TX));
    assert_eq!(outcome.event_key, format!("{TX}:paid"));
    assert_eq!(outcome.reply, WebhookReply::ok());
    assert_eq!(outcome.signal, None);
    let again = provider
        .parse_webhook(&settings(), &signed(body))
        .await
        .unwrap();
    assert_eq!(again, outcome);
    assert_eq!(
        sent(&server.received_requests().await.unwrap(), "/payment/check"),
        vec![json!({"project_identificator": SHOP, "order_id": PAYMENT.to_string()}); 2]
    );

    let mut lower = checked("success", 1500.0);
    lower["currency"] = json!("rub");
    let server = api("/payment/check", lower).await;
    let mut untyped: Value = serde_json::from_slice(&notice("SUCCESS")).unwrap();
    untyped.as_object_mut().unwrap().remove("type");
    let outcome = Antilopay::with_api_base(server.uri())
        .parse_webhook(&settings(), &signed(serde_json::to_vec(&untyped).unwrap()))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));

    let short = api("/payment/check", checked("SUCCESS", 15.0)).await;
    let outcome = Antilopay::with_api_base(short.uri())
        .parse_webhook(&settings(), &signed(notice("SUCCESS")))
        .await
        .unwrap();
    assert!(matches!(
        check_paid_amount(rub(150_000), outcome.amount),
        AmountCheck::Underpaid { .. }
    ));
    let mut foreign = checked("SUCCESS", 1500.0);
    foreign["currency"] = json!("USD");
    let foreign = api("/payment/check", foreign).await;
    let outcome = Antilopay::with_api_base(foreign.uri())
        .parse_webhook(&settings(), &signed(notice("SUCCESS")))
        .await
        .unwrap();
    assert!(!check_paid_amount(rub(150_000), outcome.amount).accepts());
}

#[tokio::test]
async fn поддельное_уведомление_отклоняется_без_запроса_к_antilopay() {
    let server = api("/payment/check", checked("SUCCESS", 1500.0)).await;
    let provider = Antilopay::with_api_base(server.uri());
    let body = notice("SUCCESS");
    let forged = signature(STRANGER, &body).unwrap();
    let merchant = signature(MERCHANT, &body).unwrap();
    for headers in [
        vec![],
        vec![("X-Apay-Callback", "")],
        vec![("X-Apay-Callback", "не base64")],
        vec![("X-Apay-Callback", forged.as_str())],
        vec![("X-Apay-Callback", merchant.as_str())],
        vec![("X-Apay-Sign", forged.as_str())],
    ] {
        assert_eq!(
            provider
                .parse_webhook(&settings(), &hook(&headers, body.clone()))
                .await,
            Err(PaymentError::BadSignature),
            "{headers:?}"
        );
    }
    let mut tampered = signed(body.clone());
    tampered.body = String::from_utf8(body.clone())
        .unwrap()
        .replace("1500.0", "15000.0")
        .into();
    assert_eq!(
        provider.parse_webhook(&settings(), &tampered).await,
        Err(PaymentError::BadSignature)
    );
    let mut version = signed(body.clone());
    version
        .headers
        .retain(|(k, _)| k != "X-Apay-Callback-Version");
    version
        .headers
        .push(("x-apay-callback-version".into(), "2".into()));
    assert!(matches!(
        provider.parse_webhook(&settings(), &version).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut suffix = signed(body.clone());
    suffix.route_suffix = Some("pay".into());
    assert!(matches!(
        provider.parse_webhook(&settings(), &suffix).await,
        Err(PaymentError::BadPayload(_))
    ));
    assert!(matches!(
        provider
            .parse_webhook(&settings(), &signed(b"not json".to_vec()))
            .await,
        Err(PaymentError::BadPayload(_))
    ));
    assert!(matches!(
        provider
            .parse_webhook(&ProviderConfig::default(), &signed(body))
            .await,
        Err(PaymentError::NotConfigured(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn уведомление_без_подтверждения_в_api_не_засчитывается() {
    let parse = |answer: Value| async move {
        let server = api("/payment/check", answer).await;
        Antilopay::with_api_base(server.uri())
            .parse_webhook(&settings(), &signed(notice("SUCCESS")))
            .await
    };
    for status in ["PENDING", "WAIT_CONFIRM"] {
        let waiting = parse(checked(status, 1500.0)).await;
        assert!(
            matches!(&waiting, Err(PaymentError::Rejected(r)) if r.contains(status)),
            "{waiting:?}"
        );
    }
    let failed = parse(checked("FAIL", 1500.0)).await.unwrap();
    assert_eq!(failed.status, ProviderStatus::Failed);
    assert!(matches!(
        parse(json!({"code": 8, "error": "unknown payment"})).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut other = checked("SUCCESS", 1500.0);
    other["payment_id"] = json!("APAY26110002");
    assert!(matches!(
        parse(other).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut order = checked("SUCCESS", 1500.0);
    order["order_id"] = json!(Uuid::from_u128(9).to_string());
    assert!(matches!(
        parse(order).await,
        Err(PaymentError::BadPayload(_))
    ));
    for missing in ["original_amount", "currency", "status"] {
        let mut partial = checked("SUCCESS", 1500.0);
        partial.as_object_mut().unwrap().remove(missing);
        assert!(
            matches!(parse(partial).await, Err(PaymentError::BadPayload(_))),
            "{missing}"
        );
    }
    let refused = parse(json!({"code": 22, "error": "address"})).await;
    assert!(
        matches!(&refused, Err(PaymentError::Rejected(r)) if r == IP_REFUSED),
        "{refused:?}"
    );
    let down = failing_api(502).await;
    assert_eq!(
        Antilopay::with_api_base(down.uri())
            .parse_webhook(&settings(), &signed(notice("SUCCESS")))
            .await,
        Err(PaymentError::Http { status: 502 })
    );
    let closed = Antilopay::with_api_base(closed_api())
        .parse_webhook(&settings(), &signed(notice("SUCCESS")))
        .await;
    assert!(matches!(closed, Err(PaymentError::Transport(_))));

    let server = api("/payment/check", checked("PENDING", 1500.0)).await;
    let provider = Antilopay::with_api_base(server.uri());
    let pending = provider
        .parse_webhook(&settings(), &signed(notice("FAIL")))
        .await
        .unwrap();
    assert_eq!(pending.status, ProviderStatus::Pending);
    for broken in [
        json!({"type": "payment", "order_id": PAYMENT.to_string(), "status": "SUCCESS"}),
        json!({"type": "payment", "payment_id": TX, "status": "SUCCESS"}),
        json!({"type": "payment", "payment_id": TX, "order_id": "o".repeat(101), "status": "SUCCESS"}),
        json!({"type": "payment", "payment_id": "../x", "order_id": PAYMENT.to_string()}),
    ] {
        let request = signed(serde_json::to_vec(&broken).unwrap());
        assert!(
            matches!(
                provider.parse_webhook(&settings(), &request).await,
                Err(PaymentError::BadPayload(_))
            ),
            "{broken}"
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn выплаты_и_пополнения_steam_пропускаются_без_запроса() {
    let server = MockServer::start().await;
    let provider = Antilopay::with_api_base(server.uri());
    for kind in ["withdraw", "topup", "что-то новое"] {
        let body = json!({
            "type": kind, "withdraw_id": "WDAPAY1", "order_id": PAYMENT.to_string(),
            "status": "COMPLETE", "amount": 1000.0
        });
        let outcome = provider
            .parse_webhook(&settings(), &signed(serde_json::to_vec(&body).unwrap()))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        assert_eq!(outcome.payment, None);
        assert_eq!(outcome.reply, WebhookReply::ok());
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn статусы_платежа_antilopay() {
    let ask = |answer: Value| async move {
        let server = api("/payment/check", answer).await;
        Antilopay::with_api_base(server.uri())
            .fetch_status(&settings(), &PAYMENT.to_string())
            .await
    };
    for (status, expected, key) in [
        ("PENDING", ProviderStatus::Pending, "pending"),
        ("SUCCESS", ProviderStatus::Paid, "paid"),
        ("FAIL", ProviderStatus::Failed, "fail"),
        ("CANCEL", ProviderStatus::Canceled, "cancel"),
        ("EXPIRED", ProviderStatus::Canceled, "expired"),
        ("SOMETHING_NEW", ProviderStatus::Pending, "something_new"),
    ] {
        let outcome = ask(checked(status, 1500.0)).await.unwrap().unwrap();
        assert_eq!(outcome.status, expected, "{status}");
        assert_eq!(outcome.event_key, format!("{TX}:{key}"));
        assert_eq!(outcome.provider_txid.as_deref(), Some(TX));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.signal, None);
    }
    let reversed = ask(checked("REVERSED", 1500.0)).await.unwrap().unwrap();
    assert_eq!(reversed.status, ProviderStatus::Refunded);
    assert_eq!(reversed.amount, Some(rub(150_000)));
    assert_eq!(reversed.event_key, format!("reversed:{TX}"));
    let disputed = ask(checked("CHARGEBACK", 1500.0)).await.unwrap().unwrap();
    assert_eq!(disputed.status, ProviderStatus::Refunded);
    assert_eq!(disputed.amount, None);
    assert_eq!(disputed.event_key, format!("chargeback:{TX}"));
    let told = disputed.notice().unwrap();
    assert_eq!(told.kind, NoticeKind::Chargeback);
    assert!(told.text.contains(TX));

    let one = |status: &str, amount: f64| json!({"refund_id": RD, "order_id": null, "amount": amount, "status": status});
    let untouched = ask(returned_payment(json!([
        one("PENDING", 500.0),
        one("WAIT_ANSWER", 500.0),
        one("FAIL", 1500.0)
    ])))
    .await
    .unwrap()
    .unwrap();
    assert_eq!(untouched.status, ProviderStatus::Paid);
    let part = ask(returned_payment(json!([one("COMPLETE", 500.0)])))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(part.status, ProviderStatus::Pending);
    let told = part.notice().unwrap();
    assert_eq!(told.kind, NoticeKind::RefundReported);
    assert!(told.text.contains(TX), "{}", told.text);
    assert_ne!(part.event_key, format!("{TX}:paid"));
    let full = ask(returned_payment(json!([one("COMPLETE", 1500.0)])))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(full.status, ProviderStatus::Refunded);
    assert_eq!(full.event_key, format!("refund:{RD}"));
    assert_eq!(full.amount, Some(rub(150_000)));
    let two = ask(returned_payment(json!([
        one("COMPLETE", 1000.0),
        one("COMPLETE", 500.0)
    ])))
    .await
    .unwrap()
    .unwrap();
    assert_eq!(two.status, ProviderStatus::Refunded);
    assert_eq!(two.event_key, format!("refund:{TX}"));

    assert_eq!(
        ask(json!({"code": 8, "error": "unknown payment"})).await,
        Err(PaymentError::Rejected(
            "платёж не найден в Antilopay".into()
        ))
    );
    let mut foreign = checked("SUCCESS", 1500.0);
    foreign["order_id"] = json!("MERCHANT-1");
    assert!(matches!(
        ask(foreign).await,
        Err(PaymentError::BadPayload(_))
    ));
    let silent = Antilopay::with_api_base(closed_api());
    assert_eq!(silent.fetch_status(&settings(), TX).await, Ok(None));
}

#[tokio::test]
async fn платёж_другого_сайта_того_же_проекта_ищется_по_номеру_antilopay() {
    let mut answer = checked("SUCCESS", 1500.0);
    answer["order_id"] = json!("bot-1204");
    let server = api("/payment/check", answer).await;
    let mut body: Value = serde_json::from_slice(&notice("SUCCESS")).unwrap();
    body["order_id"] = json!("bot-1204");
    let outcome = Antilopay::with_api_base(server.uri())
        .parse_webhook(&settings(), &signed(serde_json::to_vec(&body).unwrap()))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.payment, Some(PaymentRef::ProviderTxid(TX.into())));
}

#[tokio::test]
async fn возврат_antilopay_создаётся_один_раз() {
    let server = api("/refund/create", json!({"code": 0, "refund_id": RD})).await;
    let provider = Antilopay::with_api_base(server.uri());
    let outcome = provider
        .refund(&settings(), &refund_request())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RefundOutcome {
            provider_refund_id: Some(RD.into()),
            completed: false
        }
    );
    let requests = server.received_requests().await.unwrap();
    let raw = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(raw.contains("\"amount\":500.00"), "{raw}");
    assert_eq!(
        sent(&requests, "/refund/create"),
        vec![json!({
            "project_identificator": SHOP, "transaction_id": TX,
            "order_id": Uuid::from_u128(5).to_string(), "amount": 500.0
        })]
    );

    for (status, created) in [
        (200, json!({"code": 18, "error": "duplicate refund"})),
        (500, json!({"code": 500, "error": "try later"})),
        (502, json!("Bad Gateway")),
    ] {
        let server = MockServer::start().await;
        answers(&server, "/refund/create", status, created.clone()).await;
        answers(&server, "/refund/check", 200, refunded("WAIT_ANSWER")).await;
        let found = Antilopay::with_api_base(server.uri())
            .refund(&settings(), &refund_request())
            .await
            .unwrap();
        assert_eq!(found, outcome, "{created}");
        assert_eq!(
            sent(&server.received_requests().await.unwrap(), "/refund/check"),
            vec![
                json!({"project_identificator": SHOP, "order_id": Uuid::from_u128(5).to_string()})
            ]
        );
    }

    let lost = MockServer::start().await;
    answers(&lost, "/refund/create", 502, json!("Bad Gateway")).await;
    answers(
        &lost,
        "/refund/check",
        200,
        json!({"code": 10, "error": "unknown refund"}),
    )
    .await;
    assert_eq!(
        Antilopay::with_api_base(lost.uri())
            .refund(&settings(), &refund_request())
            .await,
        Err(PaymentError::Http { status: 502 })
    );
    let closed = Antilopay::with_api_base(closed_api())
        .refund(&settings(), &refund_request())
        .await;
    assert!(matches!(closed, Err(PaymentError::Transport(_))));

    for (key, value) in [
        ("amount", json!(400.0)),
        ("payment_id", json!("APAY26110009")),
        ("order_id", json!(Uuid::from_u128(6).to_string())),
    ] {
        let server = MockServer::start().await;
        answers(
            &server,
            "/refund/create",
            200,
            json!({"code": 18, "error": "duplicate refund"}),
        )
        .await;
        let mut foreign = refunded("COMPLETE");
        foreign[key] = value;
        answers(&server, "/refund/check", 200, foreign).await;
        let refused = Antilopay::with_api_base(server.uri())
            .refund(&settings(), &refund_request())
            .await;
        assert!(refused.is_err(), "{key}: {refused:?}");
    }
}

#[tokio::test]
async fn отказы_в_возврате_antilopay() {
    let reply = |code: i64| async move {
        let server = api("/refund/create", json!({"code": code, "error": "Refused"})).await;
        Antilopay::with_api_base(server.uri())
            .refund(&settings(), &refund_request())
            .await
    };
    for (code, part) in [
        (8, "не найден"),
        (9, "не делает возврат"),
        (11, "не разрешает возвраты"),
        (12, "сумму"),
        (15, "IP-адреса сервера"),
        (16, "не делает возврат"),
        (17, "не хватает денег"),
        (20, "только всю сумму"),
        (21, "Refused"),
    ] {
        match reply(code).await {
            Err(PaymentError::Rejected(reason)) => {
                assert!(reason.contains(part), "{code}: {reason}")
            }
            other => panic!("{code}: {other:?}"),
        }
    }
    assert!(util::keys_refused(&reply(3).await.unwrap_err()));
    let server = MockServer::start().await;
    let provider = Antilopay::with_api_base(server.uri());
    let mut broken = refund_request();
    broken.provider_txid = "../refund".into();
    assert!(matches!(
        provider.refund(&settings(), &broken).await,
        Err(PaymentError::Rejected(_))
    ));
    let mut dollars = refund_request();
    dollars.amount = usd(500);
    assert_eq!(
        provider.refund(&settings(), &dollars).await,
        Err(PaymentError::UnsupportedCurrency("USD".into()))
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn итог_возврата_antilopay_по_опросу_и_уведомлению() {
    for (status, expected, amount) in [
        (
            "COMPLETE",
            ProviderStatus::PartiallyRefunded {
                refunded: rub(50_000),
            },
            Some(rub(50_000)),
        ),
        ("FAIL", ProviderStatus::RefundDeclined, None),
        ("PENDING", ProviderStatus::Pending, None),
        ("WAIT_ANSWER", ProviderStatus::Pending, None),
    ] {
        let server = api("/refund/check", refunded(status)).await;
        let provider = Antilopay::with_api_base(server.uri());
        let polled = provider
            .fetch_refund(&settings(), RD)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(polled.status, expected, "{status}");
        assert_eq!(polled.amount, amount, "{status}");
        assert_eq!(polled.event_key, format!("refund:{RD}"));
        assert_eq!(polled.payment, Some(PaymentRef::ProviderTxid(TX.into())));
        let told = provider
            .parse_webhook(&settings(), &signed(refund_notice("FAIL")))
            .await
            .unwrap();
        assert_eq!(told, polled, "{status}");
        assert_eq!(
            sent(&server.received_requests().await.unwrap(), "/refund/check"),
            vec![json!({"project_identificator": SHOP, "refund_id": RD}); 2]
        );
        let claimed = provider
            .parse_webhook(&settings(), &signed(refund_notice("COMPLETE")))
            .await;
        if expected == ProviderStatus::Pending {
            assert!(
                matches!(&claimed, Err(PaymentError::Rejected(r)) if r.contains(status)),
                "{claimed:?}"
            );
        } else {
            assert_eq!(claimed, Ok(polled));
        }
    }

    let mut short = refunded("COMPLETE");
    short.as_object_mut().unwrap().remove("payment_id");
    let server = api("/refund/check", short).await;
    let provider = Antilopay::with_api_base(server.uri());
    let told = provider
        .parse_webhook(&settings(), &signed(refund_notice("COMPLETE")))
        .await
        .unwrap();
    assert_eq!(told.payment, Some(PaymentRef::ProviderTxid(TX.into())));
    let polled = provider
        .fetch_refund(&settings(), RD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(polled.payment, None);

    let mut moved = refunded("COMPLETE");
    moved["payment_id"] = json!("APAY26110009");
    let server = api("/refund/check", moved).await;
    assert!(matches!(
        Antilopay::with_api_base(server.uri())
            .parse_webhook(&settings(), &signed(refund_notice("COMPLETE")))
            .await,
        Err(PaymentError::BadPayload(_))
    ));
    let unknown = api(
        "/refund/check",
        json!({"code": 10, "error": "unknown refund"}),
    )
    .await;
    let provider = Antilopay::with_api_base(unknown.uri());
    assert!(matches!(
        provider
            .parse_webhook(&settings(), &signed(refund_notice("COMPLETE")))
            .await,
        Err(PaymentError::BadPayload(_))
    ));
    assert_eq!(
        provider.fetch_refund(&settings(), RD).await,
        Err(PaymentError::Rejected(
            "возврат не найден в Antilopay".into()
        ))
    );
    assert_eq!(
        provider.fetch_refund(&settings(), "outside:key").await,
        Ok(None)
    );
}

#[tokio::test]
async fn проверка_подключения_antilopay() {
    let context = CheckContext {
        webhook_url: Some(HOOK.into()),
    };
    let run = |check: Value, balance: Value| {
        let context = context.clone();
        async move {
            let server = MockServer::start().await;
            answers(&server, "/payment/check", 200, check).await;
            answers(&server, "/project/balance", 200, balance).await;
            let result = Antilopay::with_api_base(server.uri())
                .check_connection(&settings(), &context)
                .await;
            (result, server.received_requests().await.unwrap())
        }
    };
    let missing = || json!({"code": 8, "error": "unknown payment"});
    let balance = || {
        json!({
            "code": 0, "project_identificator": SHOP,
            "rub": {"available": 1234, "blocked": 0, "withdraw": 10}
        })
    };
    let (ok, requests) = run(missing(), balance()).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{ok:?}");
    let details = ok.details.join("\n");
    assert!(
        details.contains(&util::amount_text(rub(123_400))),
        "{details}"
    );
    assert!(details.contains(HOOK), "{details}");
    assert!(details.contains("87.228.9.243"), "{details}");
    assert!(!requests.iter().any(|r| r.url.path().contains("create")));
    assert_eq!(
        sent(&requests, "/payment/check"),
        vec![json!({
            "project_identificator": SHOP,
            "order_id": "00000000-0000-0000-0000-000000000000"
        })]
    );
    let text = serde_json::to_string(&ok).unwrap();
    for secret in [SECRET, &bare(MERCHANT)[..40], &bare(PROJECT_PUBLIC)[..40]] {
        assert!(!text.contains(secret), "{secret}");
    }

    let (no_balance, _) = run(missing(), json!({"code": 9, "error": "no access"})).await;
    assert_eq!(no_balance.status, CheckStatus::Ok);
    let (unconfirmed, _) = run(missing(), json!({"code": 6, "error": "moderation"})).await;
    assert_eq!(unconfirmed.status, CheckStatus::SettingsRejected);
    assert!(unconfirmed.summary.contains("не подтверждён"));
    for (code, status, part) in [
        (1, CheckStatus::KeysRejected, "Secret ID"),
        (2, CheckStatus::KeysRejected, "Secret ID"),
        (3, CheckStatus::KeysRejected, "подпись"),
        (4, CheckStatus::SettingsRejected, "проект"),
        (22, CheckStatus::SettingsRejected, "IP-адреса"),
        (15, CheckStatus::KeysRejected, "bad field"),
    ] {
        let (failed, requests) = run(json!({"code": code, "error": "bad field"}), balance()).await;
        assert_eq!(failed.status, status, "{code}");
        assert!(failed.summary.contains(part), "{code}: {}", failed.summary);
        assert!(sent(&requests, "/project/balance").is_empty());
    }
    let (server_error, _) = run(json!({"code": 500, "error": "try later"}), balance()).await;
    assert_eq!(server_error.status, CheckStatus::Unreachable);
    let closed = Antilopay::with_api_base(closed_api())
        .check_connection(&settings(), &context)
        .await;
    assert_eq!(closed.status, CheckStatus::Unreachable);
    let empty = Antilopay::new()
        .check_connection(&ProviderConfig::default(), &context)
        .await;
    assert_eq!(empty.status, CheckStatus::SettingsRejected);
    let no_url = {
        let server = api("/payment/check", missing()).await;
        Antilopay::with_api_base(server.uri())
            .check_connection(&settings(), &CheckContext::default())
            .await
    };
    assert_eq!(no_url.status, CheckStatus::Ok);
    assert!(no_url.details.join("\n").contains("Адрес панели не задан"));
}

#[test]
fn смена_ключей_в_настройках_сбрасывает_разобранные_ключи() {
    let provider = Antilopay::new();
    let first = provider.keys(&settings()).unwrap();
    assert!(Arc::ptr_eq(&first, &provider.keys(&settings()).unwrap()));
    let mut changed = settings();
    changed.set("secret_key", json!(STRANGER));
    let second = provider.keys(&changed).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(
        Some(second.sign(b"{}").unwrap()),
        signature(STRANGER, b"{}")
    );
    let mut broken = settings();
    broken.set("callback_key", json!("MF"));
    assert!(provider.keys(&broken).is_err());
    assert!(provider.keys(&broken).is_err());
    assert!(provider.keys(&settings()).is_ok());
}
