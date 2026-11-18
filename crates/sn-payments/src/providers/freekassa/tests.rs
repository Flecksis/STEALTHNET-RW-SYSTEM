use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::payment::check_paid_amount;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SHOP_ID: &str = "777";
const KEY: &str = "fk_api_key_5QxR9";
const WORD1: &str = "first-secret";
const WORD2: &str = "second-secret";
const SERVER: &str = "203.0.113.10";
const HASH: &str = "bd4161db429848651499aabcb1d89330";

fn settings() -> ProviderConfig {
    config(&[
        ("shop_id", json!(SHOP_ID)),
        ("api_key", json!(KEY)),
        ("secret_word_2", json!(WORD2)),
        ("server_ip", json!(SERVER)),
    ])
}

fn with(mut config: ProviderConfig, pairs: &[(&str, serde_json::Value)]) -> ProviderConfig {
    for (key, value) in pairs {
        config.set(key, value.clone());
    }
    config
}

fn full() -> ProviderConfig {
    with(settings(), &[("secret_word_1", json!(WORD1))])
}

fn query(url: &str) -> HashMap<String, String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn notice(amount: &str, signed_amount: &str, order: &str, secret: &str) -> String {
    let sign = notice_signature(SHOP_ID, signed_amount, secret, order).to_uppercase();
    [
        ("MERCHANT_ID", SHOP_ID.to_string()),
        ("AMOUNT", amount.to_string()),
        ("intid", "55501".to_string()),
        ("MERCHANT_ORDER_ID", order.to_string()),
        ("P_EMAIL", "client@example.com".to_string()),
        ("P_PHONE", "79001234567".to_string()),
        ("CUR_ID", "44".to_string()),
        ("payer_account", "220220******1234".to_string()),
        ("commission", "52.5".to_string()),
        ("SIGN", sign),
    ]
    .iter()
    .map(|(k, v)| format!("{k}={}", util::rfc3986(v)))
    .collect::<Vec<_>>()
    .join("&")
}

async fn parse(config: &ProviderConfig, body: String) -> PaymentResult<WebhookOutcome> {
    Freekassa::new()
        .parse_webhook(config, &hook(&[], body))
        .await
}

fn bodies(requests: &[wiremock::Request], route: &str) -> Vec<serde_json::Value> {
    requests
        .iter()
        .filter(|r| r.url.path() == route)
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

fn signed_by_key(body: &serde_json::Value) -> bool {
    let object = body.as_object().unwrap();
    let values: Vec<(String, String)> = object
        .iter()
        .filter(|(k, _)| *k != "signature")
        .map(|(k, v)| {
            let text = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), text)
        })
        .collect();
    let pairs: Vec<(&str, &str)> = values
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    object["signature"] == api_signature(KEY, &pairs)
}

async fn api(route: &str, reply: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply))
        .mount(&server)
        .await;
    server
}

fn created() -> serde_json::Value {
    json!({
        "type": "success",
        "orderId": 55501,
        "orderHash": HASH,
        "location": format!("https://pay.freekassa.net/form/55501/{HASH}")
    })
}

fn order(status: u8, amount: serde_json::Value) -> serde_json::Value {
    json!({
        "type": "success",
        "pages": 1,
        "orders": [{
            "merchant_order_id": PAYMENT.to_string(),
            "fk_order_id": 55501,
            "amount": amount,
            "currency": "RUB",
            "email": "client@example.com",
            "account": "220220******1234",
            "date": "2026-11-18 15:20:41",
            "status": status
        }]
    })
}

