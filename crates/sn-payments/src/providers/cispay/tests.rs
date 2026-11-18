use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::ids::PaymentId;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DOC_KEY: &str = "cis_sec_live_9b2e04f32a76cc89";
const SHOP: &str = "0193a1b2-7c3d-7e4f-8a9b-0c1d2e3f4a5b";
const KEY: &str = "cis_sec_live_e2e0a1b2c3";
const TX: &str = "0193a1b2-7c3d-7e4f-8a9b-0c1d2e3f4a60";
const HOOK: &str = "https://panel.example.com/api/webhooks/pay/cispay";
const V1: &str = r#"{"id":"7fa12a88-294b-4b11-bbfe-e69c3dbeaab9","status":"PAID"}"#;
const V2: &str = r#"{"id":"7fa12a88-294b-4b11-bbfe-e69c3dbeaab9","store_id":"c56d9539-7814-4112-9c44-59e55728a3bd","order_id":"ORDER-99238","payment_method":"CARD","status":"PAID","amount":150000,"currency":"RUB","charged_amount":150000,"merchant_revenue":144000,"is_sandbox":false,"description":null,"subscription_id":null,"paid_at":"2026-07-13T10:02:14+00:00","timestamp":"2026-07-13T10:02:15+00:00"}"#;

fn settings() -> ProviderConfig {
    config(&[("shop_id", json!(SHOP)), ("api_key", json!(KEY))])
}

fn testing() -> ProviderConfig {
    let mut settings = settings();
    settings.set("test_mode", json!(true));
    settings
}

fn remote(status: &str, amount: i64) -> serde_json::Value {
    json!({
        "id": TX, "order_id": PAYMENT.to_string(), "status": status, "amount": amount,
        "charged_amount": amount, "currency": "RUB", "payment_method": "SBP",
        "is_sandbox": false, "block_reason": null, "store_name": "VPN",
        "created_at": "2026-11-18T12:00:00+00:00", "expires_in_seconds": 0
    })
}

async fn api(body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/payments/status"))
        .and(query_param("id", TX))
        .and(header("X-Shop-ID", SHOP))
        .and(header("X-Api-Key", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/payments/status"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"detail": "Не найдено"})))
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

fn notice(status: &str, amount: i64) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": TX, "store_id": SHOP, "order_id": PAYMENT.to_string(), "payment_method": "SBP",
        "status": status, "amount": amount, "currency": "RUB", "charged_amount": amount,
        "merchant_revenue": amount - 5_000, "is_sandbox": false, "description": "Стандарт",
        "subscription_id": null, "paid_at": "2026-11-18T12:01:00+00:00",
        "timestamp": "2026-11-18T12:01:01+00:00"
    }))
    .unwrap()
}

fn signed(body: Vec<u8>) -> WebhookRequest {
    let sign = signature(KEY, &body);
    hook(&[("X-Signature", sign.as_str())], body)
}

async fn parse(
    server: &MockServer,
    config: &ProviderConfig,
    request: &WebhookRequest,
) -> PaymentResult<WebhookOutcome> {
    CisPay::with_api_base(server.uri())
        .parse_webhook(config, request)
        .await
}

#[test]
fn подпись_cispay_по_векторам() {
    assert_eq!(
        signature(DOC_KEY, V1.as_bytes()),
        "98c036432538aaa536263e5c6fa284c76a923dddae6777e6578f678f3f61bd8c"
    );
    assert_eq!(
        signature(DOC_KEY, V2.as_bytes()),
        "f2afd4a89e17caecca08ae502fd8cf4d73384568341eb45f23e482e456afcf0c"
    );
    assert_eq!(
        signature(
            DOC_KEY,
            r#"{"event":"subscription.activated","object":"subscription","subscription_purpose":"Подписка PRO"}"#.as_bytes()
        ),
        "077a6ebfc0368ca2fb2f05a316603f116738054a548d551795b3f4a86c9bfc25"
    );
    let good = signature(DOC_KEY, V1.as_bytes());
    assert!(verify(DOC_KEY, V1.as_bytes(), &good));
    assert!(verify(DOC_KEY, V1.as_bytes(), &good.to_ascii_uppercase()));
    assert!(!verify("cis_sec_other", V1.as_bytes(), &good));
    assert!(!verify(DOC_KEY, format!("{V1} ").as_bytes(), &good));
    assert!(!verify(DOC_KEY, V1.as_bytes(), "zz"));
}

