use super::*;
use crate::test_support::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PUBLIC: &str = include_str!("testdata/public.pem");
const SIGNER: &str = include_str!("testdata/signer.pem");
const TOKEN_VALUE: &str = "eyJhbGciOiJIUzI1NiJ9.wata-terminal.sig";
const TX: &str = "3a16a4f0-27b0-09d1-16da-ba8d5c63eae3";
const REFUND_TX: &str = "b8c19d45-1234-4123-a5b6-890123456789";
const TERMINAL: &str = "3b16a2f1-dead-4e5d-abff-90865d1e13b1";
const VECTOR_BODY: &str = r#"{"transactionType":"SBP","kind":"Payment","transactionId":"3a16a4f0-27b0-09d1-16da-ba8d5c63eae3","transactionStatus":"Paid","terminalPublicId":"3b16a2f1-dead-4e5d-abff-90865d1e13b1","amount":1188.00,"currency":"RUB","orderId":"ord-000123","paymentTime":"2024-12-04T17:41:44.434598Z","commission":10}"#;
const VECTOR_SIGNATURE: &str = "y+zzFKYKBwbKiy22maJj7+JspdWrSNqGwbwaS8hrmG6J+dtpu3EyBHXkuFiYFwjlTKXTYEWn2fAfcKHkN5Eyw6lNa59z8jLFSDZdeXpmDXIXMHyskO3w0cO33G5UnFq4FPPr+m5DnaSFCPE7O9H5OkaQS55cB0kRgDYsR48NsSLbO6eAMKWDCgBEPUFYtwejGvGr7oyTorKxZrTEMHqpkr61Y6iqmCwbTkmdukgrEcyBilsu8gehy3JazR/HnA/oeCh4dz9BKoN3lgIVqmJ2qD2J+xgtru8DvHIqWgcKrsrdSYSFx8yF2Tk8yBF5St4KUP1T0/kDyzAbko1XEpL8YQ==";
const OTHER_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAm/qGeMsHlc+Y2+0mhSIX\nwH3LT26GGi4p2UCObiZ06ZjfftBIQ4jBG77LnwMTwWwcx1EFxQKwCOr2kIozzL4o\nE52gY5jG4DXSMIii1JGCAuq+J3kdZ55gWr8vg0CyW0kOQ1txZFvKVkBN6pMagLpH\nNwMxSZuDCDtUz3x0daDPcVTNZBjyb1ygOnLMQ7JAz8qLBOOjSPd6Q+LRAN6uUJcW\nHcNH5vTyB3gsLltRnHT2lxtSqikz8tbX2E8NQYmz1bC/CoYte2vawSOaThP2u/vt\nUypmOPye+Q0cgM7tz2Q6e9+bVRoSnSbjkcEHm75u6q1msewo/2qcGu6H402bvwAn\nkQIDAQAB\n-----END PUBLIC KEY-----\n";

fn settings() -> ProviderConfig {
    config(&[("api_token", json!(TOKEN_VALUE))])
}

fn sign(body: &[u8]) -> String {
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(SIGNER.as_bytes()).unwrap();
    let signed = jsonwebtoken::crypto::sign(body, &key, jsonwebtoken::Algorithm::RS512).unwrap();
    STANDARD.encode(URL_SAFE_NO_PAD.decode(signed).unwrap())
}

fn event(kind: &str, status: &str, id: &str, amount: &str) -> Vec<u8> {
    format!(
        "{{\"transactionType\":\"SBP\",\"kind\":\"{kind}\",\"id\":\"3a1cf611-abc6-8d30-c4cd-521c9f6eeeb0\",\"transactionId\":\"{id}\",\"originalTransactionId\":{original},\"transactionStatus\":\"{status}\",\"terminalPublicId\":\"{TERMINAL}\",\"errorCode\":null,\"errorDescription\":null,\"terminalName\":\"shop\",\"amount\":{amount},\"currency\":\"RUB\",\"orderId\":\"{PAYMENT}\",\"orderDescription\":\"Стандарт\",\"paymentTime\":\"2026-11-18T12:41:44.434598Z\",\"commission\":10,\"email\":\"client@example.com\",\"paymentLinkId\":null}}",
        original = if kind == "Refund" {
            format!("\"{TX}\"")
        } else {
            "null".to_string()
        }
    )
    .into_bytes()
}