#[test]
fn подписи_freekassa_по_векторам() {
    assert_eq!(
        form_signature("7012", "100.11", "secret", "RUB", "154"),
        "64d0581f4a08af485a619950e023696a"
    );
    assert_eq!(
        form_signature("777", "199", "secret1", "RUB", "ord-42"),
        "f386853f1ce82b7d8b44cdc4ed962834"
    );
    assert_eq!(
        notice_signature("123", "100", "secret2", "test_order"),
        "30c76991785c43f10af2079878acd3d6"
    );
    assert_eq!(
        notice_signature("777", "199", "secret2", "ord-42"),
        "a48d6bacfc8ac5919aaf70bde85bd5d6"
    );
    assert_eq!(
        api_signature("test_api_key", &[("shopId", "777"), ("nonce", "123456789")]),
        "d3ee260f77acc01f3a58e0fae3966d2c18f47af28980d7ad51e5caa916e4c5d8"
    );
    assert_eq!(
        api_signature(
            "test_api_key",
            &[
                ("shopId", "777"),
                ("nonce", "123456789"),
                ("paymentId", "987654321"),
                ("i", "6"),
                ("email", "user@site.ru"),
                ("ip", "85.8.8.8"),
                ("amount", "100.23"),
                ("currency", "RUB"),
            ]
        ),
        "c7dbd85553e0cd74a9787b99e9fee2bb86c65f21da2d320fc7c447550ccab179"
    );
    assert_eq!(
        api_signature(
            "test_api_key",
            &[
                ("amount", "199"),
                ("currency", "RUB"),
                ("email", "12345@telegram.org"),
                ("i", "44"),
                ("ip", "203.0.113.10"),
                ("nonce", "1795000000000000000"),
                ("paymentId", "ord-42"),
                ("shopId", "777"),
            ]
        ),
        "c770eff42677f795c4a2a2a65cc58f40949a8369a9b8de0fb34131cd7dc00aad"
    );
}

#[test]
fn сумма_уходит_без_лишних_нулей() {
    assert_eq!(amount_text(rub(19_900)), "199");
    assert_eq!(amount_text(rub(19_950)), "199.5");
    assert_eq!(amount_text(rub(10_011)), "100.11");
    assert_eq!(amount_text(rub(10)), "0.1");
    assert_eq!(amount_text(rub(1)), "0.01");
    assert_eq!(rubles("1500"), Ok(rub(150_000)));
    assert_eq!(rubles("1500.00"), Ok(rub(150_000)));
    assert_eq!(rubles("1500.5"), Ok(rub(150_050)));
    assert_eq!(rubles("1500.500000"), Ok(rub(150_050)));
    assert!(rubles("1500.004").is_err());
    assert!(rubles("-1").is_err());
    assert!(rubles("").is_err());
}

#[test]
fn номер_запроса_всегда_растёт() {
    let mut last = 0;
    for _ in 0..10_000 {
        let next = nonce();
        assert!(next > last);
        last = next;
    }
    assert!(last > 1_700_000_000_000_000_000, "{last}");
}

#[tokio::test]
async fn ссылка_на_форму_freekassa() {
    let link = Freekassa::new()
        .create_invoice(&full(), &invoice(rub(150_000), Some("client@example.com")))
        .await
        .unwrap();
    assert!(
        link.pay_url.starts_with("https://pay.fk.money/?"),
        "{}",
        link.pay_url
    );
    let params = query(&link.pay_url);
    let order = PAYMENT.to_string();
    assert_eq!(params["m"], SHOP_ID);
    assert_eq!(params["oa"], "1500");
    assert_eq!(params["currency"], "RUB");
    assert_eq!(params["o"], order);
    assert_eq!(
        params["s"],
        form_signature(SHOP_ID, "1500", WORD1, "RUB", &order)
    );
    assert_eq!(params["lang"], "ru");
    assert_eq!(params["em"], "client@example.com");
    assert!(!params.contains_key("i"));
    assert_eq!(link.external_id, None);

    let mut page = invoice(rub(49_950), None);
    page.method = Some("page".into());
    let link = Freekassa::new()
        .create_invoice(&full(), &page)
        .await
        .unwrap();
    let params = query(&link.pay_url);
    assert_eq!(params["oa"], "499.5");
    assert!(!params.contains_key("em"));
    assert_eq!(
        Freekassa::new().create_invoice(&settings(), &page).await,
        Err(PaymentError::NotConfigured(NO_SECRET1.into()))
    );
}

