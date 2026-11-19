use super::*;
use crate::test_support::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "trb-api-key-5c21";
const DOC_BODY: &str = "{\n  \"name\": \"new_digital_product\",\n  \"created_at\": \"2025-03-20T01:15:58.332Z\",\n  \"sent_at\": \"2025-03-20T01:15:58.542Z\",\n  \"payload\": {\n    \"product_id\": 456,\n    \"amount\": 500,\n    \"currency\": \"usd\",\n    \"user_id\": 31326,\n    \"telegram_user_id\": 12321321\n  }\n}";
const DOC_SIGNATURE: &str = "e750ce3d3bb2950b036372959e137b3f4e75b812c962c03405a1463e9693f9f7";
const FULL_BODY: &str = r#"{"name":"new_digital_product","created_at":"2025-03-20T01:15:58.33246Z","sent_at":"2025-03-20T01:15:58.542279448Z","payload":{"product_id":456,"product_name":"VPN Access - 1 Month","amount":500,"currency":"usd","user_id":31326,"trb_user_id":"T-31326","telegram_user_id":12321321,"telegram_username":"durov","purchase_id":78901,"transaction_id":234567,"purchase_created_at":"2025-03-20T01:15:58.33246Z"}}"#;
const FULL_SIGNATURE: &str = "520c7ee1861adb723d8fa9c4a3c648ce5fb2d6f2ff7133ec8915792aaf077329";

fn settings() -> ProviderConfig {
    config(&[
        ("api_key", json!(KEY)),
        (
            "products",
            json!([
                {"id": 11, "name": "Пополнение 300 ₽", "amount": 30_000, "currency": "RUB", "link": "https://t.me/tribute/app?startapp=p11"},
                {"id": 12, "name": "Пополнение 500 ₽", "amount": 50_000, "currency": "RUB", "link": "https://t.me/tribute/app?startapp=p12"},
                {"id": 13, "name": "Top up $5", "amount": 500, "currency": "USD", "link": "https://t.me/tribute/app?startapp=p13"},
                {"id": 14, "name": "Пополнение 300 ₽ ещё раз", "amount": 30_000, "currency": "RUB", "link": "https://t.me/tribute/app?startapp=p14"}
            ]),
        ),
        (
            "plans",
            json!([{"period": 1547, "subscription": 1644, "name": "VPN · месяц", "tariff": 3, "tariff_period": 7}]),
        ),
    ])
}

fn signed(body: &str) -> WebhookRequest {
    hook(
        &[("trbt-signature", &signature(KEY, body.as_bytes()))],
        body.to_string(),
    )
}

async fn parse(request: WebhookRequest) -> PaymentResult<WebhookOutcome> {
    Tribute::new().parse_webhook(&settings(), &request).await
}

fn with_documented_product() -> ProviderConfig {
    let mut current = settings();
    let mut products = current.0["products"].as_array().unwrap().clone();
    products.push(json!({
        "id": 456, "name": "VPN Access - 1 Month", "amount": 500, "currency": "USD",
        "link": "https://t.me/tribute/app?startapp=p456"
    }));
    current.set("products", json!(products));
    current
}

async fn parse_documented(request: WebhookRequest) -> PaymentResult<WebhookOutcome> {
    Tribute::new()
        .parse_webhook(&with_documented_product(), &request)
        .await
}

fn purchase_body(payload: serde_json::Value) -> String {
    json!({
        "name": "new_digital_product",
        "created_at": "2026-11-18T10:00:00.1Z",
        "sent_at": "2026-11-18T10:00:00.2Z",
        "payload": payload
    })
    .to_string()
}

fn subscription_body(name: &str, expires: &str, telegram: Option<i64>) -> String {
    let mut payload = json!({
        "subscription_name": "VPN", "subscription_id": 1644, "period_id": 1547, "period": "monthly",
        "type": "regular", "price": 19_900, "amount": 17_910, "currency": "rub", "user_id": 31326,
        "trb_user_id": "T-31326", "channel_id": 614, "channel_name": "vpn", "expires_at": expires
    });
    if let Some(id) = telegram {
        payload["telegram_user_id"] = json!(id);
    }
    json!({
        "name": name,
        "created_at": "2026-11-18T10:00:00Z",
        "sent_at": "2026-11-18T10:00:01Z",
        "payload": payload
    })
    .to_string()
}

