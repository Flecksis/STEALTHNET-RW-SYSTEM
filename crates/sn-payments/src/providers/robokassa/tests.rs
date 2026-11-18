use super::*;
use crate::test_support::*;
use serde_json::json;
use sn_domain::payment::{AmountCheck, check_paid_amount};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LOGIN: &str = "stealth-shop";
const PASS1: &str = "pAss1Main7";
const PASS2: &str = "pAss2Main8";
const PASS3: &str = "pAss3Refund9";
const TEST1: &str = "tEst1Pass5";
const TEST2: &str = "tEst2Pass6";
const OP_KEY: &str = "0005F891-8CCD-434B-8455-816AFFFDBF37-0VOisWikFF";
const REQUEST: &str = "68cd7fa6-1338-4745-ba5c-28d16cbcdb3d";

fn settings() -> ProviderConfig {
    config(&[
        ("merchant_login", json!(LOGIN)),
        ("password1", json!(PASS1)),
        ("password2", json!(PASS2)),
    ])
}

fn with(mut config: ProviderConfig, pairs: &[(&str, serde_json::Value)]) -> ProviderConfig {
    for (key, value) in pairs {
        config.set(key, value.clone());
    }
    config
}

fn testing() -> ProviderConfig {
    with(
        settings(),
        &[
            ("test_mode", json!(true)),
            ("test_password1", json!(TEST1)),
            ("test_password2", json!(TEST2)),
        ],
    )
}

fn inv() -> String {
    invoice_number(PAYMENT).to_string()
}