#[tokio::test]
async fn заказ_через_api_freekassa() {
    let server = api("/orders/create", created()).await;
    let provider = Freekassa::with_api_base(server.uri());
    let link = provider
        .create_invoice(
            &settings(),
            &invoice(rub(150_000), Some("client@example.com")),
        )
        .await
        .unwrap();
    assert_eq!(
        link.pay_url,
        format!("https://pay.freekassa.net/form/55501/{HASH}")
    );
    assert_eq!(link.external_id, None);
    assert_eq!(link.payload["orderId"], "55501");
    let sent = bodies(&server.received_requests().await.unwrap(), "/orders/create");
    let body = &sent[0];
    assert_eq!(body["shopId"], 777);
    assert!(body["nonce"].as_i64().unwrap() > 1_700_000_000_000_000_000);
    assert_eq!(body["i"], 44);
    assert_eq!(body["amount"], "1500");
    assert_eq!(body["currency"], "RUB");
    assert_eq!(body["email"], "client@example.com");
    assert_eq!(body["ip"], SERVER);
    assert_eq!(body["paymentId"], PAYMENT.to_string());
    assert!(signed_by_key(body), "{body}");

    for (code, channel) in [("sbp", 44), ("card", 36), ("sberpay", 43)] {
        let server = api("/orders/create", created()).await;
        let mut request = invoice(rub(49_900), None);
        request.method = Some(code.into());
        Freekassa::with_api_base(server.uri())
            .create_invoice(&full(), &request)
            .await
            .unwrap();
        let sent = bodies(&server.received_requests().await.unwrap(), "/orders/create");
        assert_eq!(sent[0]["i"], channel, "{code}");
        assert_eq!(sent[0]["amount"], "499");
        assert_eq!(sent[0]["email"], "5093118422@telegram.org");
    }
    let mut unknown = invoice(rub(49_900), None);
    unknown.method = Some("crypto".into());
    assert_eq!(
        provider.create_invoice(&full(), &unknown).await,
        Err(PaymentError::Rejected(
            "этот способ оплаты недоступен".into()
        ))
    );
    assert!(matches!(
        provider
            .create_invoice(&settings(), &invoice(usd(1000), None))
            .await,
        Err(PaymentError::UnsupportedCurrency(_))
    ));
}

#[tokio::test]
async fn без_ip_сервера_заказ_через_api_не_создаётся() {
    let server = api("/orders/create", created()).await;
    let mut no_ip = settings();
    no_ip.0.remove("server_ip");
    let mut request = invoice(rub(49_900), None);
    request.webhook_url = "https://127.0.0.1/api/webhooks/pay/freekassa".into();
    assert_eq!(
        Freekassa::with_api_base(server.uri())
            .create_invoice(&no_ip, &request)
            .await,
        Err(PaymentError::NotConfigured(NO_IP.into()))
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    for ip in [
        "10.0.0.1",
        "127.0.0.1",
        "100.64.0.1",
        "192.0.2.1",
        "::1",
        "fd00::1",
        "fe80::1",
    ] {
        assert!(!public(&ip.parse().unwrap()), "{ip}");
    }
    for ip in ["203.0.114.1", "8.8.8.8", "2a00:1450:4010::1"] {
        assert!(public(&ip.parse().unwrap()), "{ip}");
    }
    let mut client = invoice(rub(49_900), None);
    client.client.telegram_id = None;
    assert_eq!(buyer_email(&client), "client7@panel.example.com");
}

#[tokio::test]
async fn ответы_api_freekassa_проверяются() {
    let insecure = api(
        "/orders/create",
        json!({"type": "success", "orderId": 1, "location": "http://pay.freekassa.net/form/1/x"}),
    )
    .await;
    assert!(matches!(
        Freekassa::with_api_base(insecure.uri())
            .create_invoice(&settings(), &invoice(rub(49_900), None))
            .await,
        Err(PaymentError::BadPayload(_))
    ));
    let refused = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"type": "error", "message": "Wrong signature", "data": []})),
        )
        .mount(&refused)
        .await;
    let err = Freekassa::with_api_base(refused.uri())
        .create_invoice(&settings(), &invoice(rub(49_900), None))
        .await
        .unwrap_err();
    assert!(util::keys_refused(&err), "{err:?}");
    let declined = api(
        "/orders/create",
        json!({"type": "error", "message": "Payment system is not available"}),
    )
    .await;
    assert_eq!(
        Freekassa::with_api_base(declined.uri())
            .create_invoice(&settings(), &invoice(rub(49_900), None))
            .await,
        Err(PaymentError::Rejected(
            "Payment system is not available".into()
        ))
    );
    let down = failing_api(502).await;
    assert_eq!(
        Freekassa::with_api_base(down.uri())
            .create_invoice(&settings(), &invoice(rub(49_900), None))
            .await,
        Err(PaymentError::Http { status: 502 })
    );
}