#[test]
fn подпись_совпадает_с_эталоном() {
    assert_eq!(signature(KEY, DOC_BODY.as_bytes()), DOC_SIGNATURE);
    assert_eq!(signature(KEY, FULL_BODY.as_bytes()), FULL_SIGNATURE);
}

#[tokio::test]
async fn покупка_из_документации_пополняет_баланс_покупателя() {
    let outcome = parse_documented(hook(&[("trbt-signature", FULL_SIGNATURE)], FULL_BODY))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(usd(500)));
    assert_eq!(outcome.event_key, "purchase:78901");
    assert_eq!(outcome.provider_txid.as_deref(), Some("purchase:78901"));
    assert_eq!(
        outcome.payment,
        Some(PaymentRef::Buyer(Buyer {
            telegram_id: 12_321_321,
            txid: "purchase:78901".into(),
            purpose: BuyerPurpose::TopUp,
        }))
    );
    assert_eq!(
        outcome.reply,
        WebhookReply::Json {
            status: 200,
            body: json!({"status": "ok"})
        }
    );
    let upper = hook(
        &[(
            "Trbt-Signature",
            FULL_SIGNATURE.to_ascii_uppercase().as_str(),
        )],
        FULL_BODY,
    );
    assert_eq!(
        parse_documented(upper).await.unwrap().status,
        ProviderStatus::Paid
    );
    let short = parse_documented(hook(&[("trbt-signature", DOC_SIGNATURE)], DOC_BODY)).await;
    assert!(
        matches!(short, Err(PaymentError::BadPayload(_))),
        "{short:?}"
    );
}

#[tokio::test]
async fn покупка_товара_не_для_пополнения_ждёт_решения() {
    let outcome = parse(hook(&[("trbt-signature", FULL_SIGNATURE)], FULL_BODY))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    assert_eq!(outcome.payment, None);
    assert_eq!(outcome.event_key, "purchase:78901");
    assert_eq!(outcome.amount, Some(usd(500)));
    let notice = outcome.notice().unwrap();
    assert_eq!(notice.kind, NoticeKind::ManualCredit);
    for needle in ["№456", "78901", "12321321", "товаров для пополнения"] {
        assert!(notice.text.contains(needle), "{needle}: {}", notice.text);
    }
    let anonymous = purchase_body(json!({
        "product_id": 99, "product_name": "Книга", "amount": 30_000, "currency": "rub",
        "purchase_id": 904, "transaction_id": 7, "purchase_created_at": "2026-11-18T10:00:00Z"
    }));
    let outcome = parse(signed(&anonymous)).await.unwrap();
    assert_eq!(outcome.payment, None);
    assert_eq!(outcome.status, ProviderStatus::Pending);
    let notice = outcome.notice().unwrap();
    assert_eq!(notice.kind, NoticeKind::ManualCredit);
    assert!(
        notice.text.contains("без Telegram ID покупателя")
            && notice.text.contains("товаров для пополнения"),
        "{}",
        notice.text
    );
    let bare = config(&[("api_key", json!(KEY))]);
    let outcome = Tribute::new()
        .parse_webhook(&bare, &signed(&purchase_body(json!({
            "product_id": 12, "product_name": "Пополнение 500 ₽", "amount": 50_000, "currency": "rub",
            "telegram_user_id": 5_093_118_422_i64, "purchase_id": 905, "transaction_id": 8,
            "purchase_created_at": "2026-11-18T10:00:00Z"
        }))))
        .await
        .unwrap();
    assert_eq!(outcome.payment, None);
    assert_eq!(
        outcome.notice().map(|n| n.kind),
        Some(NoticeKind::ManualCredit)
    );
}