#[tokio::test]
async fn оплата_засчитывается_после_проверки_статуса() {
    let server = api(remote("PAID", 150_000)).await;
    let outcome = parse(&server, &settings(), &signed(notice("PAID", 150_000)))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.event_key, format!("{TX}:paid"));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some(TX));
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.reply, WebhookReply::ok());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/payments/status");
}

#[tokio::test]
async fn статус_берётся_из_api_а_не_из_уведомления() {
    let server = api(remote("PENDING", 150_000)).await;
    let outcome = parse(&server, &settings(), &signed(notice("PAID", 150_000)))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
}

#[tokio::test]
async fn без_подписи_или_с_чужой_подписью_уведомление_отклоняется() {
    let server = api(remote("PAID", 150_000)).await;
    let body = notice("PAID", 150_000);
    assert_eq!(
        parse(&server, &settings(), &hook(&[], body.clone())).await,
        Err(PaymentError::BadSignature)
    );
    let forged = signature("cis_sec_guess", &body);
    assert_eq!(
        parse(
            &server,
            &settings(),
            &hook(&[("X-Signature", forged.as_str())], body.clone())
        )
        .await,
        Err(PaymentError::BadSignature)
    );
    let genuine = signature(KEY, &body);
    let mut altered = body.clone();
    altered.push(b' ');
    assert_eq!(
        parse(
            &server,
            &settings(),
            &hook(&[("X-Signature", genuine.as_str())], altered)
        )
        .await,
        Err(PaymentError::BadSignature)
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn чужой_магазин_и_чужой_заказ_не_засчитываются() {
    let server = api(remote("PAID", 150_000)).await;
    let mut value: serde_json::Value = serde_json::from_slice(&notice("PAID", 150_000)).unwrap();
    value["store_id"] = json!("0193a1b2-7c3d-7e4f-8a9b-0c1d2e3f4aff");
    assert!(matches!(
        parse(
            &server,
            &settings(),
            &signed(serde_json::to_vec(&value).unwrap())
        )
        .await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut other = remote("PAID", 150_000);
    other["order_id"] = json!("чужой заказ");
    let server = api(other).await;
    assert!(matches!(
        parse(&server, &settings(), &signed(notice("PAID", 150_000))).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn песочница_засчитывается_только_в_тестовом_режиме() {
    let mut sandbox = remote("PAID", 150_000);
    sandbox["is_sandbox"] = json!(true);
    let server = api(sandbox).await;
    let skipped = parse(&server, &settings(), &signed(notice("PAID", 150_000)))
        .await
        .unwrap();
    assert_eq!(skipped.status, ProviderStatus::Pending);
    assert_eq!(skipped.event_key, format!("{TX}:paid:sandbox"));
    let counted = parse(&server, &testing(), &signed(notice("PAID", 150_000)))
        .await
        .unwrap();
    assert_eq!(counted.status, ProviderStatus::Paid);
    let mut blocked = remote("PAID", 150_000);
    blocked["is_sandbox"] = json!(true);
    blocked["block_reason"] = json!("Оплата недоступна");
    let server = api(blocked).await;
    let outcome = parse(&server, &testing(), &signed(notice("PAID", 150_000)))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    assert_eq!(outcome.event_key, format!("{TX}:paid:blocked"));
}

#[tokio::test]
async fn крипта_по_рублёвому_счёту_сверяется_исходной_суммой() {
    let mut crypto = remote("PAID", 1_634);
    crypto["currency"] = json!("USD");
    crypto["payment_method"] = json!("CRYPTO");
    crypto["source_amount"] = json!(150_000);
    crypto["source_currency"] = json!("RUB");
    crypto["fx_rate"] = json!(91.8);
    let server = api(crypto).await;
    let outcome = parse(&server, &settings(), &signed(notice("PAID", 1_634)))
        .await
        .unwrap();
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(
        check_paid_amount(rub(150_000), outcome.amount),
        AmountCheck::Ok
    );
}

#[tokio::test]
async fn статусы_cispay() {
    for (status, expected, key) in [
        ("PAID", ProviderStatus::Paid, format!("{TX}:paid")),
        ("FAILED", ProviderStatus::Failed, format!("{TX}:failed")),
        ("EXPIRED", ProviderStatus::Canceled, format!("{TX}:expired")),
        ("REFUNDED", ProviderStatus::Refunded, format!("refund:{TX}")),
        ("PENDING", ProviderStatus::Pending, format!("{TX}:pending")),
    ] {
        let server = api(remote(status, 150_000)).await;
        let outcome = parse(&server, &settings(), &signed(notice(status, 150_000)))
            .await
            .unwrap();
        assert_eq!(outcome.status, expected, "{status}");
        assert_eq!(outcome.event_key, key, "{status}");
    }
}

#[tokio::test]
async fn сумма_не_сошлась_и_повтор() {
    let server = api(remote("PAID", 149_999)).await;
    let outcome = parse(&server, &settings(), &signed(notice("PAID", 149_999)))
        .await
        .unwrap();
    assert!(matches!(
        check_paid_amount(rub(150_000), outcome.amount),
        AmountCheck::Underpaid { .. }
    ));
    let server = api(remote("PAID", 150_000)).await;
    let first = parse(&server, &settings(), &signed(notice("PAID", 150_000)))
        .await
        .unwrap();
    let second = parse(&server, &settings(), &signed(notice("PAID", 150_000)))
        .await
        .unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn события_подписок_пропускаются() {
    let server = MockServer::start().await;
    let body = serde_json::to_vec(&json!({
        "event": "subscription.cancelled", "object": "subscription", "id": "sub-1",
        "store_id": SHOP, "status": "CANCELLED", "reason": "unbound_in_bank"
    }))
    .unwrap();
    let outcome = parse(&server, &settings(), &signed(body)).await.unwrap();
    assert_eq!(outcome.payment, None);
    assert_eq!(
        outcome.event_key,
        "subscription:subscription.cancelled:sub-1"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

fn created(amount: i64) -> ResponseTemplate {
    ResponseTemplate::new(201).set_body_json(json!({
        "id": TX, "order_id": PAYMENT.to_string(), "status": "PENDING", "amount": amount,
        "charged_amount": amount, "payment_url": format!("https://cispay.app/pay/{TX}"),
        "created_at": "2026-11-18T12:00:00+00:00"
    }))
}

async fn create(
    server: &MockServer,
    amount: Amount,
    chosen: Option<&str>,
) -> PaymentResult<Invoice> {
    let mut request = invoice(amount, None);
    request.webhook_url = HOOK.into();
    request.method = chosen.map(str::to_string);
    CisPay::with_api_base(server.uri())
        .create_invoice(&settings(), &request)
        .await
}

#[tokio::test]
async fn счёт_cispay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/payments"))
        .and(header("X-Shop-ID", SHOP))
        .and(header("X-Api-Key", KEY))
        .and(body_json(json!({
            "amount": 150_000,
            "currency": "RUB",
            "order_id": PAYMENT.to_string(),
            "customer_id": CLIENT.to_string(),
            "description": "Стандарт · 3 месяца",
            "redirect_success_url": "https://panel.example.com/cabinet/payment-wait",
            "redirect_fail_url": "https://panel.example.com/cabinet/payment-wait"
        })))
        .respond_with(created(150_000))
        .mount(&server)
        .await;
    let invoice = create(&server, rub(150_000), None).await.unwrap();
    assert_eq!(invoice.pay_url, format!("https://cispay.app/pay/{TX}"));
    assert_eq!(invoice.external_id.as_deref(), Some(TX));
}

#[tokio::test]
async fn способ_оплаты_и_валюта_уходят_в_счёт() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/payments"))
        .respond_with(created(150_000))
        .mount(&server)
        .await;
    for (chosen, wire) in [
        (Some("sbp"), Some("SBP")),
        (Some("card"), Some("CARD")),
        (Some("crypto"), Some("CRYPTO")),
        (Some("page"), None),
        (None, None),
    ] {
        create(&server, rub(150_000), chosen).await.unwrap();
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent.last().unwrap().body).unwrap();
        assert_eq!(
            body.get("payment_method").and_then(|m| m.as_str()),
            wire,
            "{chosen:?}"
        );
    }
    assert_eq!(
        create(&server, rub(4_999), Some("card")).await,
        Err(PaymentError::Rejected(
            "картой через CisPay можно оплатить от 50 ₽".into()
        ))
    );
    assert!(matches!(
        create(&server, usd(1_000), Some("sbp")).await,
        Err(PaymentError::Rejected(_))
    ));
    let dollars = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/payments"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": TX, "order_id": PAYMENT.to_string(), "status": "PENDING", "amount": 1_000,
            "charged_amount": 1_000, "payment_url": format!("https://cispay.app/pay/{TX}"),
            "created_at": "2026-11-18T12:00:00+00:00"
        })))
        .mount(&dollars)
        .await;
    create(&dollars, usd(1_000), None).await.unwrap();
    let sent = dollars.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["currency"], "USD");
    assert_eq!(body["amount"], 1_000);
    assert_eq!(body.get("payment_method"), None);
}

#[tokio::test]
async fn ответ_на_счёт_проверяется() {
    for (response, bad) in [
        (created(149_000), true),
        (
            ResponseTemplate::new(201).set_body_json(json!({
                "id": TX, "order_id": "другой", "status": "PENDING", "amount": 150_000,
                "charged_amount": 150_000, "payment_url": "https://cispay.app/pay/x",
                "created_at": "2026-11-18T12:00:00+00:00"
            })),
            true,
        ),
        (
            ResponseTemplate::new(201).set_body_json(json!({
                "id": TX, "order_id": PAYMENT.to_string(), "status": "PENDING", "amount": 150_000,
                "charged_amount": 150_000, "payment_url": null,
                "created_at": "2026-11-18T12:00:00+00:00"
            })),
            true,
        ),
        (
            ResponseTemplate::new(400)
                .set_body_json(json!({"detail": "Заказ с таким order_id уже есть"})),
            false,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/payments"))
            .respond_with(response)
            .mount(&server)
            .await;
        let result = create(&server, rub(150_000), None).await;
        if bad {
            assert!(
                matches!(result, Err(PaymentError::BadPayload(_))),
                "{result:?}"
            );
        } else {
            assert_eq!(
                result,
                Err(PaymentError::Rejected(
                    "Заказ с таким order_id уже есть".into()
                ))
            );
        }
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "detail": [{"loc": ["body", "customer_id"], "msg": "Field required", "type": "missing"}]
        })))
        .mount(&server)
        .await;
    assert_eq!(
        create(&server, rub(150_000), None).await,
        Err(PaymentError::Rejected("customer_id: Field required".into()))
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"detail": "Некорректный ключ"})),
        )
        .mount(&server)
        .await;
    assert_eq!(
        create(&server, rub(150_000), None).await,
        Err(PaymentError::Rejected(util::KEYS_REFUSED.into()))
    );
}