#[tokio::test]
async fn отказ_по_номеру_запроса_повторяется_с_новым_номером() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orders/create"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "type": "error",
            "message": "Request with same (or bigger) nonce already exist"
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/orders/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(created()))
        .with_priority(2)
        .mount(&server)
        .await;
    Freekassa::with_api_base(server.uri())
        .create_invoice(&settings(), &invoice(rub(49_900), None))
        .await
        .unwrap();
    let sent = bodies(&server.received_requests().await.unwrap(), "/orders/create");
    assert_eq!(sent.len(), 2);
    assert!(sent[1]["nonce"].as_i64() > sent[0]["nonce"].as_i64());

    let stuck = api(
        "/orders/create",
        json!({"type": "error", "message": "Request with same (or bigger) nonce already exist"}),
    )
    .await;
    let Err(PaymentError::Rejected(reason)) = Freekassa::with_api_base(stuck.uri())
        .create_invoice(&settings(), &invoice(rub(49_900), None))
        .await
    else {
        panic!("ожидался отказ");
    };
    assert!(reason.contains("nonce"), "{reason}");
}

#[tokio::test]
async fn оповещение_с_верной_подписью_засчитывает_оплату() {
    let order = PAYMENT.to_string();
    let outcome = parse(&settings(), notice("1500", "1500", &order, WORD2))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.event_key, "paid:55501");
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some("55501"));
    assert_eq!(
        outcome.reply,
        WebhookReply::Text {
            status: 200,
            body: "YES".into()
        }
    );
    for signed_amount in ["1500.00", "1500"] {
        let outcome = parse(&settings(), notice("1500.00", signed_amount, &order, WORD2))
            .await
            .unwrap();
        assert_eq!(outcome.amount, Some(rub(150_000)), "{signed_amount}");
    }
    let again = parse(&settings(), notice("1500", "1500", &order, WORD2))
        .await
        .unwrap();
    assert_eq!(again.event_key, outcome.event_key);
    let foreign = parse(&settings(), notice("1500", "1500", "old-42", WORD2))
        .await
        .unwrap();
    assert_eq!(
        foreign.payment,
        Some(PaymentRef::ProviderTxid("55501".into()))
    );
}