#[tokio::test]
async fn без_верной_подписи_уведомление_отклоняется() {
    assert_eq!(
        parse(hook(&[], FULL_BODY)).await,
        Err(PaymentError::BadSignature)
    );
    assert_eq!(
        parse(hook(
            &[("trbt-signature", &signature("other", FULL_BODY.as_bytes()))],
            FULL_BODY
        ))
        .await,
        Err(PaymentError::BadSignature)
    );
    let tampered = FULL_BODY.replace("\"amount\":500", "\"amount\":50000");
    assert_eq!(
        parse(hook(&[("trbt-signature", FULL_SIGNATURE)], tampered)).await,
        Err(PaymentError::BadSignature)
    );
    let spaced = format!("{FULL_BODY} ");
    assert_eq!(
        parse(hook(&[("trbt-signature", FULL_SIGNATURE)], spaced)).await,
        Err(PaymentError::BadSignature)
    );
    assert_eq!(
        Tribute::new()
            .parse_webhook(&ProviderConfig::default(), &signed(FULL_BODY))
            .await,
        Err(PaymentError::NotConfigured("не задано поле api_key".into()))
    );
}

#[tokio::test]
async fn повтор_покупки_даёт_тот_же_ключ() {
    let body = purchase_body(json!({
        "product_id": 12, "product_name": "Пополнение 500 ₽", "amount": 50_000, "currency": "rub",
        "telegram_user_id": 5_093_118_422_i64, "purchase_id": 901, "transaction_id": 4,
        "purchase_created_at": "2026-11-18T10:00:00Z"
    }));
    let first = parse(signed(&body)).await.unwrap();
    assert_eq!(first.amount, Some(rub(50_000)));
    let resent = body.replace("10:00:00.2Z", "10:05:00.9Z");
    assert_ne!(resent, body);
    let again = parse(signed(&resent)).await.unwrap();
    assert_eq!(again.event_key, first.event_key);
    assert_eq!(again.payment, first.payment);
}

#[tokio::test]
async fn покупка_без_telegram_id_или_в_евро_уходит_админу() {
    let web = purchase_body(json!({
        "product_id": 13, "product_name": "Пополнение", "amount": 1000, "currency": "usd",
        "trb_user_id": "W-15408", "purchase_id": 902, "transaction_id": 5,
        "purchase_created_at": "2026-11-18T10:00:00Z"
    }));
    let outcome = parse(signed(&web)).await.unwrap();
    assert_eq!(outcome.payment, None);
    assert_eq!(outcome.status, ProviderStatus::Pending);
    assert_eq!(outcome.amount, Some(usd(1000)));
    let notice = outcome.notice().unwrap();
    assert_eq!(notice.kind, NoticeKind::ManualCredit);
    assert!(notice.text.contains("без Telegram ID"), "{}", notice.text);
    assert!(notice.text.contains("902"), "{}", notice.text);
    let euro = purchase_body(json!({
        "product_id": 11, "product_name": "Пополнение", "amount": 1000, "currency": "eur",
        "telegram_user_id": 42, "purchase_id": 903, "transaction_id": 6,
        "purchase_created_at": "2026-11-18T10:00:00Z"
    }));
    let outcome = parse(signed(&euro)).await.unwrap();
    assert_eq!(outcome.payment, None);
    assert_eq!(outcome.amount, None);
    let notice = outcome.notice().unwrap();
    assert_eq!(notice.kind, NoticeKind::ManualCredit);
    assert!(notice.text.contains("10.00 EUR"), "{}", notice.text);
    for broken in [
        json!({"product_id": 1, "amount": 100, "currency": "usd", "telegram_user_id": 42}),
        json!({"purchase_id": 1, "amount": 100, "currency": "usd", "telegram_user_id": 42}),
        json!({"purchase_id": 1, "product_id": 1, "amount": 0, "currency": "usd", "telegram_user_id": 42}),
        json!({"purchase_id": 1, "product_id": 1, "amount": "сто", "currency": "usd", "telegram_user_id": 42}),
    ] {
        assert!(
            matches!(
                parse(signed(&purchase_body(broken.clone()))).await,
                Err(PaymentError::BadPayload(_))
            ),
            "{broken}"
        );
    }
}

