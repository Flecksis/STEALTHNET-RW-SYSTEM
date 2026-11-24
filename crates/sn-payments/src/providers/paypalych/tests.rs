use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::ids::PaymentId;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use uuid::Uuid;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DOC_TOKEN: &str = "72|oBCB7Z3SmUm1gvkpEdRcSR2q1ERHpG4vD3DNBmuT";
const TOKEN: &str = "15|pX9kL2mN4qR6sT8vW0yZ1aB3cD5eF7gH";
const SHOP: &str = "LXZv3R7Q8B";
const BILL: &str = "GkLWvKx3";
const HOOK: &str = "https://panel.example.com/api/webhooks/pay/pal24";

fn settings() -> ProviderConfig {
    config(&[("api_token", json!(TOKEN)), ("shop_id", json!(SHOP))])
}

fn bill(status: &str, amount: serde_json::Value, currency: &str) -> serde_json::Value {
    json!({
        "id": BILL, "order_id": PAYMENT.to_string(), "status": status, "type": "NORMAL",
        "amount": amount, "currency_in": currency, "created_at": "2026-11-18 12:00:00",
        "ttl": 3600, "success": true
    })
}

async fn api(status: &str, amount: serde_json::Value, currency: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/bill/status"))
        .and(query_param("id", BILL))
        .and(header("Authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(bill(status, amount, currency)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bill/status"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"success": false, "message": "api:error.bill_not_found"})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

fn postback(status: &str, sum: &str, trs: &str) -> WebhookRequest {
    let order = PAYMENT.to_string();
    let sign = signature(&[sum, &order], TOKEN);
    hook(
        &[("Content-Type", "application/x-www-form-urlencoded")],
        format!(
            "Status={status}&InvId={order}&Commission=0.00&CurrencyIn=RUB&OutSum={sum}&TrsId={trs}&AccountType=SBP&AccountNumber=79*****1234&BalanceAmount={sum}&BalanceCurrency=RUB&SignatureValue={sign}"
        ),
    )
}

async fn parse(server: &MockServer, request: &WebhookRequest) -> PaymentResult<WebhookOutcome> {
    PayPalych::with_api_base(server.uri())
        .parse_webhook(&settings(), request)
        .await
}

fn form_of(request: &wiremock::Request) -> Vec<(String, String)> {
    util::form_pairs(&request.body).unwrap()
}

#[test]
fn подпись_постбэков_по_векторам() {
    assert_eq!(
        signature(&["380.55", "payment-19238465"], DOC_TOKEN),
        "DC32D403CE6BD2E54D4D213278DB286C"
    );
    assert_ne!(
        signature(
            &["380.55", "payment-19238465"],
            "oBCB7Z3SmUm1gvkpEdRcSR2q1ERHpG4vD3DNBmuT"
        ),
        "DC32D403CE6BD2E54D4D213278DB286C"
    );
    assert_eq!(
        signature(&["18.54", "Заказ 123"], DOC_TOKEN),
        "6C05AE98A0DF7A122BFE5EF1A0A30959"
    );
    assert_eq!(
        signature(
            &["3150.00", "RUB", "KSvv3R7123", "QSvv3R7123", "LXZv3R7Q8B"],
            DOC_TOKEN
        ),
        "4D436304B89894888FB3A2A5BAC24801"
    );
    assert_eq!(
        signature(&["KSvv3R7123", "QSvv3R7123", "LXZv3R7Q8B"], DOC_TOKEN),
        "876C15C8A54D98A8166CA26609BFB74C"
    );
    assert_eq!(
        signature(&["350.00", "LXZv3R7Q8B"], DOC_TOKEN),
        "19BC5A469FB613FB258989183D9F1137"
    );
    assert!(signed(
        &["380.55", "payment-19238465"],
        DOC_TOKEN,
        Some(" dc32d403ce6bd2e54d4d213278db286c ")
    ));
    assert!(!signed(&["380.55", "payment-19238465"], DOC_TOKEN, None));
}

#[tokio::test]
async fn оплата_засчитывается_только_после_проверки_счёта() {
    let server = api("SUCCESS", json!(1500.0), "RUB").await;
    let outcome = parse(&server, &postback("SUCCESS", "1500.00", BILL))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.event_key, format!("{BILL}:paid"));
    assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(outcome.provider_txid.as_deref(), Some(BILL));
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.reply, WebhookReply::ok());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/bill/status");
}