#[tokio::test]
async fn подпись_и_магазин_оповещения_проверяются() {
    let order = PAYMENT.to_string();
    assert_eq!(
        parse(
            &settings(),
            format!("MERCHANT_ID=777&AMOUNT=1500&MERCHANT_ORDER_ID={order}")
        )
        .await,
        Err(PaymentError::BadSignature)
    );
    assert_eq!(
        parse(&settings(), notice("1500", "1500", &order, WORD1)).await,
        Err(PaymentError::BadSignature)
    );
    let forged = notice("1500", "1500", &order, WORD2).replace("AMOUNT=1500", "AMOUNT=15000");
    assert_eq!(
        parse(&settings(), forged).await,
        Err(PaymentError::BadSignature)
    );
    let other_shop = with(settings(), &[("shop_id", json!("778"))]);
    assert!(matches!(
        parse(&other_shop, notice("1500", "1500", &order, WORD2)).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn оповещение_формой_multipart_и_строкой_запроса() {
    let order = PAYMENT.to_string();
    let sign = notice_signature(SHOP_ID, "1500", WORD2, &order);
    let boundary = "------------------------d74496d66958873e";
    let fields = [
        ("MERCHANT_ID", SHOP_ID),
        ("AMOUNT", "1500"),
        ("intid", "55501"),
        ("MERCHANT_ORDER_ID", order.as_str()),
        ("SIGN", sign.as_str()),
    ];
    let mut body = String::new();
    for (name, value) in fields {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let request = hook(&[("Content-Type", content_type.as_str())], body.clone());
    let outcome = Freekassa::new()
        .parse_webhook(&settings(), &request)
        .await
        .unwrap();
    assert_eq!(outcome.amount, Some(rub(150_000)));
    let doubled = body.replace("name=\"intid\"", "name=\"AMOUNT\"");
    assert!(
        Freekassa::new()
            .parse_webhook(&settings(), &hook(&[], doubled))
            .await
            .is_err()
    );

    let query = notice("1500", "1500", &order, WORD2);
    let mut get = hook(&[], query.clone());
    get.query = Some(query);
    assert_eq!(
        Freekassa::new()
            .parse_webhook(&settings(), &get)
            .await
            .unwrap()
            .status,
        ProviderStatus::Paid
    );
}

#[tokio::test]
async fn сумма_оповещения_не_сошлась() {
    let order = PAYMENT.to_string();
    let outcome = parse(&settings(), notice("1499", "1499", &order, WORD2))
        .await
        .unwrap();
    assert!(!check_paid_amount(rub(150_000), outcome.amount).accepts());
    assert!(matches!(
        parse(&settings(), notice("1500.004", "1500.004", &order, WORD2)).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn проверка_адреса_отправителя_freekassa() {
    let order = PAYMENT.to_string();
    let send = |config: ProviderConfig, ip: Option<&str>| {
        let mut request = hook(&[], notice("1500", "1500", &order, WORD2));
        request.remote_ip = ip.map(|ip| ip.parse().unwrap());
        async move { Freekassa::new().parse_webhook(&config, &request).await }
    };
    let strict = with(settings(), &[("check_ip", json!(true))]);
    for ip in ["168.119.157.136", "51.250.54.238", "::ffff:178.154.197.79"] {
        assert!(send(strict.clone(), Some(ip)).await.is_ok(), "{ip}");
    }
    for ip in [Some("10.0.0.1"), None] {
        assert!(
            matches!(
                send(strict.clone(), ip).await,
                Err(PaymentError::BadPayload(_))
            ),
            "{ip:?}"
        );
    }
    let custom = with(
        strict.clone(),
        &[("sender_ips", json!("203.0.113.5, 203.0.113.6"))],
    );
    assert!(send(custom.clone(), Some("203.0.113.6")).await.is_ok());
    assert!(send(custom, Some("168.119.157.136")).await.is_err());
    assert!(send(settings(), Some("10.0.0.1")).await.is_ok());
    assert!(
        Freekassa::new()
            .validate_settings(&with(settings(), &[("sender_ips", json!("1.2.3.4, nope"))]))
            .is_err()
    );
}

#[tokio::test]
async fn опрос_статуса_freekassa() {
    let server = api("/orders", order(1, json!(1500))).await;
    let provider = Freekassa::with_api_base(server.uri());
    let outcome = provider
        .fetch_status(&settings(), &PAYMENT.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.provider_txid.as_deref(), Some("55501"));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    let sent = bodies(&server.received_requests().await.unwrap(), "/orders");
    assert_eq!(sent[0]["paymentId"], PAYMENT.to_string());
    assert!(signed_by_key(&sent[0]));

    for (status, expected) in [
        (0, ProviderStatus::Pending),
        (6, ProviderStatus::Refunded),
        (8, ProviderStatus::Failed),
        (9, ProviderStatus::Canceled),
    ] {
        let server = api("/orders", order(status, json!("1500.00"))).await;
        let outcome = Freekassa::with_api_base(server.uri())
            .fetch_status(&settings(), &PAYMENT.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, expected, "{status}");
    }

    let empty = api(
        "/orders",
        json!({"type": "success", "pages": 0, "orders": []}),
    )
    .await;
    let outcome = Freekassa::with_api_base(empty.uri())
        .fetch_status(&settings(), &PAYMENT.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    assert_eq!(outcome.provider_txid, None);
    assert_eq!(
        Freekassa::with_api_base(empty.uri())
            .fetch_status(&settings(), "55501")
            .await,
        Err(PaymentError::Rejected("заказ не найден в Freekassa".into()))
    );
    let by_number = Freekassa::with_api_base(server.uri())
        .fetch_status(&settings(), "55501")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_number.provider_txid.as_deref(), Some("55501"));
    let sent = bodies(&server.received_requests().await.unwrap(), "/orders");
    assert_eq!(sent.last().unwrap()["orderId"], 55501);
}

fn refund_request(amount: Amount, key: u128) -> RefundRequest {
    RefundRequest {
        payment_id: sn_domain::ids::PaymentId(41),
        provider_txid: "55501".into(),
        amount,
        reason: None,
        key: Uuid::from_u128(key),
        email: None,
        description: None,
    }
}

async fn refund_api(status: u8) -> MockServer {
    let server = api("/orders", order(status, json!(1500))).await;
    Mock::given(method("POST"))
        .and(path("/orders/refund"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"type": "success", "id": 77})),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn возврат_freekassa() {
    let server = refund_api(1).await;
    let provider = Freekassa::with_api_base(server.uri());
    let full_refund = provider
        .refund(&settings(), &refund_request(rub(150_000), 1))
        .await
        .unwrap();
    assert_eq!(
        full_refund,
        RefundOutcome {
            provider_refund_id: Some("55501:77".into()),
            completed: false
        }
    );
    assert_eq!(
        provider
            .refund(&settings(), &refund_request(rub(150_000), 1))
            .await
            .unwrap(),
        full_refund
    );
    let sent = bodies(&server.received_requests().await.unwrap(), "/orders/refund");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["orderId"], 55501);
    assert!(sent[0].get("orderAmount").is_none());
    assert!(signed_by_key(&sent[0]));

    provider
        .refund(&settings(), &refund_request(rub(50_050), 2))
        .await
        .unwrap();
    let sent = bodies(&server.received_requests().await.unwrap(), "/orders/refund");
    assert_eq!(sent[1]["orderAmount"], "500.5");

    for status in [0, 6] {
        let server = refund_api(status).await;
        assert!(matches!(
            Freekassa::with_api_base(server.uri())
                .refund(&settings(), &refund_request(rub(150_000), 3))
                .await,
            Err(PaymentError::Rejected(_))
        ));
        assert!(
            bodies(&server.received_requests().await.unwrap(), "/orders/refund").is_empty(),
            "{status}"
        );
    }
}

#[tokio::test]
async fn опрос_возврата_freekassa() {
    let done = api("/orders", order(6, json!(1500))).await;
    let outcome = Freekassa::with_api_base(done.uri())
        .fetch_refund(&settings(), "55501:77")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Refunded);
    assert_eq!(outcome.event_key, "refund:55501:77");
    let waiting = api("/orders", order(1, json!(1500))).await;
    let outcome = Freekassa::with_api_base(waiting.uri())
        .fetch_refund(&settings(), "55501:77")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    assert_eq!(
        Freekassa::new().fetch_refund(&settings(), "refund-1").await,
        Ok(None)
    );
}

async fn check_api(shops: serde_json::Value) -> MockServer {
    let server = api("/shops", shops).await;
    Mock::given(method("POST"))
        .and(path("/currencies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "success",
            "currencies": [
                {"id": 44, "name": "СБП (API)", "currency": "RUB", "is_enabled": 1, "is_favorite": 0},
                {"id": 36, "name": "Card RUB API", "currency": "RUB", "is_enabled": 0, "is_favorite": 0}
            ]
        })))
        .mount(&server)
        .await;
    server
}

async fn check(server: &MockServer, config: &ProviderConfig) -> ConnectionCheck {
    let context = CheckContext {
        webhook_url: Some("https://panel.example.com/api/webhooks/pay/freekassa".into()),
    };
    Freekassa::with_api_base(server.uri())
        .check_connection(config, &context)
        .await
}

#[tokio::test]
async fn проверка_подключения_freekassa() {
    let server = check_api(json!({"type": "success", "shops": [{"id": 777, "name": "VPN", "url": "https://vpn.example.com"}]})).await;
    let result = check(&server, &settings()).await;
    assert_eq!(result.status, CheckStatus::Ok, "{result:?}");
    let text = result.details.join("\n");
    assert!(text.contains("Карта, SberPay"), "{text}");
    assert!(text.contains(SERVER), "{text}");
    assert!(text.contains("Секретное слово 1 не задано"), "{text}");
    assert!(
        text.contains("https://panel.example.com/api/webhooks/pay/freekassa"),
        "{text}"
    );
    assert!(!serde_json::to_string(&result).unwrap().contains(KEY));
    let result = check(&server, &full()).await;
    assert!(
        !result
            .details
            .join("\n")
            .contains("Секретное слово 1 не задано")
    );

    let other =
        check_api(json!({"type": "success", "shops": [{"id": 900, "name": "Другой"}]})).await;
    let result = check(&other, &settings()).await;
    assert_eq!(result.status, CheckStatus::SettingsRejected);
    assert!(result.details.join("\n").contains("900"));

    let refused = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"type": "error", "message": "Wrong signature", "data": []})),
        )
        .mount(&refused)
        .await;
    assert_eq!(
        check(&refused, &settings()).await.status,
        CheckStatus::KeysRejected
    );
    let down = failing_api(503).await;
    assert_eq!(
        check(&down, &settings()).await.status,
        CheckStatus::Unreachable
    );
    let closed = Freekassa::with_api_base(closed_api())
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert_eq!(closed.status, CheckStatus::Unreachable);
}