fn signed(body: Vec<u8>) -> WebhookRequest {
    let signature = sign(&body);
    hook(
        &[
            ("Content-Type", "application/json"),
            ("X-Signature", signature.as_str()),
        ],
        body,
    )
}

fn tx_json(id: &str, kind: &str, status: &str, amount: f64) -> serde_json::Value {
    json!({
        "id": id,
        "terminalName": "shop",
        "terminalPublicId": TERMINAL,
        "type": "SBP",
        "kind": kind,
        "originalTransactionId": if kind == "Refund" { json!(TX) } else { json!(null) },
        "amount": amount,
        "currency": "RUB",
        "status": status,
        "errorCode": null,
        "errorDescription": null,
        "orderId": PAYMENT.to_string(),
        "orderDescription": "Стандарт",
        "creationTime": "2026-11-18T12:41:33.744768Z",
        "paymentTime": "2026-11-18T12:41:44.434598Z",
        "totalCommission": 10,
        "paymentLinkId": "3fa85f64-5717-4562-b3fc-2c963f66afa6"
    })
}

async fn api_with_key() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/public-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": PUBLIC })))
        .mount(&server)
        .await;
    server
}

async fn with_transaction(server: &MockServer, id: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(format!("/transactions/{id}")))
        .and(header("authorization", format!("Bearer {TOKEN_VALUE}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

fn hits(requests: &[wiremock::Request], route: &str) -> usize {
    requests.iter().filter(|r| r.url.path() == route).count()
}

#[test]
fn подпись_wata_проверяется_ключом_rsa() {
    assert!(verify_signature(
        PUBLIC,
        VECTOR_BODY.as_bytes(),
        VECTOR_SIGNATURE
    ));
    let wrapped: String = VECTOR_SIGNATURE
        .as_bytes()
        .chunks(76)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("\r\n");
    assert!(verify_signature(PUBLIC, VECTOR_BODY.as_bytes(), &wrapped));
    let changed = VECTOR_BODY.replace("1188.00", "1188.01");
    assert!(!verify_signature(
        PUBLIC,
        changed.as_bytes(),
        VECTOR_SIGNATURE
    ));
    let spaced = VECTOR_BODY.replace(",\"kind\"", ", \"kind\"");
    assert!(!verify_signature(
        PUBLIC,
        spaced.as_bytes(),
        VECTOR_SIGNATURE
    ));
    assert!(!verify_signature(
        OTHER_KEY,
        VECTOR_BODY.as_bytes(),
        VECTOR_SIGNATURE
    ));
    assert!(!verify_signature(
        PUBLIC,
        VECTOR_BODY.as_bytes(),
        "не base64"
    ));
    assert!(!verify_signature(
        "не ключ",
        VECTOR_BODY.as_bytes(),
        VECTOR_SIGNATURE
    ));
    let own = sign(VECTOR_BODY.as_bytes());
    assert_eq!(own, VECTOR_SIGNATURE);
}

#[tokio::test]
async fn ссылка_на_оплату_wata() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/links"))
        .and(header("authorization", format!("Bearer {TOKEN_VALUE}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
            "type": "OneTime",
            "amount": 1500.00,
            "currency": "RUB",
            "status": "Opened",
            "url": "https://payment.wata.pro/pay-form/3fa85f64-5717-4562-b3fc-2c963f66afa6",
            "terminalName": "shop",
            "terminalPublicId": TERMINAL,
            "creationTime": "2026-11-18T12:00:00.123Z",
            "orderId": PAYMENT.to_string(),
            "expirationDateTime": "2026-11-18T13:00:00Z",
            "isArbitraryAmountAllowed": false,
            "arbitraryAmounts": []
        })))
        .mount(&server)
        .await;
    let before = Utc::now();
    let link = Wata::with_api_base(server.uri())
        .create_invoice(
            &settings(),
            &invoice(rub(150_000), Some("client@example.com")),
        )
        .await
        .unwrap();
    assert_eq!(
        link.pay_url,
        "https://payment.wata.pro/pay-form/3fa85f64-5717-4562-b3fc-2c963f66afa6"
    );
    assert_eq!(link.external_id, None);
    assert_eq!(
        link.expires_at,
        Some("2026-11-18T13:00:00Z".parse::<DateTime<Utc>>().unwrap())
    );
    assert_eq!(link.payload["terminal"], TERMINAL);
    let requests = server.received_requests().await.unwrap();
    let sent: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let raw = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(raw.contains("\"amount\":1500.00"), "{raw}");
    assert_eq!(sent["type"], "OneTime");
    assert_eq!(sent["currency"], "RUB");
    assert_eq!(sent["orderId"], PAYMENT.to_string());
    assert_eq!(sent["description"], "Стандарт · 3 месяца");
    assert_eq!(
        sent["successRedirectUrl"],
        "https://panel.example.com/cabinet/payment-wait"
    );
    assert_eq!(sent["failRedirectUrl"], sent["successRedirectUrl"]);
    assert_eq!(sent["email"], "client@example.com");
    let until: DateTime<Utc> = sent["expirationDateTime"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(until >= before + Duration::minutes(59) && until <= Utc::now() + Duration::minutes(61));
}

#[test]
fn срок_ссылки_в_пределах_wata() {
    assert_eq!(lifetime(Duration::minutes(5)), Duration::minutes(15));
    assert_eq!(lifetime(Duration::hours(2)), Duration::hours(2));
    assert_eq!(lifetime(Duration::days(60)), Duration::days(30));
    assert_eq!(
        deadline(Some("2026-11-18T13:00:00.071Z")),
        Some("2026-11-18T13:00:00.071Z".parse().unwrap())
    );
    assert_eq!(
        deadline(Some("2026-11-18T13:00:00")),
        Some("2026-11-18T13:00:00Z".parse().unwrap())
    );
    assert_eq!(deadline(Some("завтра")), None);
}

#[tokio::test]
async fn ответ_на_ссылку_и_ошибки_wata() {
    let reply = |status: u16, body: serde_json::Value| async move {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/links"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        Wata::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(rub(150_000), None))
            .await
    };
    let base = json!({"amount": 1500.0, "currency": "RUB", "status": "Opened", "url": "https://payment.wata.pro/x", "orderId": PAYMENT.to_string()});
    let mut insecure = base.clone();
    insecure["url"] = json!("http://payment.wata.pro/x");
    assert!(matches!(
        reply(200, insecure).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut cheaper = base.clone();
    cheaper["amount"] = json!(15.0);
    assert!(matches!(
        reply(200, cheaper).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut other = base.clone();
    other["orderId"] = json!("ord-1");
    assert!(matches!(
        reply(200, other).await,
        Err(PaymentError::BadPayload(_))
    ));
    assert!(reply(200, base).await.is_ok());
    assert_eq!(
        reply(
            400,
            json!({"error": {"code": "Payment:PL_1001", "message": "Amount is out of range", "details": null, "data": null, "validationErrors": null}})
        )
        .await,
        Err(PaymentError::Rejected(
            "сумма вне пределов, которые WATA разрешает терминалу".into()
        ))
    );
    assert_eq!(
        reply(
            400,
            json!({"error": {"code": null, "message": null, "validationErrors": [{"message": "The Amount field is required.", "members": ["amount"]}]}})
        )
        .await,
        Err(PaymentError::Rejected("The Amount field is required.".into()))
    );
    let refused = reply(401, json!(null)).await.unwrap_err();
    assert!(util::keys_refused(&refused), "{refused:?}");
    assert_eq!(
        reply(429, json!({"error": {"message": "Retry after 45 seconds"}})).await,
        Err(PaymentError::Http { status: 429 })
    );
    let mut method_chosen = invoice(rub(150_000), None);
    method_chosen.method = Some("sbp".into());
    assert_eq!(
        Wata::new()
            .create_invoice(&settings(), &method_chosen)
            .await,
        Err(PaymentError::Rejected(
            "этот способ оплаты недоступен".into()
        ))
    );
}

#[tokio::test]
async fn оплата_засчитывается_после_проверки_транзакции_через_api() {
    let server = api_with_key().await;
    with_transaction(&server, TX, tx_json(TX, "Payment", "Paid", 1500.0)).await;
    let provider = Wata::with_api_base(server.uri());
    let outcome = provider
        .parse_webhook(&settings(), &signed(event("Payment", "Paid", TX, "1.00")))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.event_key, format!("{TX}:paid"));
    assert_eq!(outcome.provider_txid.as_deref(), Some(TX));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.reply, WebhookReply::ok());
    let again = provider
        .parse_webhook(&settings(), &signed(event("Payment", "Paid", TX, "1.00")))
        .await
        .unwrap();
    assert_eq!(again.event_key, outcome.event_key);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(hits(&requests, "/public-key"), 1);
    assert_eq!(hits(&requests, &format!("/transactions/{TX}")), 2);
}

#[tokio::test]
async fn уведомление_без_подписи_или_с_чужой_подписью_отклоняется() {
    let server = api_with_key().await;
    let provider = Wata::with_api_base(server.uri());
    let body = event("Payment", "Paid", TX, "1500.00");
    assert_eq!(
        provider
            .parse_webhook(&settings(), &hook(&[], body.clone()))
            .await,
        Err(PaymentError::BadSignature)
    );
    let mut tampered = signed(body.clone());
    tampered.body = String::from_utf8(body.clone())
        .unwrap()
        .replace("1500.00", "15000.00")
        .into_bytes()
        .into();
    assert_eq!(
        provider.parse_webhook(&settings(), &tampered).await,
        Err(PaymentError::BadSignature)
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(hits(&requests, &format!("/transactions/{TX}")), 0);
    assert_eq!(hits(&requests, "/public-key"), 1);
}

#[tokio::test]
async fn сменённый_ключ_wata_перечитывается_не_чаще_раза_в_минуту() {
    let server = api_with_key().await;
    with_transaction(&server, TX, tx_json(TX, "Payment", "Paid", 1500.0)).await;
    let provider = Wata::with_api_base(server.uri());
    let stale = |age: u64| CachedKey {
        pem: OTHER_KEY.to_string(),
        fetched: Instant::now()
            .checked_sub(std::time::Duration::from_secs(age))
            .unwrap(),
    };
    provider
        .keys
        .lock()
        .unwrap()
        .insert(server.uri(), stale(10));
    assert_eq!(
        provider
            .parse_webhook(
                &settings(),
                &signed(event("Payment", "Paid", TX, "1500.00"))
            )
            .await,
        Err(PaymentError::BadSignature)
    );
    assert_eq!(
        hits(&server.received_requests().await.unwrap(), "/public-key"),
        0
    );
    provider
        .keys
        .lock()
        .unwrap()
        .insert(server.uri(), stale(120));
    assert!(
        provider
            .parse_webhook(
                &settings(),
                &signed(event("Payment", "Paid", TX, "1500.00"))
            )
            .await
            .is_ok()
    );
    assert_eq!(
        hits(&server.received_requests().await.unwrap(), "/public-key"),
        1
    );
}

#[tokio::test]
async fn чужой_терминал_и_промежуточные_статусы() {
    let server = api_with_key().await;
    Mock::given(method("GET"))
        .and(path(format!("/transactions/{TX}")))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(
                json!({"error": {"code": null, "message": "Transaction not found"}}),
            ),
        )
        .mount(&server)
        .await;
    let provider = Wata::with_api_base(server.uri());
    assert!(matches!(
        provider
            .parse_webhook(
                &settings(),
                &signed(event("Payment", "Paid", TX, "1500.00"))
            )
            .await,
        Err(PaymentError::BadPayload(_))
    ));
    for status in ["Created", "Pending", "Declined"] {
        let outcome = provider
            .parse_webhook(
                &settings(),
                &signed(event("Payment", status, TX, "1500.00")),
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending, "{status}");
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.provider_txid, None);
        assert_eq!(outcome.reply, WebhookReply::ok());
    }
    assert_eq!(
        hits(
            &server.received_requests().await.unwrap(),
            &format!("/transactions/{TX}")
        ),
        1
    );
    let subscription = br#"{"eventType":"subscription_status_changed","subscriptionId":"3fa85f64-5717-4562-b3fc-2c963f66afa6","terminalPublicId":"3b16a2f1-dead-4e5d-abff-90865d1e13b1","previousStatus":"Active","status":"Completed","occurredAt":"2026-11-18T12:00:00Z"}"#.to_vec();
    let skipped = provider
        .parse_webhook(&settings(), &signed(subscription))
        .await
        .unwrap();
    assert_eq!(skipped.status, ProviderStatus::Pending);
    assert_eq!(skipped.payment, None);

    let mismatch = api_with_key().await;
    with_transaction(&mismatch, TX, tx_json(TX, "Refund", "Paid", 1500.0)).await;
    assert!(matches!(
        Wata::with_api_base(mismatch.uri())
            .parse_webhook(
                &settings(),
                &signed(event("Payment", "Paid", TX, "1500.00"))
            )
            .await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn возврат_wata_через_api() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/transactions/refunds"))
        .and(header("authorization", format!("Bearer {TOKEN_VALUE}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "transactionId": REFUND_TX,
            "originalTransactionId": TX,
            "transactionStatus": "Pending",
            "kind": "Refund",
            "errorCode": null,
            "errorDescription": null
        })))
        .mount(&server)
        .await;
    let provider = Wata::with_api_base(server.uri());
    let request = RefundRequest {
        payment_id: sn_domain::ids::PaymentId(41),
        provider_txid: TX.into(),
        amount: rub(50_000),
        reason: None,
        key: Uuid::from_u128(5),
        email: None,
        description: None,
    };
    let outcome = provider.refund(&settings(), &request).await.unwrap();
    assert_eq!(
        outcome,
        RefundOutcome {
            provider_refund_id: Some(REFUND_TX.into()),
            completed: false
        }
    );
    assert_eq!(
        provider.refund(&settings(), &request).await.unwrap(),
        outcome
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = String::from_utf8(requests[0].body.clone()).unwrap();
    assert_eq!(
        body,
        format!("{{\"originalTransactionId\":\"{TX}\",\"amount\":500.00}}")
    );

    let declined = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/transactions/refunds"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "transactionId": REFUND_TX, "originalTransactionId": TX,
            "transactionStatus": "Declined", "kind": "Refund",
            "errorCode": "Payment:TRA_1102", "errorDescription": "Insufficient balance"
        })))
        .mount(&declined)
        .await;
    let mut other = request.clone();
    other.key = Uuid::from_u128(6);
    assert_eq!(
        Wata::with_api_base(declined.uri())
            .refund(&settings(), &other)
            .await,
        Err(PaymentError::Rejected(
            "на балансе терминала WATA не хватает денег на возврат".into()
        ))
    );
    let busy = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/transactions/refunds"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"code": "Payment:TRA_1103", "message": "Refund already in progress"}
        })))
        .mount(&busy)
        .await;
    assert_eq!(
        Wata::with_api_base(busy.uri())
            .refund(&settings(), &other)
            .await,
        Err(PaymentError::Rejected(
            "по этой оплате в WATA уже идёт возврат, дождитесь его результата".into()
        ))
    );
    let mut broken = other.clone();
    broken.provider_txid = "55501".into();
    assert!(matches!(
        provider.refund(&settings(), &broken).await,
        Err(PaymentError::Rejected(_))
    ));
}