#[tokio::test]
async fn опрос_статуса_cispay() {
    let server = api(remote("PAID", 150_000)).await;
    let provider = CisPay::with_api_base(server.uri());
    let outcome = provider
        .fetch_status(&settings(), TX)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(
        provider
            .fetch_status(&settings(), "0193a1b2-7c3d-7e4f-8a9b-0c1d2e3f4a99")
            .await,
        Err(PaymentError::Rejected("платёж не найден в CisPay".into()))
    );
}

fn refund_request(amount: i64) -> RefundRequest {
    RefundRequest {
        payment_id: PaymentId(41),
        provider_txid: TX.into(),
        amount: rub(amount),
        reason: None,
        key: Uuid::from_u128(21),
        email: None,
        description: None,
    }
}

async fn refund_api(payment: serde_json::Value) -> MockServer {
    let server = api(payment).await;
    Mock::given(method("POST"))
        .and(path("/payments/refund"))
        .and(query_param("id", TX))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "transaction_id": TX, "status": "REFUNDED", "refund_status": "SUCCESS"
        })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn возврат_cispay_полный_и_частичный() {
    let server = refund_api(remote("PAID", 150_000)).await;
    let provider = CisPay::with_api_base(server.uri());
    let full = provider
        .refund(&settings(), &refund_request(150_000))
        .await
        .unwrap();
    assert_eq!(
        full,
        RefundOutcome {
            provider_refund_id: Some(format!("{TX}:refund:{}", Uuid::from_u128(21).simple())),
            completed: true
        }
    );
    provider
        .refund(&settings(), &refund_request(50_000))
        .await
        .unwrap();
    let bodies: Vec<serde_json::Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/payments/refund")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(bodies[0], json!({"confirm_amount": 150_000}));
    assert_eq!(
        bodies[1],
        json!({"amount": 50_000, "confirm_amount": 50_000})
    );
    assert!(matches!(
        provider.refund(&settings(), &refund_request(150_001)).await,
        Err(PaymentError::Rejected(_))
    ));
}