#[tokio::test]
async fn статус_берётся_из_api_а_не_из_постбэка() {
    let pending = api("NEW", json!(1500), "RUB").await;
    let outcome = parse(&pending, &postback("SUCCESS", "1500.00", BILL))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    let paid = api("SUCCESS", json!(1500), "RUB").await;
    let outcome = parse(&paid, &postback("FAIL", "1500.00", BILL))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
}

#[tokio::test]
async fn счёт_ищется_по_номеру_заказа_если_trsid_не_номер_счёта() {
    let server = api("SUCCESS", json!(1500), "RUB").await;
    Mock::given(method("GET"))
        .and(path("/bill/search"))
        .and(query_param("order_id", PAYMENT.to_string().as_str()))
        .and(query_param("shop_id", SHOP))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": [
                {"id": "Other1", "order_id": "чужой", "status": "SUCCESS", "amount": 10, "currency_in": "RUB"},
                bill("SUCCESS", json!(1500), "RUB")
            ]
        })))
        .mount(&server)
        .await;
    let outcome = parse(&server, &postback("SUCCESS", "1500.00", "PaymentXYZ"))
        .await
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.provider_txid.as_deref(), Some(BILL));
}

#[tokio::test]
async fn чужой_счёт_не_засчитывается() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/bill/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": BILL, "order_id": "другой заказ", "status": "SUCCESS",
            "amount": 1500, "currency_in": "RUB", "success": true
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bill/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": []})),
        )
        .mount(&server)
        .await;
    assert_eq!(
        parse(&server, &postback("SUCCESS", "1500.00", BILL)).await,
        Err(PaymentError::BadSignature)
    );
}