#[tokio::test]
async fn подписка_ведёт_к_привязке_срока_и_каждое_продление_отдельно() {
    let first = parse(signed(&subscription_body(
        "new_subscription",
        "2026-12-18T10:00:00Z",
        Some(5_093_118_422),
    )))
    .await
    .unwrap();
    assert_eq!(first.status, ProviderStatus::Paid);
    assert_eq!(first.amount, Some(rub(19_900)));
    assert_eq!(
        first.payment,
        Some(PaymentRef::Buyer(Buyer {
            telegram_id: 5_093_118_422,
            txid: "sub:1644:5093118422:2026-12-18T10:00:00Z".into(),
            purpose: BuyerPurpose::Plan {
                plan: "1547".into(),
                series: "sub:1644:5093118422:".into(),
            },
        }))
    );
    let renewed = parse(signed(&subscription_body(
        "renewed_subscription",
        "2027-01-18T10:00:00Z",
        Some(5_093_118_422),
    )))
    .await
    .unwrap();
    assert_ne!(renewed.event_key, first.event_key);
    assert_eq!(
        renewed.provider_txid.as_deref(),
        Some("sub:1644:5093118422:2027-01-18T10:00:00Z")
    );
    let cancelled = parse(signed(&subscription_body(
        "cancelled_subscription",
        "2027-01-18T10:00:00Z",
        Some(5_093_118_422),
    )))
    .await
    .unwrap();
    assert_eq!(cancelled.payment, None);
    assert_eq!(cancelled.status, ProviderStatus::Pending);
    assert_eq!(cancelled.signal, None);
    let anonymous = parse(signed(&subscription_body(
        "new_subscription",
        "2026-12-18T10:00:00Z",
        None,
    )))
    .await
    .unwrap();
    assert_eq!(anonymous.payment, None);
    assert_eq!(
        anonymous.notice().map(|n| n.kind),
        Some(NoticeKind::ManualCredit)
    );
    let provider = Tribute::new();
    assert_eq!(
        provider.plan_binding(&settings(), "1547"),
        Some(PlanBinding {
            tariff_id: 3,
            period_id: 7
        })
    );
    assert_eq!(provider.plan_binding(&settings(), "1548"), None);
    assert_eq!(provider.plan_binding(&settings(), "abc"), None);
}

#[tokio::test]
async fn возврат_покупки_находит_платёж_по_номеру_покупки() {
    let body = json!({
        "name": "digital_product_refunded",
        "created_at": "2026-11-18T11:00:00Z",
        "sent_at": "2026-11-18T11:00:01Z",
        "payload": {
            "product_id": 12, "product_name": "Пополнение 500 ₽", "amount": 50_000, "currency": "rub",
            "telegram_user_id": 5_093_118_422_i64, "purchase_id": 901, "transaction_id": 4,
            "refund_reason": "telegram_refund", "refunded_at": "2026-11-18T11:00:00Z"
        }
    })
    .to_string();
    let outcome = parse(signed(&body)).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Refunded);
    assert_eq!(outcome.amount, Some(rub(50_000)));
    assert_eq!(outcome.event_key, "refund:901");
    assert_eq!(
        outcome.payment,
        Some(PaymentRef::ProviderTxid("purchase:901".into()))
    );
}

#[tokio::test]
async fn донаты_и_прочие_события_не_засчитываются() {
    for name in [
        "new_donation",
        "recurrent_donation",
        "cancelled_donation",
        "physical_order_created",
        "physical_order_shipped",
        "",
    ] {
        let body = json!({
            "name": name,
            "created_at": "2026-11-18T12:00:00Z",
            "sent_at": "2026-11-18T12:00:00Z",
            "payload": {"telegram_user_id": 42, "amount": 1000, "currency": "rub", "donation_request_id": 5}
        })
        .to_string();
        let outcome = parse(signed(&body)).await.unwrap();
        assert_eq!(outcome.payment, None, "{name}");
        assert_eq!(outcome.status, ProviderStatus::Pending, "{name}");
        assert_eq!(outcome.signal, None, "{name}");
    }
    assert!(matches!(
        parse(signed("not json")).await,
        Err(PaymentError::BadPayload(_))
    ));
}