fn notification(out_sum: &str, inv: &str, password: &str, extra: &[(&str, &str)]) -> String {
    let mut custom: Vec<(&str, &str)> = extra
        .iter()
        .copied()
        .filter(|(k, _)| k.starts_with("Shp_"))
        .collect();
    custom.sort();
    let tail: String = custom.iter().map(|(k, v)| format!(":{k}={v}")).collect();
    let sign = signature("md5", &format!("{out_sum}:{inv}:{password}{tail}")).to_uppercase();
    let mut fields = vec![
        ("OutSum".to_string(), out_sum.to_string()),
        ("InvId".to_string(), inv.to_string()),
        ("Fee".to_string(), "52.500000".to_string()),
        ("EMail".to_string(), "client@example.com".to_string()),
        ("SignatureValue".to_string(), sign),
        ("PaymentMethod".to_string(), "BankCard".to_string()),
        ("IncCurrLabel".to_string(), "BankCardPSR".to_string()),
    ];
    for (k, v) in extra {
        fields.push((k.to_string(), v.to_string()));
    }
    fields
        .iter()
        .map(|(k, v)| format!("{k}={}", util::rfc3986(v)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn parse(config: &ProviderConfig, body: String) -> PaymentResult<WebhookOutcome> {
    Robokassa::new()
        .parse_webhook(config, &hook(&[], body))
        .await
}

fn query(url: &str) -> HashMap<String, String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn state_xml(result: u8, state: Option<&str>, out_sum: &str) -> String {
    let state = state
        .map(|code| {
            format!(
                "<State><Code>{code}</Code><RequestDate>2026-11-18T15:20:41.6216618+03:00</RequestDate><StateDate>2026-11-18T15:19:02.42+03:00</StateDate></State>"
            )
        })
        .unwrap_or_default();
    let info = if result == 0 {
        format!(
            "<Info><IncCurrLabel>BankCardPSR</IncCurrLabel><IncSum>{out_sum}</IncSum><IncAccount>220220******1234</IncAccount><PaymentMethod><Code>BankCard</Code><Description>Банковская карта</Description></PaymentMethod><OutCurrLabel>BankCardPSR</OutCurrLabel><OutSum>{out_sum}</OutSum><OpKey>{OP_KEY}</OpKey><BankCardRRN>000000000000</BankCardRRN></Info>"
        )
    } else {
        String::new()
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<OperationStateResponse xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns=\"http://merchant.roboxchange.com/WebService/\">\r\n  <Result>\r\n    <Code>{result}</Code>\r\n  </Result>\r\n  {state}{info}\r\n</OperationStateResponse>"
    )
}

fn xml(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/xml; charset=utf-8")
        .set_body_string(body)
}

async fn state_api(result: u8, state: Option<&str>, out_sum: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Merchant/WebService/Service.asmx/OpStateExt"))
        .respond_with(xml(state_xml(result, state, out_sum)))
        .mount(&server)
        .await;
    server
}

fn currencies_xml(result: u8) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<CurrenciesList xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns=\"http://merchant.roboxchange.com/WebService/\">\n  <Result>\n    <Code>{result}</Code>\n  </Result>\n  <Groups>\n    <Group Code=\"BankCard\" Description=\"Банковской картой\">\n      <Items>\n        <Currency Label=\"BankCardPSR\" Alias=\"BankCard\" Name=\"Банковская карта\"/>\n        <Currency Label=\"SBPPSR\" Alias=\"SBP\" Name=\"СБП\" MaxValue=\"1000000\"/>\n        <Currency Label=\"SberPayPSR\" Alias=\"SberPay\" Name=\"SberPay\" MinValue=\"1\" MaxValue=\"20000000\"/>\n      </Items>\n    </Group>\n  </Groups>\n</CurrenciesList>"
    )
}

fn decode(part: &str) -> serde_json::Value {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

#[test]
fn подпись_robokassa_по_контрольным_векторам() {
    let base = "demo:990.00:12:password_1";
    assert_eq!(signature("md5", base), "6c9d6eb96b82e643b343c26a85349a37");
    assert_eq!(
        signature("sha1", base),
        "2983d701dc8d3adaa38caaa3741a24b54d40f537"
    );
    assert_eq!(
        signature("sha256", base),
        "4cfecd5dae8217e8cd7a78b23f8dc56e5ec4766c6a548bbed5e1ac0c41c4b325"
    );
    assert_eq!(
        signature("sha384", base),
        "7eac8b1252034706b6707ee9cae22857b35cb76cf35f2ece9f0cbc350119c4c8681d10b2646b38b5b8a245d14ebf5fdd"
    );
    assert_eq!(
        signature("sha512", base),
        "7544de28e3e64061e86bbea2c15b21f75da407bb22ca9afb2c0c44f83e24aaf6d8a4637dbf0e9665921b02c9b1cdeb566714ad00b8d040f7a00374e93ee641db"
    );
    assert_eq!(
        signature(
            "md5",
            "100.000000:450009:password_2:Shp_login=Vasya:Shp_oplata=1"
        ),
        "4adcef23deded52faef69a96efdf67e8"
    );
    assert_eq!(
        signature("md5", "demo:12:password_2"),
        "700a3ea3d41e9811ded6225ae651150a"
    );
    assert_eq!(signature("ripemd160", base), "");
}

#[test]
fn номер_счёта_берётся_из_номера_платежа() {
    let first = invoice_number(PAYMENT);
    assert_eq!(first, invoice_number(PAYMENT));
    assert!((1..=INVOICE_MASK).contains(&first));
    assert_ne!(first, invoice_number(CLIENT));
    assert_eq!(invoice_number(Uuid::from_u128(0)), 1);
    assert!(invoice_number(Uuid::from_u128(u128::MAX)) <= INVOICE_MASK);
    assert!(invoice_id("0").is_err());
    assert!(invoice_id("12a").is_err());
    assert!(invoice_id("12345678901234567890").is_err());
    assert_eq!(invoice_id(" 450009 ").unwrap(), "450009");
}

#[tokio::test]
async fn ссылка_на_оплату_robokassa() {
    let request = invoice(rub(150_000), Some("client@example.com"));
    let link = Robokassa::new()
        .create_invoice(&settings(), &request)
        .await
        .unwrap();
    let number = inv();
    assert_eq!(link.external_id.as_deref(), Some(number.as_str()));
    assert!(
        link.pay_url
            .starts_with("https://auth.robokassa.ru/Merchant/Index.aspx?"),
        "{}",
        link.pay_url
    );
    let params = query(&link.pay_url);
    assert_eq!(params["MerchantLogin"], LOGIN);
    assert_eq!(params["OutSum"], "1500.00");
    assert_eq!(params["InvId"], number);
    assert_eq!(params["Description"], "Стандарт 3 месяца");
    assert_eq!(params["Email"], "client@example.com");
    assert_eq!(params["Culture"], "ru");
    assert_eq!(
        params["SignatureValue"],
        signature("md5", &format!("{LOGIN}:1500.00:{number}:{PASS1}"))
    );
    assert!(!params.contains_key("IsTest"));
    assert!(!params.contains_key("IncCurrLabel"));
    assert!(link.expires_at.is_none());

    let dollars = invoice(usd(1000), None);
    assert!(matches!(
        Robokassa::new().create_invoice(&settings(), &dollars).await,
        Err(PaymentError::UnsupportedCurrency(_))
    ));
}

#[tokio::test]
async fn способ_оплаты_тестовый_режим_и_алгоритм_уходят_в_ссылку() {
    async fn make(config: ProviderConfig, method: Option<&str>) -> PaymentResult<Invoice> {
        let mut request = invoice(rub(49_900), None);
        request.method = method.map(str::to_string);
        Robokassa::new().create_invoice(&config, &request).await
    }
    for (code, label) in [
        ("card", Some("BankCard")),
        ("sbp", Some("SBP")),
        ("sberpay", Some("SberPay")),
        ("tpay", Some("TinkoffPay")),
        ("page", None),
    ] {
        let invoice = make(settings(), Some(code)).await.unwrap();
        let params = query(&invoice.pay_url);
        assert_eq!(
            params.get("IncCurrLabel").map(String::as_str),
            label,
            "{code}"
        );
        assert!(!params.contains_key("Email"));
    }
    assert_eq!(
        make(settings(), Some("crypto")).await,
        Err(PaymentError::Rejected(
            "этот способ оплаты недоступен".into()
        ))
    );

    let invoice = make(testing(), None).await.unwrap();
    let params = query(&invoice.pay_url);
    let number = inv();
    assert_eq!(params["IsTest"], "1");
    assert_eq!(
        params["SignatureValue"],
        signature("md5", &format!("{LOGIN}:499.00:{number}:{TEST1}"))
    );
    assert_eq!(invoice.payload["test"], true);

    let half = with(settings(), &[("test_mode", json!(true))]);
    assert!(matches!(
        make(half, None).await,
        Err(PaymentError::NotConfigured(_))
    ));

    let sha = with(settings(), &[("hash", json!("sha256"))]);
    let invoice = make(sha, None).await.unwrap();
    assert_eq!(
        query(&invoice.pay_url)["SignatureValue"],
        signature("sha256", &format!("{LOGIN}:499.00:{number}:{PASS1}"))
    );
}

#[test]
fn описание_без_спецсимволов_и_не_длиннее_100_знаков() {
    assert_eq!(
        plain_description("Стандарт · 3 месяца"),
        "Стандарт 3 месяца"
    );
    assert_eq!(
        plain_description("Пополнение <script>\"баланса\"</script>"),
        "Пополнение script баланса script"
    );
    assert_eq!(plain_description("•••"), "Оплата заказа");
    assert_eq!(plain_description(&"я".repeat(300)).chars().count(), 100);
}

#[tokio::test]
async fn уведомление_с_верной_подписью_засчитывает_оплату() {
    let number = inv();
    let outcome = parse(
        &settings(),
        notification("1500.000000", &number, PASS2, &[]),
    )
    .await
    .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.event_key, format!("paid:{number}"));
    assert_eq!(
        outcome.payment,
        Some(PaymentRef::ProviderTxid(number.clone()))
    );
    assert_eq!(outcome.provider_txid.as_deref(), Some(number.as_str()));
    assert_eq!(
        outcome.reply,
        WebhookReply::Text {
            status: 200,
            body: format!("OK{number}")
        }
    );
    assert_eq!(
        check_paid_amount(rub(150_000), outcome.amount),
        AmountCheck::Ok
    );
    let two_digits = parse(&settings(), notification("1500.00", &number, PASS2, &[]))
        .await
        .unwrap();
    assert_eq!(two_digits.amount, Some(rub(150_000)));
    let again = parse(
        &settings(),
        notification("1500.000000", &number, PASS2, &[]),
    )
    .await
    .unwrap();
    assert_eq!(again.event_key, outcome.event_key);
}

#[tokio::test]
async fn без_подписи_или_с_чужой_подписью_уведомление_отклоняется() {
    let number = inv();
    let unsigned = format!("OutSum=1500.000000&InvId={number}");
    assert_eq!(
        parse(&settings(), unsigned).await,
        Err(PaymentError::BadSignature)
    );
    assert_eq!(
        parse(
            &settings(),
            notification("1500.000000", &number, PASS1, &[])
        )
        .await,
        Err(PaymentError::BadSignature)
    );
    let forged = notification("1500.000000", &number, PASS2, &[])
        .replace("OutSum=1500.000000", "OutSum=15000.000000");
    assert_eq!(
        parse(&settings(), forged).await,
        Err(PaymentError::BadSignature)
    );
    let sha = with(settings(), &[("hash", json!("sha512"))]);
    assert_eq!(
        parse(&sha, notification("1500.000000", &number, PASS2, &[])).await,
        Err(PaymentError::BadSignature)
    );
    assert_eq!(
        parse(&settings(), "OutSum=1&OutSum=2".into()).await,
        Err(PaymentError::BadPayload("поле «OutSum» повторяется".into()))
    );
}

#[tokio::test]
async fn пользовательские_параметры_входят_в_подпись_по_алфавиту() {
    let number = inv();
    let body = notification(
        "1500.000000",
        &number,
        PASS2,
        &[("Shp_oplata", "1"), ("Shp_login", "Vasya")],
    );
    assert!(body.find("Shp_oplata").unwrap() < body.find("Shp_login").unwrap());
    assert_eq!(
        parse(&settings(), body).await.unwrap().status,
        ProviderStatus::Paid
    );
    let mut fields = notification("1500.000000", &number, PASS2, &[("Shp_login", "Vasya")]);
    fields = fields.replace("Shp_login=Vasya", "Shp_login=Petya");
    assert_eq!(
        parse(&settings(), fields).await,
        Err(PaymentError::BadSignature)
    );
}

#[tokio::test]
async fn тестовые_оплаты_засчитываются_только_в_тестовом_режиме() {
    let number = inv();
    let test_signed = notification("499.00", &number, TEST2, &[]);
    assert_eq!(
        parse(&settings(), test_signed.clone()).await,
        Err(PaymentError::BadSignature)
    );
    let mut enabled_without_password = settings();
    enabled_without_password.set("test_mode", json!(true));
    assert_eq!(
        parse(&enabled_without_password, test_signed.clone()).await,
        Err(PaymentError::BadSignature)
    );
    let outcome = parse(&testing(), test_signed).await.unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(49_900)));
    let real = parse(&testing(), notification("1500.000000", &number, PASS2, &[]))
        .await
        .unwrap();
    assert_eq!(real.status, ProviderStatus::Paid);
}