#[tokio::test]
async fn возврат_с_комиссией_покупателя_не_возвращает_лишнего() {
    let mut paid = remote("PAID", 150_000);
    paid["charged_amount"] = json!(155_000);
    let server = refund_api(paid).await;
    CisPay::with_api_base(server.uri())
        .refund(&settings(), &refund_request(150_000))
        .await
        .unwrap();
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path() == "/payments/refund")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body, json!({"amount": 150_000, "confirm_amount": 150_000}));
}

#[tokio::test]
async fn возврат_невозможен_для_крипты_и_неоплаченного() {
    let mut crypto = remote("PAID", 1_634);
    crypto["payment_method"] = json!("CRYPTO");
    crypto["currency"] = json!("USD");
    crypto["source_amount"] = json!(150_000);
    crypto["source_currency"] = json!("RUB");
    for payment in [
        crypto,
        remote("PENDING", 150_000),
        remote("REFUNDED", 150_000),
    ] {
        let server = refund_api(payment).await;
        assert!(matches!(
            CisPay::with_api_base(server.uri())
                .refund(&settings(), &refund_request(150_000))
                .await,
            Err(PaymentError::Rejected(_))
        ));
        assert!(
            !server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path() == "/payments/refund")
        );
    }
}

#[tokio::test]
async fn проверка_подключения_cispay() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/store/capabilities"))
        .and(header("X-Api-Key", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "store_id": SHOP, "store_name": "VPN Shop", "is_active": true,
            "payment_methods": [
                {"payment_method": "SBP", "is_active": true, "system_fee_percent": 350, "customer_fee_share_percent": 0},
                {"payment_method": "CARD", "is_active": false, "system_fee_percent": 450, "customer_fee_share_percent": 0}
            ],
            "direct_sbp_enabled": false
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"detail": "Некорректный ключ"})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    let provider = CisPay::with_api_base(server.uri());
    let context = CheckContext {
        webhook_url: Some(HOOK.into()),
    };
    let ok = provider.check_connection(&settings(), &context).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
    assert!(ok.summary.contains("VPN Shop"));
    assert_eq!(ok.details[0], "Включены способы: СБП");
    assert!(ok.details.iter().any(|d| d.contains(HOOK)));
    let sandbox = provider.check_connection(&testing(), &context).await;
    assert!(
        sandbox
            .details
            .iter()
            .any(|d| d.contains("тестовые платежи"))
    );
    let mut wrong = settings();
    wrong.set("api_key", json!("cis_sec_wrong"));
    assert_eq!(
        provider.check_connection(&wrong, &context).await.status,
        CheckStatus::KeysRejected
    );
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "store_id": "0193a1b2-7c3d-7e4f-8a9b-0c1d2e3f4aff", "store_name": "Чужой",
            "is_active": true, "payment_methods": []
        })))
        .mount(&other)
        .await;
    assert_eq!(
        CisPay::with_api_base(other.uri())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::SettingsRejected
    );
    let off = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "store_id": SHOP, "store_name": "VPN", "is_active": false, "payment_methods": []
        })))
        .mount(&off)
        .await;
    assert_eq!(
        CisPay::with_api_base(off.uri())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::SettingsRejected
    );
    let down = failing_api(502).await;
    assert_eq!(
        CisPay::with_api_base(down.uri())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::Unreachable
    );
}