#[tokio::test]
async fn итог_возврата_wata_по_опросу_и_уведомлению() {
    for (status, expected) in [
        (
            "Paid",
            ProviderStatus::PartiallyRefunded {
                refunded: rub(50_000),
            },
        ),
        ("Declined", ProviderStatus::RefundDeclined),
        ("Pending", ProviderStatus::Pending),
    ] {
        let server = api_with_key().await;
        with_transaction(
            &server,
            REFUND_TX,
            tx_json(REFUND_TX, "Refund", status, 500.0),
        )
        .await;
        let provider = Wata::with_api_base(server.uri());
        let polled = provider
            .fetch_refund(&settings(), REFUND_TX)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(polled.status, expected, "{status}");
        assert!(
            polled.event_key.starts_with(&format!("refund:{REFUND_TX}")),
            "{}",
            polled.event_key
        );
        if status != "Pending" {
            let notified = provider
                .parse_webhook(
                    &settings(),
                    &signed(event("Refund", status, REFUND_TX, "500.00")),
                )
                .await
                .unwrap();
            assert_eq!(notified.status, expected, "{status}");
            assert_eq!(notified.event_key, format!("refund:{REFUND_TX}"));
            assert_eq!(notified.provider_txid.as_deref(), Some(TX));
            assert_eq!(notified.payment, Some(PaymentRef::PublicId(PAYMENT)));
        }
    }
    let server = api_with_key().await;
    with_transaction(&server, TX, tx_json(TX, "Payment", "Paid", 1500.0)).await;
    assert!(matches!(
        Wata::with_api_base(server.uri())
            .fetch_refund(&settings(), TX)
            .await,
        Err(PaymentError::BadPayload(_))
    ));
    assert_eq!(
        Wata::new().fetch_refund(&settings(), "refund-1").await,
        Ok(None)
    );
}