#[tokio::test]
async fn сумма_не_сошлась_и_дробные_копейки() {
    let number = inv();
    let outcome = parse(
        &settings(),
        notification("1499.000000", &number, PASS2, &[]),
    )
    .await
    .unwrap();
    assert_eq!(outcome.amount, Some(rub(149_900)));
    assert!(!check_paid_amount(rub(150_000), outcome.amount).accepts());
    assert!(matches!(
        parse(
            &settings(),
            notification("1500.004000", &number, PASS2, &[])
        )
        .await,
        Err(PaymentError::BadPayload(_))
    ));
    assert!(matches!(
        parse(&settings(), notification("1500.000000", "0", PASS2, &[])).await,
        Err(PaymentError::BadPayload(_))
    ));
    assert_eq!(sum("1500.000000"), Ok(rub(150_000)));
    assert_eq!(sum("1500"), Ok(rub(150_000)));
    assert_eq!(sum("1500.5"), Ok(rub(150_050)));
    assert!(sum("-1").is_err());
    assert!(sum("").is_err());
    assert!(sum("1e3").is_err());
}

#[tokio::test]
async fn проверка_адреса_отправителя_по_желанию_оператора() {
    let number = inv();
    let strict = with(settings(), &[("check_ip", json!(true))]);
    let send = |config: ProviderConfig, ip: Option<&str>| {
        let body = notification("1500.000000", &number, PASS2, &[]);
        let mut request = hook(&[], body);
        request.remote_ip = ip.map(|ip| ip.parse().unwrap());
        async move { Robokassa::new().parse_webhook(&config, &request).await }
    };
    for ip in ["185.59.216.65", "185.59.217.65", "::ffff:185.59.217.65"] {
        assert!(send(strict.clone(), Some(ip)).await.is_ok(), "{ip}");
    }
    for ip in [Some("10.0.0.1"), Some("127.0.0.1"), None] {
        let Err(PaymentError::BadPayload(reason)) = send(strict.clone(), ip).await else {
            panic!("ожидался отказ для {ip:?}");
        };
        assert!(reason.contains("185.59.216.65"), "{reason}");
    }
    assert!(send(settings(), Some("10.0.0.1")).await.is_ok());
    let forged = {
        let mut request = hook(&[], notification("1500.000000", &number, PASS1, &[]));
        request.remote_ip = Some("10.0.0.1".parse().unwrap());
        Robokassa::new().parse_webhook(&strict, &request).await
    };
    assert_eq!(forged, Err(PaymentError::BadSignature));
}