#[tokio::test]
async fn счёт_ведёт_на_самый_дешёвый_товар_не_меньше_суммы() {
    let provider = Tribute::new();
    let pick = |amount| {
        let provider = &provider;
        async move {
            provider
                .create_invoice(&settings(), &invoice(amount, None))
                .await
        }
    };
    let cheap = pick(rub(19_900)).await.unwrap();
    assert_eq!(cheap.pay_url, "https://t.me/tribute/app?startapp=p11");
    assert_eq!(cheap.external_id, None);
    assert_eq!(cheap.payload, json!({"product": 11, "charge": 30_000}));
    let exact = pick(rub(50_000)).await.unwrap();
    assert_eq!(exact.pay_url, "https://t.me/tribute/app?startapp=p12");
    assert_eq!(
        pick(rub(50_001)).await,
        Err(PaymentError::Rejected(
            "в Tribute нет товара на эту сумму".into()
        ))
    );
    assert_eq!(
        pick(usd(499)).await.unwrap().pay_url,
        "https://t.me/tribute/app?startapp=p13"
    );
    let mut anonymous = invoice(rub(19_900), Some("a@example.com"));
    anonymous.client.telegram_id = None;
    assert!(matches!(
        provider.create_invoice(&settings(), &anonymous).await,
        Err(PaymentError::Rejected(_))
    ));
}

#[test]
fn суммы_товаров_для_кнопок_пополнения() {
    let provider = Tribute::new();
    assert_eq!(
        provider.fixed_amounts(&settings(), Currency::Rub),
        Some(vec![
            Money::new(30_000, Currency::Rub),
            Money::new(50_000, Currency::Rub)
        ])
    );
    assert_eq!(
        provider.fixed_amounts(&settings(), Currency::Usd),
        Some(vec![Money::new(500, Currency::Usd)])
    );
    let bare = config(&[("api_key", json!(KEY))]);
    assert_eq!(provider.fixed_amounts(&bare, Currency::Rub), Some(vec![]));
    assert!(provider.needs_telegram());
}

#[test]
fn настройки_tribute_проверяются() {
    let provider = Tribute::new();
    assert!(provider.is_configured(&settings()));
    assert!(provider.is_configured(&config(&[("api_key", json!(KEY))])));
    assert!(!provider.is_configured(&ProviderConfig::default()));
    let mut twice = settings();
    twice.set(
        "products",
        json!([
            {"id": 11, "amount": 30_000, "currency": "RUB", "link": "https://t.me/tribute/app?startapp=p11"},
            {"id": 11, "amount": 30_000, "currency": "RUB", "link": "https://t.me/tribute/app?startapp=p11"}
        ]),
    );
    assert!(matches!(
        provider.validate_settings(&twice),
        Err(PaymentError::NotConfigured(_))
    ));
    let mut plain = settings();
    plain.set(
        "products",
        json!([{"id": 11, "amount": 30_000, "currency": "RUB", "link": "http://t.me/x"}]),
    );
    assert!(provider.validate_settings(&plain).is_err());
    let mut unbound = settings();
    unbound.set("plans", json!([{"period": 1547, "tariff": 3}]));
    assert!(provider.validate_settings(&unbound).is_err());
    let mut doubled = settings();
    doubled.set(
        "plans",
        json!([
            {"period": 1547, "tariff": 3, "tariff_period": 7},
            {"period": 1547, "tariff": 4, "tariff_period": 9}
        ]),
    );
    assert!(provider.validate_settings(&doubled).is_err());
    let mut text = settings();
    text.set("products", json!("11,12"));
    assert!(provider.validate_settings(&text).is_err());
    assert!(
        provider
            .settings_schema()
            .iter()
            .any(|field| field.key == "api_key" && field.is_secret())
    );
}

fn product_json(id: i64, amount: i64, currency: &str, status: &str) -> serde_json::Value {
    json!({
        "id": id, "type": "digital", "name": format!("Пополнение {id}"), "description": "",
        "amount": amount, "currency": currency, "starsAmount": 0, "starsAmountEnabled": false,
        "status": status, "isCustom": false, "acceptCards": true, "acceptWalletPay": true,
        "protectContent": false, "created": "2026-11-01T00:00:00Z", "updated": "2026-11-01T00:00:00Z",
        "link": format!("https://t.me/tribute/app?startapp=p{id}"),
        "webLink": format!("https://web.tribute.tg/p/{id}")
    })
}