#[tokio::test]
async fn без_подписи_или_с_чужой_подписью_постбэк_отклоняется() {
    let server = api("SUCCESS", json!(1500), "RUB").await;
    let order = PAYMENT.to_string();
    let forged = signature(&["1500.00", &order], "1|угаданный");
    let cases = [
        format!("Status=SUCCESS&InvId={order}&OutSum=1500.00&TrsId={BILL}"),
        format!("Status=SUCCESS&InvId={order}&OutSum=1500.00&TrsId={BILL}&SignatureValue={forged}"),
        format!(
            "Status=SUCCESS&InvId={order}&OutSum=15000.00&TrsId={BILL}&SignatureValue={}",
            signature(&["1500.00", &order], TOKEN)
        ),
        "Status=SUCCESS".to_string(),
    ];
    for body in cases {
        assert_eq!(
            parse(&server, &hook(&[], body.clone())).await,
            Err(PaymentError::BadSignature),
            "{body}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    let lower = format!(
        "Status=SUCCESS&InvId={order}&OutSum=1500.00&TrsId={BILL}&SignatureValue={}",
        signature(&["1500.00", &order], TOKEN).to_ascii_lowercase()
    );
    assert_eq!(
        parse(&server, &hook(&[], lower)).await.unwrap().status,
        ProviderStatus::Paid
    );
}

#[tokio::test]
async fn статусы_счёта_paypalych() {
    for (status, expected, key) in [
        ("SUCCESS", ProviderStatus::Paid, format!("{BILL}:paid")),
        ("OVERPAID", ProviderStatus::Paid, format!("{BILL}:paid")),
        ("FAIL", ProviderStatus::Failed, format!("{BILL}:fail")),
        ("NEW", ProviderStatus::Pending, format!("{BILL}:new")),
        (
            "PROCESS",
            ProviderStatus::Pending,
            format!("{BILL}:process"),
        ),
        (
            "UNDERPAID",
            ProviderStatus::Pending,
            format!("{BILL}:underpaid"),
        ),
    ] {
        let server = api(status, json!(1500), "RUB").await;
        let outcome = parse(&server, &postback("SUCCESS", "1500.00", BILL))
            .await
            .unwrap();
        assert_eq!(outcome.status, expected, "{status}");
        assert_eq!(outcome.event_key, key, "{status}");
        assert_eq!(
            outcome.notice().map(|n| n.kind),
            (status == "UNDERPAID").then_some(NoticeKind::ManualCredit),
            "{status}"
        );
    }
}

#[tokio::test]
async fn сумма_не_сошлась_и_чужая_валюта() {
    let expected = rub(150_000);
    let under = api("SUCCESS", json!(1499.99), "RUB").await;
    let outcome = parse(&under, &postback("SUCCESS", "1499.99", BILL))
        .await
        .unwrap();
    assert!(matches!(
        check_paid_amount(expected, outcome.amount),
        AmountCheck::Underpaid { .. }
    ));
    let dollars = api("SUCCESS", json!(1500), "USD").await;
    let outcome = parse(&dollars, &postback("SUCCESS", "1500.00", BILL))
        .await
        .unwrap();
    assert!(matches!(
        check_paid_amount(expected, outcome.amount),
        AmountCheck::CurrencyMismatch { .. }
    ));
}

#[tokio::test]
async fn повтор_постбэка_даёт_то_же_событие() {
    let server = api("SUCCESS", json!(1500), "RUB").await;
    let first = parse(&server, &postback("SUCCESS", "1500.00", BILL))
        .await
        .unwrap();
    let second = parse(&server, &postback("SUCCESS", "1500.00", BILL))
        .await
        .unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn p2p_сделки_пропускаются() {
    let server = MockServer::start().await;
    let sign = signature(&["350.00", "LXZv3R7Q8B"], TOKEN);
    let body = format!(
        "TrsId=LXZv3R7Q8B&InvId=deal-1&Status=SUCCESS&Amount=350.00&Currency=RUB&Payment%5BTrsId%5D=P1&SignatureValue={sign}"
    );
    let outcome = parse(&server, &hook(&[], body)).await.unwrap();
    assert_eq!(outcome.payment, None);
    assert_eq!(outcome.status, ProviderStatus::Pending);
    assert_eq!(outcome.event_key, "p2p:LXZv3R7Q8B");
    let forged = "TrsId=LXZv3R7Q8B&Amount=350.00&SignatureValue=00".to_string();
    assert_eq!(
        parse(&server, &hook(&[], forged)).await,
        Err(PaymentError::BadSignature)
    );
}

fn routed(kind: &str, body: String) -> WebhookRequest {
    let mut request = hook(&[], body);
    request.route_suffix = Some(kind.into());
    request
}

async fn refund_status_api(status: &str, amount: &str) -> MockServer {
    let server = api("SUCCESS", json!(1500), "RUB").await;
    Mock::given(method("GET"))
        .and(path("/refund/status"))
        .and(query_param("id", "R77"))
        .and(header("Authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "id": "R77", "status": status, "amount": amount, "currency": "RUB"
        })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn уведомления_о_возврате_и_чарджбэке() {
    let order = PAYMENT.to_string();
    let other = Uuid::from_u128(7).to_string();
    let refund_sign = signature(&["500.00", "RUB", BILL, "QSvv3R7123", "R77"], TOKEN);
    let refund = |status: &str, inv: &str, sign: &str| {
        routed(
            "refund",
            format!(
                "Id=R77&Amount=500.00&Currency=RUB&Status={status}&InvId={inv}&BillId={BILL}&PaymentId=QSvv3R7123&SignatureValue={sign}"
            ),
        )
    };
    let server = refund_status_api("SUCCESS", "500.00").await;
    let done = parse(&server, &refund("SUCCESS", &order, &refund_sign))
        .await
        .unwrap();
    assert_eq!(
        done.status,
        ProviderStatus::PartiallyRefunded {
            refunded: rub(50_000)
        }
    );
    assert_eq!(done.event_key, "refund:R77");
    assert_eq!(done.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(done.provider_txid.as_deref(), Some(BILL));
    assert!(matches!(
        parse(&server, &refund("SUCCESS", &other, &refund_sign)).await,
        Err(PaymentError::BadPayload(_))
    ));
    let processing = refund_status_api("PROCESS", "500.00").await;
    assert_eq!(
        parse(&processing, &refund("SUCCESS", &order, &refund_sign))
            .await
            .unwrap()
            .status,
        ProviderStatus::Pending
    );
    let declined = refund_status_api("FAIL", "500.00").await;
    assert_eq!(
        parse(&declined, &refund("SUCCESS", &order, &refund_sign))
            .await
            .unwrap()
            .status,
        ProviderStatus::RefundDeclined
    );
    let bigger = refund_status_api("SUCCESS", "1500.00").await;
    assert!(matches!(
        parse(&bigger, &refund("SUCCESS", &order, &refund_sign)).await,
        Err(PaymentError::BadPayload(_))
    ));
    let quiet = MockServer::start().await;
    assert_eq!(
        parse(&quiet, &refund("SUCCESS", &order, "00")).await,
        Err(PaymentError::BadSignature)
    );
    let chargeback_sign = signature(&[BILL, "QSvv3R7123", "C5"], TOKEN);
    let chargeback = |status: &str, inv: &str| {
        routed(
            "chargeback",
            format!(
                "Id=C5&Status={status}&InvId={inv}&BillId={BILL}&PaymentId=QSvv3R7123&SignatureValue={chargeback_sign}"
            ),
        )
    };
    let taken = parse(&server, &chargeback("SUCCESS", &order))
        .await
        .unwrap();
    assert_eq!(taken.status, ProviderStatus::Refunded);
    assert_eq!(taken.event_key, "chargeback:C5");
    assert_eq!(taken.payment, Some(PaymentRef::PublicId(PAYMENT)));
    assert_eq!(taken.notice().map(|n| n.kind), Some(NoticeKind::Chargeback));
    assert!(matches!(
        parse(&server, &chargeback("SUCCESS", &other)).await,
        Err(PaymentError::BadPayload(_))
    ));
    let lost = parse(&server, &chargeback("FAIL", &order)).await.unwrap();
    assert_eq!(lost.status, ProviderStatus::Pending);
    assert_eq!(lost.notice(), None);
    assert!(matches!(
        parse(&quiet, &routed("pay", "a=1".into())).await,
        Err(PaymentError::BadPayload(_))
    ));
    assert!(quiet.received_requests().await.unwrap().is_empty());
}

fn created() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "success": "true",
        "link_url": "https://pally.info/link/GkLWvKx3",
        "link_page_url": "https://pally.info/transfer/GkLWvKx3",
        "bill_id": BILL
    }))
}