#[test]
fn ответ_robokassa_в_xml_разбирается() {
    let body = state_xml(0, Some("100"), "1500.000000");
    assert_eq!(result_code(&body).as_deref(), Some("0"));
    assert_eq!(xml_text(&body, &["State", "Code"]).as_deref(), Some("100"));
    assert_eq!(
        xml_text(&body, &["Info", "PaymentMethod", "Description"]).as_deref(),
        Some("Банковская карта")
    );
    assert_eq!(xml_text(&body, &["Info", "OpKey"]).as_deref(), Some(OP_KEY));
    assert_eq!(xml_text(&body, &["Info", "Missing"]), None);
    assert_eq!(
        xml_text("<a><Code2>1</Code2><Code>7</Code></a>", &["a", "Code"]).as_deref(),
        Some("7")
    );
    assert_eq!(
        xml_text(
            "<a x=\"1\"><b/><c>R&amp;D &lt;1&gt; &#1046;&#x416;</c></a>",
            &["a", "c"]
        )
        .as_deref(),
        Some("R&D <1> ЖЖ")
    );
    assert_eq!(xml_text("<a><b/></a>", &["a", "b"]), None);
    assert_eq!(unescape("a & b &zz; &amp"), "a & b &zz; &amp");
    let listed = offers(&currencies_xml(0));
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[1].alias.as_deref(), Some("SBP"));
    assert_eq!(listed[0].name.as_deref(), Some("Банковская карта"));
}