async fn catalog_api() -> MockServer {
    let server = MockServer::start().await;
    let first: Vec<serde_json::Value> = (1..=100)
        .map(|id| product_json(id, 30_000, "RUB", "approved"))
        .collect();
    Mock::given(method("GET"))
        .and(path("/products"))
        .and(header("Api-Key", KEY))
        .and(query_param("type", "digital"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "rows": first, "meta": {"total": 104, "offset": 1, "limit": 100}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/products"))
        .and(header("Api-Key", KEY))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "rows": [
                product_json(101, 50_000, "rub", "approved"),
                product_json(102, 1_000, "usd", "approved"),
                product_json(103, 10_000, "RUB", "pending"),
                product_json(104, 900, "EUR", "approved")
            ],
            "meta": {"total": 104, "offset": 2, "limit": 100}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/subscriptions"))
        .and(header("Api-Key", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": [{
                "subscriptionId": 1644, "name": "VPN", "currency": "rub",
                "periods": [
                    {"periodId": 1547, "period": "monthly", "price": 199},
                    {"periodId": 1548, "period": "quarterly", "price": 549.5}
                ]
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "no_access", "message": "invalid api key"
        })))
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn каталог_товаров_и_подписок_для_настроек() {
    let server = catalog_api().await;
    let catalog = Tribute::with_api_base(server.uri())
        .catalog(&settings())
        .await
        .unwrap()
        .unwrap();
    let products = catalog["products"].as_array().unwrap();
    assert_eq!(products.len(), 102);
    assert_eq!(
        products[100],
        json!({"id": 101, "name": "Пополнение 101", "amount": 50_000, "currency": "RUB", "link": "https://t.me/tribute/app?startapp=p101"})
    );
    assert!(
        !products
            .iter()
            .any(|p| p["id"] == json!(103) || p["id"] == json!(104))
    );
    assert_eq!(
        catalog["subscriptions"],
        json!([{
            "id": 1644, "name": "VPN", "currency": "RUB",
            "periods": [
                {"id": 1547, "period": "monthly", "price": 19_900},
                {"id": 1548, "period": "quarterly", "price": 54_950}
            ]
        }])
    );
}

#[tokio::test]
async fn проверка_подключения_tribute() {
    let server = catalog_api().await;
    let provider = Tribute::with_api_base(server.uri());
    let context = CheckContext {
        webhook_url: Some("https://panel.example.com/api/webhooks/pay/tribute".into()),
    };
    let mut current = settings();
    current.set(
        "products",
        json!([
            {"id": 101, "amount": 50_000, "currency": "RUB", "link": "https://t.me/tribute/app?startapp=p101"},
            {"id": 555, "amount": 70_000, "currency": "RUB", "link": "https://t.me/tribute/app?startapp=p555"}
        ]),
    );
    let ok = provider.check_connection(&current, &context).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
    let details = ok.details.join("\n");
    assert!(details.contains("Товаров для пополнения: 2"), "{details}");
    assert!(details.contains("№555"), "{details}");
    assert!(!details.contains("№101"), "{details}");
    assert!(
        details.contains("Подписок в Tribute: 1, привязано сроков: 1"),
        "{details}"
    );
    assert!(
        details.contains("https://panel.example.com/api/webhooks/pay/tribute"),
        "{details}"
    );
    let wrong = provider
        .check_connection(&config(&[("api_key", json!("wrong-key"))]), &context)
        .await;
    assert_eq!(wrong.status, CheckStatus::KeysRejected);
    assert!(!format!("{wrong:?}").contains("wrong-key"));
    let unreachable = Tribute::with_api_base(closed_api())
        .check_connection(&settings(), &context)
        .await;
    assert_eq!(unreachable.status, CheckStatus::Unreachable);
    let empty = provider
        .check_connection(&ProviderConfig::default(), &context)
        .await;
    assert_eq!(empty.status, CheckStatus::SettingsRejected);
}