#[tokio::test]
async fn опрос_статуса_wata_по_номеру_заказа() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/transactions"))
        .and(query_param("OrderId", PAYMENT.to_string()))
        .and(header("authorization", format!("Bearer {TOKEN_VALUE}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                tx_json("11111111-1111-4111-8111-111111111111", "Payment", "Declined", 1500.0),
                tx_json(TX, "Payment", "Paid", 1500.0)
            ],
            "hasNextPage": false
        })))
        .mount(&server)
        .await;
    let outcome = Wata::with_api_base(server.uri())
        .fetch_status(&settings(), &PAYMENT.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.provider_txid.as_deref(), Some(TX));
    assert_eq!(outcome.amount, Some(rub(150_000)));

    let open = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/transactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [tx_json(TX, "Payment", "Declined", 1500.0)],
            "hasNextPage": false
        })))
        .mount(&open)
        .await;
    let outcome = Wata::with_api_base(open.uri())
        .fetch_status(&settings(), &PAYMENT.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    assert_eq!(outcome.provider_txid, None);

    let limited = failing_api(429).await;
    assert_eq!(
        Wata::with_api_base(limited.uri())
            .fetch_status(&settings(), &PAYMENT.to_string())
            .await,
        Err(PaymentError::Http { status: 429 })
    );
    assert!(matches!(
        Wata::new().fetch_status(&settings(), "55501").await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn проверка_подключения_wata() {
    let server = api_with_key().await;
    Mock::given(method("GET"))
        .and(path("/finance/balance"))
        .and(header("authorization", format!("Bearer {TOKEN_VALUE}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "terminalPublicId": TERMINAL, "date": "2026-11-18", "balance": 1108.32, "currency": "RUB"
        })))
        .mount(&server)
        .await;
    let context = CheckContext {
        webhook_url: Some("https://panel.example.com/api/webhooks/pay/wata".into()),
    };
    let result = Wata::with_api_base(server.uri())
        .check_connection(&settings(), &context)
        .await;
    assert_eq!(result.status, CheckStatus::Ok, "{result:?}");
    let text = result.details.join("\n");
    assert!(text.contains(TERMINAL), "{text}");
    assert!(
        text.contains("https://panel.example.com/api/webhooks/pay/wata"),
        "{text}"
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains(TOKEN_VALUE)
    );

    let fallback = api_with_key().await;
    Mock::given(method("GET"))
        .and(path("/finance/balance"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&fallback)
        .await;
    Mock::given(method("GET"))
        .and(path("/links"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items": [], "totalCount": 0})),
        )
        .mount(&fallback)
        .await;
    assert_eq!(
        Wata::with_api_base(fallback.uri())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::Ok
    );

    let refused = failing_api(401).await;
    let mut sandbox = settings();
    sandbox.set("sandbox", json!(true));
    let result = Wata::with_api_base(refused.uri())
        .check_connection(&sandbox, &context)
        .await;
    assert_eq!(result.status, CheckStatus::KeysRejected);
    assert!(result.details.join("\n").contains("тестового кабинета"));
    let limited = failing_api(429).await;
    assert_eq!(
        Wata::with_api_base(limited.uri())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::Unreachable
    );
    let closed = Wata::with_api_base(closed_api())
        .check_connection(&settings(), &context)
        .await;
    assert_eq!(closed.status, CheckStatus::Unreachable);
}

#[test]
fn настройки_wata() {
    let provider = Wata::new();
    assert!(provider.is_configured(&settings()));
    assert!(!provider.is_configured(&ProviderConfig::default()));
    assert!(
        provider
            .validate_settings(&config(&[("api_token", json!("two words"))]))
            .is_err()
    );
    let mut sandbox = settings();
    sandbox.set("sandbox", json!(true));
    assert_eq!(provider.base(&sandbox), SANDBOX);
    assert_eq!(provider.base(&settings()), LIVE);
    assert!(provider.methods(Currency::Rub).is_empty());
    assert_eq!(provider.currencies(), &[Currency::Rub, Currency::Usd]);
    assert!(provider.polls_by_payment_id() && provider.status_by_order());
    let schema = provider.settings_schema();
    assert!(schema.iter().any(|f| f.key == "api_token" && f.is_secret()));
    for currency in [Currency::Rub, Currency::Usd] {
        let label = provider.client_label(currency, &settings());
        assert!(crate::english::text(label.title).is_some());
        assert!(crate::english::text(label.hint).is_some());
    }
}

#[tokio::test]
async fn терминал_из_настроек_сверяется_с_уведомлением_транзакцией_и_токеном() {
    const OTHER: &str = "0f0e0d0c-0b0a-4909-8807-060504030201";
    let provider = Wata::new();
    assert!(provider.verifies_webhooks_by_api());
    let mut own = settings();
    own.set("terminal_id", json!(TERMINAL));
    let mut other = settings();
    other.set("terminal_id", json!(OTHER));
    let mut broken = settings();
    broken.set("terminal_id", json!("мой терминал"));
    assert!(provider.validate_settings(&own).is_ok());
    assert!(provider.validate_settings(&other).is_ok());
    assert!(provider.validate_settings(&broken).is_err());
    assert!(
        provider
            .settings_schema()
            .iter()
            .any(|f| f.key == "terminal_id" && !f.is_secret())
    );

    let server = api_with_key().await;
    with_transaction(&server, TX, tx_json(TX, "Payment", "Paid", 1500.0)).await;
    let wata = Wata::with_api_base(server.uri());
    let paid = wata
        .parse_webhook(&own, &signed(event("Payment", "Paid", TX, "1500.00")))
        .await
        .unwrap();
    assert_eq!(paid.status, ProviderStatus::Paid);
    assert!(matches!(
        wata.parse_webhook(&other, &signed(event("Payment", "Paid", TX, "1500.00")))
            .await,
        Err(PaymentError::BadPayload(_))
    ));

    let moved = api_with_key().await;
    let mut tx = tx_json(TX, "Payment", "Paid", 1500.0);
    tx["terminalPublicId"] = json!(OTHER);
    with_transaction(&moved, TX, tx).await;
    let wata = Wata::with_api_base(moved.uri());
    assert!(matches!(
        wata.parse_webhook(&own, &signed(event("Payment", "Paid", TX, "1500.00")))
            .await,
        Err(PaymentError::BadPayload(_))
    ));
    assert_eq!(
        wata.parse_webhook(
            &settings(),
            &signed(event("Payment", "Paid", TX, "1500.00"))
        )
        .await
        .unwrap()
        .status,
        ProviderStatus::Paid
    );

    let balance = api_with_key().await;
    Mock::given(method("GET"))
        .and(path("/finance/balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "terminalPublicId": TERMINAL, "date": "2026-11-18", "balance": 10.0, "currency": "RUB"
        })))
        .mount(&balance)
        .await;
    let context = CheckContext {
        webhook_url: Some("https://panel.example.com/api/webhooks/pay/wata".into()),
    };
    let wata = Wata::with_api_base(balance.uri());
    let wrong = wata.check_connection(&other, &context).await;
    assert_eq!(wrong.status, CheckStatus::SettingsRejected, "{wrong:?}");
    assert!(wrong.details.join("\n").contains(TERMINAL), "{wrong:?}");
    let right = wata.check_connection(&own, &context).await;
    assert_eq!(right.status, CheckStatus::Ok, "{right:?}");
    assert!(
        !right.details.join("\n").contains("«ID терминала»"),
        "{right:?}"
    );
    let unset = wata.check_connection(&settings(), &context).await;
    assert_eq!(unset.status, CheckStatus::Ok, "{unset:?}");
    assert!(
        unset.details.join("\n").contains("«ID терминала»"),
        "{unset:?}"
    );
}
