use crate::error::{PaymentError, PaymentResult};
use crate::json::{self, Json};
use crate::md5;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::DateTime;
use sn_domain::money::{Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use uuid::Uuid;

const API: &str = "https://api.heleket.hk";

pub struct Heleket {
    http: reqwest::Client,
    api_base: Option<String>,
}

impl Default for Heleket {
    fn default() -> Self {
        Self::new()
    }
}

impl Heleket {
    pub fn new() -> Self {
        Heleket {
            http: http_client(),
            api_base: None,
        }
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Heleket {
            http: http_client(),
            api_base: Some(api_base.into()),
        }
    }

    fn base(&self, config: &ProviderConfig) -> String {
        match &self.api_base {
            Some(base) => base.trim_end_matches('/').to_string(),
            None => config
                .get("api_url")
                .filter(|url| util::is_https(url))
                .unwrap_or(API)
                .trim_end_matches('/')
                .to_string(),
        }
    }

    async fn call(&self, config: &ProviderConfig, path: &str, body: &Json) -> PaymentResult<Json> {
        let merchant = config.require("merchant_id")?;
        let key = config.require("api_key")?;
        let bytes = body.to_bytes();
        let reply = util::send(
            self.http
                .post(format!("{}{path}", self.base(config)))
                .header("merchant", merchant)
                .header("sign", signature(&bytes, key))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(bytes),
        )
        .await?;
        let parsed = reply.json()?;
        let accepted = parsed.get("state").and_then(Json::scalar_text) == Some("0");
        match parsed.get("result") {
            Some(result) if reply.is_success() && accepted => Ok(result.clone()),
            _ => Err(reply.failure(error_text(&parsed).as_deref())),
        }
    }
}

fn error_text(parsed: &Json) -> Option<String> {
    if let Some(message) = parsed
        .get("message")
        .and_then(Json::as_str)
        .filter(|m| !m.trim().is_empty())
    {
        return Some(message.to_string());
    }
    let mut texts = Vec::new();
    if let Some(Json::Object(fields)) = parsed.get("errors") {
        for (_, value) in fields {
            match value {
                Json::String(text) => texts.push(text.clone()),
                Json::Array(items) => {
                    texts.extend(items.iter().filter_map(Json::as_str).map(str::to_string))
                }
                _ => {}
            }
        }
    }
    let texts: Vec<&str> = texts
        .iter()
        .map(|t| t.trim().trim_end_matches('.'))
        .filter(|t| !t.is_empty())
        .collect();
    (!texts.is_empty()).then(|| texts.join("; "))
}

pub fn signature(body: &[u8], api_key: &str) -> String {
    md5::hex(format!("{}{api_key}", STANDARD.encode(body)).as_bytes())
}

fn signed_variants(raw: &[u8]) -> Option<(String, Vec<Vec<u8>>)> {
    let members = json::members(raw)?;
    let sign = members.iter().find(|m| m.key == "sign")?;
    let received = Json::parse(sign.raw_value)?.as_str()?.to_ascii_lowercase();
    let mut original = vec![b'{'];
    for (i, member) in members.iter().filter(|m| m.key != "sign").enumerate() {
        if i > 0 {
            original.push(b',');
        }
        original.extend_from_slice(member.raw_key);
        original.push(b':');
        original.extend_from_slice(member.raw_value);
    }
    original.push(b'}');
    let mut variants = vec![original];
    if let Some(Json::Object(mut all)) = Json::parse(raw) {
        all.retain(|(key, _)| key != "sign");
        let php = Json::Object(all).to_php().into_bytes();
        if !variants.contains(&php) {
            variants.push(php);
        }
    }
    Some((received, variants))
}

fn verify(api_key: &str, raw: &[u8]) -> bool {
    let Some((received, variants)) = signed_variants(raw) else {
        return false;
    };
    received.len() == 32
        && variants
            .iter()
            .any(|variant| util::secret_eq(&signature(variant, api_key), &received))
}

fn map_status(status: &str) -> ProviderStatus {
    match status {
        "paid" | "paid_over" => ProviderStatus::Paid,
        "wrong_amount" | "fail" | "system_fail" => ProviderStatus::Failed,
        "cancel" => ProviderStatus::Canceled,
        "refund_paid" => ProviderStatus::Refunded,
        _ => ProviderStatus::Pending,
    }
}

fn outcome(info: &Json) -> PaymentResult<WebhookOutcome> {
    let uuid = info
        .get("uuid")
        .and_then(Json::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| util::bad("нет uuid счёта"))?;
    let status = info
        .get("status")
        .or_else(|| info.get("payment_status"))
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет статуса счёта"))?;
    let mapped = map_status(status);
    let amount = match mapped {
        ProviderStatus::Paid => Some(util::json_amount(info.get("amount"), info.get("currency"))?),
        _ => None,
    };
    Ok(WebhookOutcome {
        event_key: format!("{uuid}:{status}"),
        payment: util::public_ref(info.get("order_id").and_then(Json::as_str))
            .or_else(|| Some(PaymentRef::ProviderTxid(uuid.to_string()))),
        provider_txid: Some(uuid.to_string()),
        status: mapped,
        amount,
        saved_method: None,
        reply: WebhookReply::ok(),
    })
}

#[async_trait]
impl PaymentProvider for Heleket {
    fn id(&self) -> &'static str {
        providers::HELEKET
    }

    fn title(&self) -> &'static str {
        "Heleket"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Usd]
    }

    fn client_label(&self, _currency: Currency) -> ClientLabel {
        ClientLabel {
            title: "Криптовалюта",
            hint: "USDT и другие монеты",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                "merchant_id",
                "ID мерчанта",
                "Личный кабинет Heleket → Настройки",
            ),
            SettingField::secret(
                "api_key",
                "Платёжный API-ключ",
                "Личный кабинет Heleket → API",
            ),
            SettingField::url(
                "api_url",
                "Адрес API",
                "Оставьте пустым, тогда используется https://api.heleket.hk. Меняйте, только если Heleket объявит новый адрес",
            )
            .optional(),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if config
            .get("api_url")
            .is_some_and(|url| !util::is_https(url))
        {
            return Err(PaymentError::NotConfigured(
                "адрес API Heleket должен начинаться с https://".into(),
            ));
        }
        match config.get("merchant_id").map(Uuid::parse_str) {
            Some(Ok(_)) => Ok(()),
            _ => Err(PaymentError::NotConfigured(
                "ID мерчанта Heleket — это UUID из личного кабинета".into(),
            )),
        }
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Usd)?;
        let mut body = Json::object([
            ("amount", Json::from(request.amount.units())),
            ("currency", Json::from("USD")),
            (
                "order_id",
                Json::from(request.payment_public_id.to_string()),
            ),
            ("url_callback", Json::from(request.webhook_url.clone())),
            ("is_payment_multiple", Json::from(true)),
            (
                "lifetime",
                Json::from(request.expires_in.num_seconds().clamp(300, 43_200)),
            ),
        ]);
        if let Some(url) = util::return_url(request) {
            body.push("url_return", Json::from(url.clone()));
            body.push("url_success", Json::from(url));
        }
        let result = self.call(config, "/v1/payment", &body).await?;
        let uuid = result
            .get("uuid")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| util::bad("Heleket не вернул uuid счёта"))?;
        let pay_url = result
            .get("url")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("Heleket не вернул ссылку на оплату"))?;
        let expires_at = result
            .get("expired_at")
            .and_then(Json::scalar_text)
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|secs| DateTime::from_timestamp(secs, 0));
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(uuid.to_string()),
            expires_at,
            payload: serde_json::json!({ "uuid": uuid }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let key = config.require("api_key")?;
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        if !verify(key, &request.body) {
            return Err(PaymentError::BadSignature);
        }
        match body.get("type").and_then(Json::as_str) {
            Some("payment") | None => outcome(&body),
            Some(other) => Ok(util::skipped(
                format!(
                    "{other}:{}",
                    body.get("uuid").and_then(Json::as_str).unwrap_or_default()
                ),
                None,
                None,
                WebhookReply::ok(),
            )),
        }
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let uuid = util::safe_id(external_id)?;
        let result = self
            .call(
                config,
                "/v1/payment/info",
                &Json::object([("uuid", Json::from(uuid))]),
            )
            .await?;
        outcome(&result).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;
    use sn_domain::payment::{AmountCheck, check_paid_amount};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

    const KEY: &str = "heleket-payment-key";
    const MERCHANT: &str = "8b03432e-385b-4670-8d06-064591096795";

    fn settings() -> ProviderConfig {
        config(&[("merchant_id", json!(MERCHANT)), ("api_key", json!(KEY))])
    }

    fn data(status: &str, amount: &str, currency: &str) -> String {
        format!(
            r#"{{"type":"payment","uuid":"62f88b36-a9d5-4fa6-aa26-e040c3dbf26d","order_id":"{PAYMENT}","amount":"{amount}","payment_amount":"6.99000000","payment_amount_usd":"6.99","merchant_amount":"6.85","is_final":true,"status":"{status}","from":null,"network":"tron","currency":"{currency}","payer_currency":"USDT","additional_data":null,"url":"https:\/\/pay.heleket.com\/pay\/62f88b36","txid":"6f0d9c83"}}"#
        )
    }

    fn signed(data: &str, key: &str) -> String {
        let sign = signature(data.as_bytes(), key);
        format!("{},\"sign\":\"{sign}\"}}", &data[..data.len() - 1])
    }

    async fn parse(body: String) -> PaymentResult<WebhookOutcome> {
        Heleket::new()
            .parse_webhook(&settings(), &hook(&[], body))
            .await
    }

    #[tokio::test]
    async fn подпись_проверяется_по_исходным_байтам() {
        let body = signed(&data("paid", "6.99", "USD"), KEY);
        let outcome = parse(body.clone()).await.unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(usd(699)));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(
            outcome.provider_txid.as_deref(),
            Some("62f88b36-a9d5-4fa6-aa26-e040c3dbf26d")
        );
        let wrong = signed(&data("paid", "6.99", "USD"), "other-key");
        assert_eq!(parse(wrong).await, Err(PaymentError::BadSignature));
        let missing = data("paid", "6.99", "USD");
        assert_eq!(parse(missing).await, Err(PaymentError::BadSignature));
        let tampered = body.replace("\"6.99\",\"payment_amount\"", "\"0.01\",\"payment_amount\"");
        assert_ne!(tampered, body);
        assert_eq!(parse(tampered).await, Err(PaymentError::BadSignature));
        assert!(matches!(
            parse("not json".into()).await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn подпись_сходится_и_для_тела_с_пробелами_и_экранированным_юникодом() {
        let canonical = r#"{"type":"payment","uuid":"u-1","order_id":"x","amount":"1.00","currency":"USD","status":"paid","additional_data":"Тест\/1"}"#;
        let sign = signature(canonical.as_bytes(), KEY);
        let pretty = format!(
            "{{ \"type\": \"payment\", \"uuid\": \"u-1\", \"order_id\": \"x\", \"amount\": \"1.00\", \"currency\": \"USD\", \"status\": \"paid\", \"additional_data\": \"\\u0422\\u0435\\u0441\\u0442/1\", \"sign\": \"{sign}\" }}"
        );
        let outcome = parse(pretty).await.unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(
            outcome.payment,
            Some(PaymentRef::ProviderTxid("u-1".into()))
        );
    }

    #[tokio::test]
    async fn статусы_heleket() {
        for (status, expected) in [
            ("paid", ProviderStatus::Paid),
            ("paid_over", ProviderStatus::Paid),
            ("wrong_amount", ProviderStatus::Failed),
            ("fail", ProviderStatus::Failed),
            ("system_fail", ProviderStatus::Failed),
            ("cancel", ProviderStatus::Canceled),
            ("refund_paid", ProviderStatus::Refunded),
            ("process", ProviderStatus::Pending),
            ("check", ProviderStatus::Pending),
            ("confirm_check", ProviderStatus::Pending),
            ("wrong_amount_waiting", ProviderStatus::Pending),
            ("locked", ProviderStatus::Pending),
        ] {
            let outcome = parse(signed(&data(status, "6.99", "USD"), KEY))
                .await
                .unwrap();
            assert_eq!(outcome.status, expected, "{status}");
            assert_eq!(
                outcome.event_key,
                format!("62f88b36-a9d5-4fa6-aa26-e040c3dbf26d:{status}")
            );
        }
        let not_final = signed(
            &data("paid", "6.99", "USD").replace("\"is_final\":true", "\"is_final\":false"),
            KEY,
        );
        assert_eq!(parse(not_final).await.unwrap().status, ProviderStatus::Paid);
        let over_not_final = signed(
            &data("paid_over", "7.10", "USD").replace("\"is_final\":true", "\"is_final\":false"),
            KEY,
        );
        let over = parse(over_not_final).await.unwrap();
        assert_eq!(over.status, ProviderStatus::Paid);
        assert_eq!(over.amount, Some(usd(710)));
        let wallet = signed(
            &data("paid", "6.99", "USD").replace("\"type\":\"payment\"", "\"type\":\"wallet\""),
            KEY,
        );
        let skipped = parse(wallet).await.unwrap();
        assert_eq!(skipped.payment, None);
        assert_eq!(skipped.status, ProviderStatus::Pending);
    }

    #[tokio::test]
    async fn недоплата_и_чужая_валюта_не_проходят_сверку() {
        let expected = usd(699);
        let under = parse(signed(&data("paid", "6.98", "USD"), KEY))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, under.amount),
            AmountCheck::Underpaid { .. }
        ));
        let rub = parse(signed(&data("paid", "6.99", "RUB"), KEY))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, rub.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
        assert!(matches!(
            parse(signed(&data("paid", "6.99", "USDT"), KEY)).await,
            Err(PaymentError::BadPayload(_))
        ));
        let eight = parse(signed(&data("paid", "6.99000000", "USD"), KEY))
            .await
            .unwrap();
        assert_eq!(eight.amount, Some(usd(699)));
        assert!(matches!(
            parse(signed(&data("paid", "6.999", "USD"), KEY)).await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    struct SignedBody;

    impl Match for SignedBody {
        fn matches(&self, request: &Request) -> bool {
            request
                .headers
                .get("sign")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|sign| sign == signature(&request.body, KEY))
        }
    }

    #[tokio::test]
    async fn счёт_создаётся_с_подписью_тела() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .and(header("merchant", MERCHANT))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "26109ba0-b05b-4ee0-93d1-fd62c822ce95",
                    "order_id": PAYMENT.to_string(),
                    "amount": "6.99",
                    "currency": "USD",
                    "url": "https://pay.heleket.com/pay/26109ba0",
                    "expired_at": 1_763_308_800,
                    "status": "check",
                    "is_final": false
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider = Heleket::with_api_base(server.uri());
        let created = provider
            .create_invoice(&settings(), &invoice(usd(699), None))
            .await
            .unwrap();
        assert_eq!(
            created.external_id.as_deref(),
            Some("26109ba0-b05b-4ee0-93d1-fd62c822ce95")
        );
        assert_eq!(created.pay_url, "https://pay.heleket.com/pay/26109ba0");
        assert_eq!(
            created.expires_at.map(|at| at.timestamp()),
            Some(1_763_308_800)
        );
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(body["amount"], "6.99");
        assert_eq!(body["currency"], "USD");
        assert_eq!(body["order_id"], PAYMENT.to_string());
        assert_eq!(body["lifetime"], 3600);
        assert_eq!(body["is_payment_multiple"], true);
        assert!(matches!(
            provider
                .create_invoice(&settings(), &invoice(rub(49_900), None))
                .await,
            Err(PaymentError::UnsupportedCurrency(_))
        ));
    }

    #[tokio::test]
    async fn ошибка_heleket_не_становится_счётом() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "state": 1,
                "message": "The amount field must be at least 1"
            })))
            .mount(&server)
            .await;
        let result = Heleket::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(usd(50), None))
            .await;
        assert_eq!(
            result,
            Err(PaymentError::Rejected(
                "The amount field must be at least 1".into()
            ))
        );
    }

    #[tokio::test]
    async fn ошибки_полей_heleket_видны_оператору() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "state": 1,
                "errors": {
                    "amount": ["The amount field must be at least 1."],
                    "url_callback": ["The url callback format is invalid."]
                }
            })))
            .mount(&server)
            .await;
        let result = Heleket::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(usd(50), None))
            .await;
        assert_eq!(
            result,
            Err(PaymentError::Rejected(
                "The amount field must be at least 1; The url callback format is invalid".into()
            ))
        );
    }

    #[test]
    fn адрес_api_новый_и_меняется_в_настройках() {
        let heleket = Heleket::new();
        assert_eq!(heleket.base(&settings()), "https://api.heleket.hk");
        let mut custom = settings();
        custom.set("api_url", json!("https://api.heleket.example/"));
        assert_eq!(heleket.base(&custom), "https://api.heleket.example");
        assert!(heleket.is_configured(&custom));
        custom.set("api_url", json!("http://api.heleket.example"));
        assert!(!heleket.is_configured(&custom));
        assert_eq!(heleket.base(&custom), "https://api.heleket.hk");
    }

    #[tokio::test]
    async fn статус_счёта_запрашивается_по_uuid() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/payment/info"))
            .and(SignedBody)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": 0,
                "result": {
                    "uuid": "26109ba0",
                    "order_id": PAYMENT.to_string(),
                    "amount": "6.99",
                    "currency": "USD",
                    "payment_status": "paid",
                    "is_final": true
                }
            })))
            .mount(&server)
            .await;
        let outcome = Heleket::with_api_base(server.uri())
            .fetch_status(&settings(), "26109ba0")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(usd(699)));
        let sent = server.received_requests().await.unwrap();
        assert_eq!(sent[0].body, br#"{"uuid":"26109ba0"}"#);
    }

    #[test]
    fn настройки_heleket_проверяются() {
        let heleket = Heleket::new();
        assert!(heleket.is_configured(&settings()));
        assert!(!heleket.is_configured(&ProviderConfig::default()));
        let bad = config(&[("merchant_id", json!("shop-1")), ("api_key", json!(KEY))]);
        assert!(matches!(
            heleket.validate_settings(&bad),
            Err(PaymentError::NotConfigured(_))
        ));
    }
}