async fn create(server: &MockServer, chosen: Option<&str>) -> PaymentResult<Invoice> {
    let mut request = invoice(rub(150_000), Some("client@example.com"));
    request.method = chosen.map(str::to_string);
    PayPalych::with_api_base(server.uri())
        .create_invoice(&settings(), &request)
        .await
}

#[tokio::test]
async fn счёт_paypalych() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bill/create"))
        .and(header("Authorization", format!("Bearer {TOKEN}").as_str()))
        .and(header("Content-Type", "application/x-www-form-urlencoded"))
        .respond_with(created())
        .mount(&server)
        .await;
    let invoice = create(&server, None).await.unwrap();
    assert_eq!(invoice.pay_url, "https://pally.info/transfer/GkLWvKx3");
    assert_eq!(invoice.external_id.as_deref(), Some(BILL));
    let sent = form_of(&server.received_requests().await.unwrap()[0]);
    let order = PAYMENT.to_string();
    let field = |key: &str| util::field(&sent, key);
    assert_eq!(field("amount"), Some("1500.00"));
    assert_eq!(field("shop_id"), Some(SHOP));
    assert_eq!(field("order_id"), Some(order.as_str()));
    assert_eq!(field("description"), Some("Стандарт · 3 месяца"));
    assert_eq!(field("type"), Some("normal"));
    assert_eq!(field("currency_in"), Some("RUB"));
    assert_eq!(field("ttl"), Some("3600"));
    assert_eq!(field("payer_data[email]"), Some("client@example.com"));
    assert_eq!(field("payment_method"), None);
    assert_eq!(field("success_url"), None);
    for (chosen, wire) in [
        (Some("sbp"), Some("SBP")),
        (Some("card"), Some("BANK_CARD")),
        (Some("page"), None),
    ] {
        create(&server, chosen).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let sent = form_of(requests.last().unwrap());
        assert_eq!(util::field(&sent, "payment_method"), wire, "{chosen:?}");
    }
    assert!(matches!(
        create(&server, Some("crypto")).await,
        Err(PaymentError::Rejected(_))
    ));
}