#[test]
fn настройки_и_способы_freekassa() {
    let provider = Freekassa::new();
    assert!(provider.is_configured(&settings()));
    assert!(!provider.is_configured(&ProviderConfig::default()));
    for (key, value) in [
        ("shop_id", json!("shop-777")),
        ("api_key", json!("with space")),
        ("server_ip", json!("сервер")),
    ] {
        assert!(
            provider
                .validate_settings(&with(settings(), &[(key, value.clone())]))
                .is_err(),
            "{key}={value}"
        );
    }
    let schema = provider.settings_schema();
    let secrets: Vec<&str> = schema
        .iter()
        .filter(|f| f.is_secret())
        .map(|f| f.key)
        .collect();
    assert_eq!(secrets, ["api_key", "secret_word_2", "secret_word_1"]);
    let codes: Vec<&str> = provider
        .methods(Currency::Rub)
        .iter()
        .map(|m| m.code)
        .collect();
    assert_eq!(codes, ["sbp", "card", "sberpay", "page"]);
    assert!(provider.methods(Currency::Usd).is_empty());
    assert_eq!(
        provider.client_label(Currency::Rub, &settings()).title,
        "СБП"
    );
    assert_eq!(provider.client_label(Currency::Rub, &full()), PAGE_LABEL);
    let russian = |text: &str| {
        text.chars()
            .any(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё'))
    };
    for method in provider.methods(Currency::Rub) {
        for text in [method.title, method.hint] {
            if russian(text) {
                assert!(crate::english::text(text).is_some(), "{text}");
            }
        }
    }
    assert!(crate::english::text(PAGE_LABEL.hint).is_some());
}

#[tokio::test]
async fn из_нескольких_заказов_одного_платежа_берётся_оплаченный() {
    let two = api(
        "/orders",
        json!({
            "type": "success",
            "pages": 1,
            "orders": [
                {"merchant_order_id": PAYMENT.to_string(), "fk_order_id": 55500, "amount": 1500, "currency": "RUB", "status": 9},
                {"merchant_order_id": PAYMENT.to_string(), "fk_order_id": 55501, "amount": 1500, "currency": "RUB", "status": 1},
                {"merchant_order_id": PAYMENT.to_string(), "fk_order_id": 55502, "amount": 1500, "currency": "RUB", "status": 0}
            ]
        }),
    )
    .await;
    let outcome = Freekassa::with_api_base(two.uri())
        .fetch_status(&settings(), &PAYMENT.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.provider_txid.as_deref(), Some("55501"));
}