#[tokio::test]
async fn опрос_статуса_robokassa() {
    let number = inv();
    let expected = signature("md5", &format!("{LOGIN}:{number}:{PASS2}"));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Merchant/WebService/Service.asmx/OpStateExt"))
        .and(query_param("MerchantLogin", LOGIN))
        .and(query_param("InvoiceID", number.as_str()))
        .and(query_param("Signature", expected.as_str()))
        .respond_with(xml(state_xml(0, Some("100"), "1500.000000")))
        .mount(&server)
        .await;
    let outcome = Robokassa::with_api_base(server.uri())
        .fetch_status(&settings(), &number)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Paid);
    assert_eq!(outcome.amount, Some(rub(150_000)));
    assert_eq!(outcome.provider_txid.as_deref(), Some(number.as_str()));

    for (state, expected) in [
        ("5", ProviderStatus::Pending),
        ("10", ProviderStatus::Canceled),
        ("20", ProviderStatus::Pending),
        ("50", ProviderStatus::Pending),
        ("60", ProviderStatus::Failed),
        ("80", ProviderStatus::Pending),
    ] {
        let server = state_api(0, Some(state), "1500.000000").await;
        let outcome = Robokassa::with_api_base(server.uri())
            .fetch_status(&settings(), &number)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, expected, "{state}");
        assert_eq!(outcome.amount, None, "{state}");
    }

    let missing = state_api(3, None, "").await;
    let outcome = Robokassa::with_api_base(missing.uri())
        .fetch_status(&settings(), &number)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ProviderStatus::Pending);
    assert_eq!(outcome.provider_txid.as_deref(), Some(number.as_str()));

    let rejected = state_api(1, None, "").await;
    let Err(PaymentError::Rejected(reason)) = Robokassa::with_api_base(rejected.uri())
        .fetch_status(&settings(), &number)
        .await
    else {
        panic!("ожидался отказ");
    };
    assert!(reason.contains("пароль №2"), "{reason}");

    let broken = failing_api(502).await;
    assert_eq!(
        Robokassa::with_api_base(broken.uri())
            .fetch_status(&settings(), &number)
            .await,
        Err(PaymentError::Http { status: 502 })
    );
    assert!(matches!(
        Robokassa::new().fetch_status(&settings(), "../x").await,
        Err(PaymentError::BadPayload(_))
    ));
}