#[tokio::test]
async fn ошибки_paypalych_понятны_оператору() {
    for (status, body, expected) in [
        (
            401,
            json!({"message": "Unauthenticated."}),
            PaymentError::Rejected(util::KEYS_REFUSED.into()),
        ),
        (
            403,
            json!({"success": false, "message": "api:error.ip_access_denied"}),
            PaymentError::Rejected(plain("api:error.ip_access_denied")),
        ),
        (
            403,
            json!({"success": false, "message": "api:error.shop_not_found"}),
            PaymentError::Rejected("PayPalych не нашла магазин с таким ID".into()),
        ),
        (
            503,
            json!({"success": false, "message": "api:error.payment_method_temporarily_unavailable"}),
            PaymentError::Http { status: 503 },
        ),
        (
            200,
            json!({"success": false, "message": "api:error.invalid_amount"}),
            PaymentError::Rejected("PayPalych не приняла сумму счёта".into()),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        assert_eq!(create(&server, None).await, Err(expected));
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "bill_id": BILL, "link_page_url": "http://pally.info/transfer/x"
        })))
        .mount(&server)
        .await;
    assert!(matches!(
        create(&server, None).await,
        Err(PaymentError::BadPayload(_))
    ));
    let mut usd = invoice(usd(100), None);
    usd.method = None;
    assert!(matches!(
        PayPalych::with_api_base(server.uri())
            .create_invoice(&settings(), &usd)
            .await,
        Err(PaymentError::UnsupportedCurrency(_))
    ));
}

#[tokio::test]
async fn опрос_статуса_paypalych() {
    let server = api("SUCCESS", json!(1500), "RUB").await;
    let provider = PayPalych::with_api_base(server.uri());
    let outcome = provider
        .fetch_status(&settings(), BILL)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(
        provider.fetch_status(&settings(), "Missing1").await,
        Err(PaymentError::Rejected("счёт не найден в PayPalych".into()))
    );
    assert!(provider.fetch_status(&settings(), "../x").await.is_err());
}

fn refund_request(amount: i64) -> RefundRequest {
    RefundRequest {
        payment_id: PaymentId(41),
        provider_txid: BILL.into(),
        amount: rub(amount),
        reason: None,
        key: Uuid::from_u128(11),
        email: None,
        description: None,
    }
}

async fn refund_api(existing: serde_json::Value) -> MockServer {
    let server = api("SUCCESS", json!(1500), "RUB").await;
    Mock::given(method("GET"))
        .and(path("/bill/payments"))
        .and(query_param("id", BILL))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": [
                {"id": "PayFailed1", "bill_id": BILL, "status": "FAIL", "amount": 1500},
                {"id": "PayOk1", "bill_id": BILL, "status": "SUCCESS", "amount": 1500, "refunded_amount": 0}
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/refund/search"))
        .and(query_param("payment_id", "PayOk1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": existing})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/refund/full/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "id": "R1", "status": "NEW", "amount": 1500, "currency": "RUB"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/refund/partial/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {"id": "R2", "status": "SUCCESS", "amount": 500, "currency": "RUB"}
        })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn возврат_paypalych_полный_и_частичный() {
    let server = refund_api(json!([])).await;
    let provider = PayPalych::with_api_base(server.uri());
    let full = provider
        .refund(&settings(), &refund_request(150_000))
        .await
        .unwrap();
    assert_eq!(
        full,
        RefundOutcome {
            provider_refund_id: Some("refund:R1".into()),
            completed: false
        }
    );
    let partial = provider
        .refund(&settings(), &refund_request(50_000))
        .await
        .unwrap();
    assert_eq!(
        partial,
        RefundOutcome {
            provider_refund_id: Some("refund:R2".into()),
            completed: true
        }
    );
    let requests = server.received_requests().await.unwrap();
    let full_form = form_of(
        requests
            .iter()
            .find(|r| r.url.path() == "/refund/full/create")
            .unwrap(),
    );
    assert_eq!(util::field(&full_form, "payment_id"), Some("PayOk1"));
    assert_eq!(
        util::field(&full_form, "order_id"),
        Some(Uuid::from_u128(11).to_string().as_str())
    );
    assert_eq!(util::field(&full_form, "amount"), None);
    let partial_form = form_of(
        requests
            .iter()
            .find(|r| r.url.path() == "/refund/partial/create")
            .unwrap(),
    );
    assert_eq!(util::field(&partial_form, "amount"), Some("500.00"));
    assert!(matches!(
        provider.refund(&settings(), &refund_request(150_001)).await,
        Err(PaymentError::Rejected(_))
    ));
}

#[tokio::test]
async fn повтор_возврата_с_тем_же_ключом_не_возвращает_дважды() {
    let server = refund_api(json!([{
        "id": "R9", "status": "PROCESS", "amount": 1500, "currency": "RUB",
        "order_id": Uuid::from_u128(11).to_string()
    }]))
    .await;
    let outcome = PayPalych::with_api_base(server.uri())
        .refund(&settings(), &refund_request(150_000))
        .await
        .unwrap();
    assert_eq!(outcome.provider_refund_id.as_deref(), Some("refund:R9"));
    assert!(!outcome.completed);
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method == wiremock::http::Method::POST)
    );
}