#[test]
fn настройки_и_способы_cispay() {
    let provider = CisPay::new();
    assert!(provider.is_configured(&settings()));
    assert!(provider.accepts_http_webhooks());
    assert!(provider.supports_refunds());
    assert!(!provider.supports_refund_polling());
    assert_eq!(provider.currencies(), &[Currency::Rub, Currency::Usd]);
    let codes: Vec<&str> = provider
        .methods(Currency::Rub)
        .iter()
        .map(|m| m.code)
        .collect();
    assert_eq!(codes, ["sbp", "card", "crypto", "page"]);
    assert_eq!(
        provider.method(Currency::Rub, "card").unwrap().min_minor,
        Some(5_000)
    );
    assert!(provider.methods(Currency::Usd).is_empty());
    assert_eq!(
        provider.client_label(Currency::Usd, &settings()).title,
        "Криптовалюта"
    );
    for method in provider.methods(Currency::Rub) {
        assert!(crate::english::text(method.title).is_some() || method.title == "SberPay");
        assert!(
            crate::english::text(method.hint).is_some(),
            "{}",
            method.hint
        );
    }
    for label in [RUB_LABEL, USD_LABEL] {
        assert!(
            crate::english::text(label.title).is_some(),
            "{}",
            label.title
        );
        assert!(crate::english::text(label.hint).is_some(), "{}", label.hint);
    }
    for (key, value) in [("shop_id", json!("12345")), ("api_key", json!("cis sec"))] {
        let mut broken = settings();
        broken.set(key, value.clone());
        assert!(
            provider.validate_settings(&broken).is_err(),
            "{key} {value}"
        );
    }
    assert!(!provider.is_configured(&ProviderConfig::default()));
}