async fn refund_api(state: &str, out_sum: &str, reply: serde_json::Value) -> MockServer {
    let server = state_api(0, Some(state), out_sum).await;
    Mock::given(method("POST"))
        .and(path("/RefundService/Refund/Create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply))
        .mount(&server)
        .await;
    server
}

fn refund_request(amount: Amount) -> RefundRequest {
    RefundRequest {
        payment_id: sn_domain::ids::PaymentId(41),
        provider_txid: inv(),
        amount,
        reason: None,
        key: Uuid::from_u128(99),
        email: None,
        description: None,
    }
}

fn sent_token(requests: &[wiremock::Request]) -> Vec<String> {
    requests
        .iter()
        .filter(|r| r.url.path() == "/RefundService/Refund/Create")
        .map(|r| {
            assert_eq!(
                r.headers.get("content-type").unwrap().to_str().unwrap(),
                "application/json"
            );
            serde_json::from_slice::<String>(&r.body).unwrap()
        })
        .collect()
}

#[tokio::test]
async fn возврат_через_api_robokassa() {
    let refundable = with(settings(), &[("password3", json!(PASS3))]);
    let ok = json!({"success": true, "message": null, "requestId": REQUEST});
    let server = refund_api("100", "1500.000000", ok.clone()).await;
    let provider = Robokassa::with_api_base(server.uri());
    let full = provider
        .refund(&refundable, &refund_request(rub(150_000)))
        .await
        .unwrap();
    assert_eq!(
        full,
        RefundOutcome {
            provider_refund_id: Some(REQUEST.into()),
            completed: false
        }
    );
    let again = provider
        .refund(&refundable, &refund_request(rub(150_000)))
        .await
        .unwrap();
    assert_eq!(again, full);
    let tokens = sent_token(&server.received_requests().await.unwrap());
    assert_eq!(tokens.len(), 1);
    let parts: Vec<&str> = tokens[0].split('.').collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(decode(parts[0]), json!({"alg": "HS256", "typ": "JWT"}));
    assert_eq!(decode(parts[1]), json!({"OpKey": OP_KEY}));
    let mut mac = Hmac::<Sha256>::new_from_slice(PASS3.as_bytes()).unwrap();
    mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
    assert_eq!(
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()),
        parts[2]
    );

    let server = refund_api("100", "1500.000000", ok).await;
    let mut part = refund_request(rub(50_000));
    part.key = Uuid::from_u128(100);
    Robokassa::with_api_base(server.uri())
        .refund(&refundable, &part)
        .await
        .unwrap();
    let tokens = sent_token(&server.received_requests().await.unwrap());
    let payload = tokens[0].split('.').nth(1).unwrap().to_string();
    assert_eq!(
        String::from_utf8(URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap(),
        format!("{{\"OpKey\":\"{OP_KEY}\",\"RefundSum\":500.00}}")
    );

    assert!(matches!(
        Robokassa::new()
            .refund(&settings(), &refund_request(rub(150_000)))
            .await,
        Err(PaymentError::NotConfigured(_))
    ));
    let poor = refund_api(
        "100",
        "1500.000000",
        json!({"success": false, "message": "NotEnoughOperationFunds", "requestId": null}),
    )
    .await;
    assert_eq!(
        Robokassa::with_api_base(poor.uri())
            .refund(&refundable, &refund_request(rub(150_000)))
            .await,
        Err(PaymentError::Rejected(
            "по этой операции в Robokassa не осталось денег для возврата".into()
        ))
    );
    let crediting = refund_api("50", "1500.000000", json!({})).await;
    assert!(matches!(
        Robokassa::with_api_base(crediting.uri())
            .refund(&refundable, &refund_request(rub(150_000)))
            .await,
        Err(PaymentError::Rejected(_))
    ));
    assert!(
        sent_token(&crediting.received_requests().await.unwrap()).is_empty(),
        "возврат по незачисленной оплате не запрашивается"
    );
}

async fn refund_state(reply: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/RefundService/Refund/GetState"))
        .and(query_param("id", REQUEST))
        .respond_with(reply)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn опрос_возврата_robokassa() {
    let finished = refund_state(ResponseTemplate::new(200).set_body_raw(
        format!("{{\"requestId\":\"{REQUEST}\",\"amount\":500.000000,\"label\":\"finished\"}}"),
        "application/json",
    ))
    .await;
    let outcome = Robokassa::with_api_base(finished.uri())
        .fetch_refund(&settings(), REQUEST)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome.status,
        ProviderStatus::PartiallyRefunded {
            refunded: rub(50_000)
        }
    );
    assert_eq!(outcome.event_key, format!("refund:{REQUEST}"));
    assert_eq!(outcome.amount, Some(rub(50_000)));

    for (label, expected) in [
        ("processing", ProviderStatus::Pending),
        ("canceled", ProviderStatus::RefundDeclined),
    ] {
        let server = refund_state(
            ResponseTemplate::new(200)
                .set_body_json(json!({"requestId": REQUEST, "amount": 500, "label": label})),
        )
        .await;
        let outcome = Robokassa::with_api_base(server.uri())
            .fetch_refund(&settings(), REQUEST)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, expected, "{label}");
    }

    let unknown = refund_state(
        ResponseTemplate::new(400)
            .set_body_json(json!({"message": "Id is invalid or request id does not exist"})),
    )
    .await;
    assert_eq!(
        Robokassa::with_api_base(unknown.uri())
            .fetch_refund(&settings(), REQUEST)
            .await,
        Err(PaymentError::Rejected(
            "заявка на возврат не найдена в Robokassa".into()
        ))
    );
    assert_eq!(
        Robokassa::new().fetch_refund(&settings(), "refund-1").await,
        Ok(None)
    );
}

async fn check_api(currencies: u8, md5: u8, sha256: u8) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Merchant/WebService/Service.asmx/GetCurrencies"))
        .and(query_param("MerchantLogin", LOGIN))
        .respond_with(xml(currencies_xml(currencies)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Merchant/WebService/Service.asmx/OpStateExt"))
        .respond_with(move |request: &wiremock::Request| {
            let params: HashMap<String, String> = request
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let base = format!(
                "{}:{}:{PASS2}",
                params["MerchantLogin"], params["InvoiceID"]
            );
            let code = if params["Signature"] == signature("md5", &base) {
                md5
            } else if params["Signature"] == signature("sha256", &base) {
                sha256
            } else {
                1
            };
            xml(state_xml(code, None, ""))
        })
        .mount(&server)
        .await;
    server
}

async fn check(server: &MockServer, config: &ProviderConfig) -> ConnectionCheck {
    let context = CheckContext {
        webhook_url: Some("https://panel.example.com/api/webhooks/pay/robokassa".into()),
    };
    Robokassa::with_api_base(server.uri())
        .check_connection(config, &context)
        .await
}

#[tokio::test]
async fn проверка_подключения_robokassa() {
    let server = check_api(0, 3, 1).await;
    let result = check(&server, &settings()).await;
    assert_eq!(result.status, CheckStatus::Ok, "{result:?}");
    let text = result.details.join("\n");
    assert!(text.contains("Банковская карта, СБП, SberPay"), "{text}");
    assert!(
        text.contains("https://panel.example.com/api/webhooks/pay/robokassa"),
        "{text}"
    );
    assert!(text.contains("Пароль №3 не задан"), "{text}");
    assert!(!serde_json::to_string(&result).unwrap().contains(PASS2));

    let wrong_hash = check_api(0, 1, 3).await;
    let result = check(&wrong_hash, &settings()).await;
    assert_eq!(result.status, CheckStatus::SettingsRejected, "{result:?}");
    assert!(result.summary.contains("SHA-256"), "{}", result.summary);

    let wrong_password = check_api(0, 1, 1).await;
    let result = check(&wrong_password, &settings()).await;
    assert_eq!(result.status, CheckStatus::KeysRejected, "{result:?}");
    assert!(result.summary.contains("пароль №2"), "{}", result.summary);

    let unknown_shop = check_api(2, 3, 3).await;
    let result = check(&unknown_shop, &settings()).await;
    assert_eq!(result.status, CheckStatus::KeysRejected);
    assert!(result.summary.contains(LOGIN), "{}", result.summary);

    let testing_check = check(&server, &with(testing(), &[("password3", json!(PASS3))])).await;
    let text = testing_check.details.join("\n");
    assert!(text.contains("тестовый режим"), "{text}");
    assert!(!text.contains("Пароль №3 не задан"), "{text}");

    let down = failing_api(503).await;
    assert_eq!(
        check(&down, &settings()).await.status,
        CheckStatus::Unreachable
    );
    let closed = Robokassa::with_api_base(closed_api())
        .check_connection(&settings(), &CheckContext::default())
        .await;
    assert_eq!(closed.status, CheckStatus::Unreachable);
}

#[test]
fn настройки_и_способы_robokassa() {
    let provider = Robokassa::new();
    assert!(provider.is_configured(&settings()));
    assert!(provider.validate_settings(&settings()).is_ok());
    assert!(!provider.is_configured(&ProviderConfig::default()));
    for (key, value) in [
        ("password1", json!("with:colon")),
        ("password2", json!("with space")),
        ("merchant_login", json!("шоп")),
        ("hash", json!("ripemd160")),
    ] {
        assert!(
            provider
                .validate_settings(&with(settings(), &[(key, value.clone())]))
                .is_err(),
            "{key}={value}"
        );
    }
    assert!(
        provider
            .validate_settings(&with(settings(), &[("test_mode", json!(true))]))
            .is_err()
    );
    assert!(provider.validate_settings(&testing()).is_ok());
    let schema = provider.settings_schema();
    let secrets: Vec<&str> = schema
        .iter()
        .filter(|f| f.is_secret())
        .map(|f| f.key)
        .collect();
    assert_eq!(
        secrets,
        [
            "password1",
            "password2",
            "password3",
            "test_password1",
            "test_password2"
        ]
    );
    let hash = schema.iter().find(|f| f.key == "hash").unwrap();
    assert_eq!(hash.default, Some("md5"));
    assert_eq!(hash.labels, Some(HASH_LABELS));
    let codes: Vec<&str> = provider
        .methods(Currency::Rub)
        .iter()
        .map(|m| m.code)
        .collect();
    assert_eq!(codes, ["card", "sbp", "sberpay", "tpay", "page"]);
    assert!(provider.methods(Currency::Usd).is_empty());
    let label = provider.client_label(Currency::Rub, &settings());
    assert_eq!(label.title, "Карта");
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
    assert!(crate::english::text(label.hint).is_some());
}