#[tokio::test]
async fn возврат_только_по_оплаченному_счёту() {
    let server = api("NEW", json!(1500), "RUB").await;
    assert!(matches!(
        PayPalych::with_api_base(server.uri())
            .refund(&settings(), &refund_request(150_000))
            .await,
        Err(PaymentError::Rejected(_))
    ));
}

#[tokio::test]
async fn опрос_возврата_paypalych() {
    for (status, expected) in [
        (
            "SUCCESS",
            ProviderStatus::PartiallyRefunded {
                refunded: rub(150_000),
            },
        ),
        ("FAIL", ProviderStatus::RefundDeclined),
        ("NEW", ProviderStatus::Pending),
        ("PROCESS", ProviderStatus::Pending),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/refund/status"))
            .and(query_param("id", "R1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true, "id": "R1", "status": status, "amount": "1500.00", "currency": "RUB"
            })))
            .mount(&server)
            .await;
        let outcome = PayPalych::with_api_base(server.uri())
            .fetch_refund(&settings(), "refund:R1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, expected, "{status}");
        assert_eq!(outcome.event_key, "refund:R1");
    }
    assert_eq!(
        PayPalych::new().fetch_refund(&settings(), "R1").await,
        Ok(None)
    );
}

#[tokio::test]
async fn проверка_подключения_paypalych() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/merchant/balance"))
        .and(header("Authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": "true",
            "balances": [{"currency": "RUB", "balance_available": "93988.37000000", "balance_locked": "0", "balance_hold": "0"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/merchant/balance"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"message": "Unauthenticated."})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bill/search"))
        .and(query_param("shop_id", SHOP))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": []})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bill/search"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"success": false, "message": "api:error.shop_not_found"})),
        )
        .with_priority(10)
        .mount(&server)
        .await;
    let provider = PayPalych::with_api_base(server.uri());
    let context = CheckContext {
        webhook_url: Some(HOOK.into()),
    };
    let ok = provider.check_connection(&settings(), &context).await;
    assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
    assert!(
        ok.details[0].starts_with("Доступно на балансе: "),
        "{:?}",
        ok.details
    );
    assert!(
        ok.details
            .iter()
            .any(|d| d.contains(&format!("Result URL {HOOK}")))
    );
    assert!(ok.details.iter().any(
        |d| d.contains(&format!("{HOOK}/refund")) && d.contains(&format!("{HOOK}/chargeback"))
    ));
    let mut wrong = settings();
    wrong.set("api_token", json!("1|wrong"));
    let keys = provider.check_connection(&wrong, &context).await;
    assert_eq!(keys.status, CheckStatus::KeysRejected);
    assert_eq!(keys.summary, "PayPalych не приняла API-токен");
    let mut other_shop = settings();
    other_shop.set("shop_id", json!("Other7"));
    let shop = provider.check_connection(&other_shop, &context).await;
    assert_eq!(shop.status, CheckStatus::SettingsRejected);
    assert!(shop.summary.contains("Other7") && shop.summary.contains("не нашла магазин"));
    let bare = provider
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert!(
        bare.details
            .iter()
            .any(|d| d.contains("Адрес панели не задан"))
    );
    for request in server.received_requests().await.unwrap() {
        assert_eq!(request.method, wiremock::http::Method::GET);
    }
    let down = failing_api(502).await;
    assert_eq!(
        PayPalych::with_api_base(down.uri())
            .check_connection(&settings(), &context)
            .await
            .status,
        CheckStatus::Unreachable
    );
}

#[test]
fn настройки_и_способы_paypalych() {
    let provider = PayPalych::new();
    assert!(provider.is_configured(&settings()));
    assert!(provider.accepts_http_webhooks());
    assert!(provider.verifies_webhooks_by_api());
    assert!(provider.supports_refunds());
    assert!(provider.supports_refund_polling());
    let codes: Vec<&str> = provider
        .methods(Currency::Rub)
        .iter()
        .map(|m| m.code)
        .collect();
    assert_eq!(codes, ["sbp", "card", "page"]);
    assert!(provider.methods(Currency::Usd).is_empty());
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
        ["api_token"]
    );
    for (key, value) in [
        ("api_token", json!("72| с пробелом")),
        ("shop_id", json!("../shop")),
        ("shop_id", json!("x".repeat(65))),
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
