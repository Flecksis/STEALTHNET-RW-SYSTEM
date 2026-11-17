use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util::{self, field};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use uuid::Uuid;

const API: &str = "https://api.cloudpayments.ru";
const HMAC_HEADER: &str = "content-hmac";

pub struct CloudPayments {
    http: reqwest::Client,
    api_base: String,
}

impl Default for CloudPayments {
    fn default() -> Self {
        Self::new()
    }
}

impl CloudPayments {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        CloudPayments {
            http: http_client(),
            api_base: api_base.into().trim_end_matches('/').to_string(),
        }
    }

    async fn call(
        &self,
        config: &ProviderConfig,
        method: &str,
        body: &Json,
        request_id: Option<&str>,
    ) -> PaymentResult<Option<Json>> {
        let public_id = config.require("public_id")?;
        let secret = config.require("api_secret")?;
        let mut builder = self
            .http
            .post(format!("{}/{method}", self.api_base))
            .basic_auth(public_id, Some(secret))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_bytes());
        if let Some(id) = request_id {
            builder = builder.header("X-Request-ID", id);
        }
        let reply = util::send(builder).await?;
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(parsed.get("Message").and_then(Json::as_str)));
        }
        match parsed.get("Success").and_then(Json::as_bool) {
            Some(true) => Ok(Some(parsed.get("Model").cloned().unwrap_or(Json::Null))),
            _ => match parsed.get("Message").and_then(Json::as_str) {
                Some("Not found") => Ok(None),
                message => {
                    Err(reply.failure(Some(message.unwrap_or("CloudPayments отклонил запрос"))))
                }
            },
        }
    }
}

fn reply(code: i64) -> WebhookReply {
    WebhookReply::Json {
        status: 200,
        body: serde_json::json!({ "code": code }),
    }
}

fn hmac_valid(secret: &str, body: &[u8], header: Option<&str>) -> bool {
    let Some(expected) = header.and_then(|h| STANDARD.decode(h.trim()).ok()) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

fn notification_fields(body: &[u8]) -> PaymentResult<Vec<(String, String)>> {
    let trimmed = body.iter().position(|b| !b.is_ascii_whitespace());
    if trimmed.and_then(|at| body.get(at)) != Some(&b'{') {
        return util::form_pairs(body);
    }
    match Json::parse(body) {
        Some(Json::Object(members)) => Ok(members
            .into_iter()
            .filter_map(|(key, value)| {
                let text = match value {
                    Json::String(s) | Json::Number(s) => s,
                    Json::Bool(b) => b.to_string(),
                    _ => return None,
                };
                Some((key, text))
            })
            .collect()),
        _ => Err(util::bad("тело не JSON-объект")),
    }
}

fn transaction_number(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

fn is_test(fields: &[(String, String)]) -> bool {
    field(fields, "TestMode").is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn amount(fields: &[(String, String)], amount_key: &str) -> PaymentResult<Amount> {
    let currency = util::pay_currency(field(fields, "Currency").unwrap_or("RUB"))?;
    let raw = field(fields, amount_key).ok_or_else(|| util::bad("нет суммы"))?;
    Ok(Amount::new(util::minor(raw, currency)?, currency))
}

fn check_code(fields: &[(String, String)], test_matches: bool) -> i64 {
    if !test_matches || field(fields, "OperationType").is_some_and(|op| op != "Payment") {
        return 13;
    }
    if util::public_ref(field(fields, "InvoiceId")).is_none() {
        return 10;
    }
    match amount(fields, "Amount") {
        Ok(paid) if paid.currency == PayCurrency::Rub && paid.minor > 0 => 0,
        _ => 12,
    }
}

fn transaction_outcome(model: &Json) -> PaymentResult<WebhookOutcome> {
    let id = model
        .get("TransactionId")
        .and_then(Json::scalar_text)
        .ok_or_else(|| util::bad("нет TransactionId"))?;
    let status_text = model
        .get("Status")
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет статуса транзакции"))?;
    let status = match status_text {
        "Completed" => ProviderStatus::Paid,
        _ => ProviderStatus::Pending,
    };
    let currency = model.get("Currency").cloned().unwrap_or(Json::from("RUB"));
    Ok(WebhookOutcome {
        event_key: format!("transaction.{status_text}:{id}"),
        payment: util::public_ref(model.get("InvoiceId").and_then(Json::as_str))
            .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string()))),
        provider_txid: Some(id.to_string()),
        status,
        amount: Some(util::json_amount(model.get("Amount"), Some(&currency))?),
        saved_method: None,
        reply: WebhookReply::ok(),
    })
}

#[async_trait]
impl PaymentProvider for CloudPayments {
    fn id(&self) -> &'static str {
        providers::CLOUDPAYMENTS
    }

    fn title(&self) -> &'static str {
        "CloudPayments"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: "Карты российских банков",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text("public_id", "Public ID", "Кабинет CloudPayments → Сайты"),
            SettingField::secret(
                "api_secret",
                "Пароль для API",
                "Кабинет CloudPayments → Сайты",
            ),
            SettingField::flag(
                "test_mode",
                "Тестовый режим",
                "Принимать только тестовые оплаты сайта в тестовом режиме",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if config
            .get("public_id")
            .is_some_and(|id| id.starts_with("pk_"))
        {
            Ok(())
        } else {
            Err(PaymentError::NotConfigured(
                "Public ID CloudPayments начинается с pk_".into(),
            ))
        }
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn supports_refunds(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let mut body = Json::object([
            ("Amount", Json::number(request.amount.units())),
            ("Currency", Json::from("RUB")),
            (
                "Description",
                Json::from(util::clip(&request.description, 250)),
            ),
            (
                "InvoiceId",
                Json::from(request.payment_public_id.to_string()),
            ),
            (
                "AccountId",
                Json::from(request.client.public_id.to_string()),
            ),
            ("RequireConfirmation", Json::from(false)),
            ("SendEmail", Json::from(false)),
            ("CultureName", Json::from("ru-RU")),
        ]);
        if let Some(email) = util::client_email(request) {
            body.push("Email", Json::from(email));
        }
        if let Some(url) = util::return_url(request).filter(|u| util::is_https(u)) {
            body.push("SuccessRedirectUrl", Json::from(url.clone()));
            body.push("FailRedirectUrl", Json::from(url));
        }
        let order = self
            .call(
                config,
                "orders/create",
                &body,
                Some(&request.payment_public_id.to_string()),
            )
            .await?
            .ok_or_else(|| PaymentError::Rejected("CloudPayments не создал счёт".into()))?;
        let pay_url = order
            .get("Url")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("CloudPayments не вернул ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: None,
            expires_at: None,
            payload: serde_json::json!({
                "order_id": order.get("Id").and_then(Json::as_str),
                "number": order.get("Number").and_then(Json::scalar_text),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let secret = config.require("api_secret")?;
        let kind = request
            .route_suffix
            .as_deref()
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| util::bad("в адресе уведомления нет его типа"))?;
        if !hmac_valid(secret, &request.body, request.header(HMAC_HEADER)) {
            return Err(PaymentError::BadSignature);
        }
        let fields = notification_fields(&request.body)?;
        let transaction = field(&fields, "TransactionId").unwrap_or_default();
        let event_key = format!("{kind}:{transaction}");
        let test_matches = is_test(&fields) == config.flag("test_mode");
        let payment = util::public_ref(field(&fields, "InvoiceId"));
        match kind.as_str() {
            "check" => Ok(util::skipped(
                event_key,
                payment,
                None,
                reply(check_code(&fields, test_matches)),
            )),
            "pay" if test_matches => {
                let status = match field(&fields, "Status") {
                    Some("Completed") => ProviderStatus::Paid,
                    _ => ProviderStatus::Pending,
                };
                if transaction.is_empty() {
                    return Err(util::bad("нет TransactionId"));
                }
                Ok(WebhookOutcome {
                    event_key,
                    payment,
                    provider_txid: Some(transaction.to_string()),
                    status,
                    amount: Some(amount(&fields, "Amount")?),
                    saved_method: None,
                    reply: reply(0),
                })
            }
            "refund" if test_matches => {
                let original = field(&fields, "PaymentTransactionId")
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| util::bad("нет PaymentTransactionId"))?;
                let refunded = amount(&fields, "Amount")?;
                Ok(WebhookOutcome {
                    event_key,
                    payment: payment
                        .or_else(|| Some(PaymentRef::ProviderTxid(original.to_string()))),
                    provider_txid: Some(original.to_string()),
                    status: ProviderStatus::PartiallyRefunded { refunded },
                    amount: Some(refunded),
                    saved_method: None,
                    reply: reply(0),
                })
            }
            _ => Ok(util::skipped(event_key, None, None, reply(0))),
        }
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let model = if let Some(transaction) = transaction_number(external_id) {
            self.call(
                config,
                "payments/get",
                &Json::object([("TransactionId", Json::from(transaction))]),
                None,
            )
            .await?
        } else {
            let invoice = Uuid::parse_str(external_id)
                .map_err(|_| util::bad("ожидался номер транзакции или наш id платежа"))?;
            self.call(
                config,
                "v2/payments/find",
                &Json::object([("InvoiceId", Json::from(invoice.to_string()))]),
                None,
            )
            .await?
        };
        model.as_ref().map(transaction_outcome).transpose()
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let transaction = transaction_number(&request.provider_txid).ok_or_else(|| {
            PaymentError::Rejected("у платежа нет номера транзакции CloudPayments".into())
        })?;
        let model = self
            .call(
                config,
                "payments/refund",
                &Json::object([
                    ("TransactionId", Json::from(transaction)),
                    ("Amount", Json::number(request.amount.units())),
                ]),
                Some(&util::idempotency_key(&[
                    "refund",
                    &request.payment_id.to_string(),
                    &transaction.to_string(),
                    &request.amount.minor.to_string(),
                    request.reason.as_deref().unwrap_or_default(),
                ])),
            )
            .await?
            .ok_or_else(|| {
                PaymentError::Rejected("транзакция не найдена в CloudPayments".into())
            })?;
        Ok(RefundOutcome {
            provider_refund_id: model
                .get("TransactionId")
                .and_then(Json::scalar_text)
                .map(str::to_string),
            completed: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;
    use sn_domain::ids::PaymentId;
    use sn_domain::payment::{AmountCheck, check_paid_amount};
    use wiremock::matchers::{basic_auth, body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SECRET: &str = "cp-api-secret";

    fn settings() -> ProviderConfig {
        config(&[
            ("public_id", json!("pk_8f3b2c1d0e")),
            ("api_secret", json!(SECRET)),
        ])
    }

    fn sign(body: &str, secret: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        STANDARD.encode(mac.finalize().into_bytes())
    }

    fn pay_form(amount: &str, currency: &str, test: &str) -> String {
        format!(
            "TransactionId=1504&Amount={amount}&Currency={currency}&PaymentAmount={amount}&PaymentCurrency={currency}&DateTime=2026-11-16+14%3A52%3A00&CardFirstSix=424242&CardLastFour=4242&CardType=Visa&TestMode={test}&Status=Completed&OperationType=Payment&InvoiceId={PAYMENT}&AccountId={CLIENT}&Email=client%40example.com&Token=tk_secret"
        )
    }

    async fn parse(
        kind: &str,
        body: &str,
        signature: Option<String>,
    ) -> PaymentResult<WebhookOutcome> {
        let headers: Vec<(&str, String)> = signature
            .map(|s| vec![("Content-HMAC", s)])
            .unwrap_or_default();
        let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let mut request = hook(&headers, body.to_string());
        request.route_suffix = Some(kind.into());
        CloudPayments::new()
            .parse_webhook(&settings(), &request)
            .await
    }

    #[tokio::test]
    async fn content_hmac_по_сырому_телу() {
        let body = pay_form("499.00", "RUB", "0");
        let outcome = parse("pay", &body, Some(sign(&body, SECRET)))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(rub(49_900)));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.provider_txid.as_deref(), Some("1504"));
        assert_eq!(outcome.reply, reply(0));
        assert_eq!(outcome.saved_method, None);
        assert_eq!(
            parse("pay", &body, Some(sign(&body, "other"))).await,
            Err(PaymentError::BadSignature)
        );
        assert_eq!(
            parse("pay", &body, None).await,
            Err(PaymentError::BadSignature)
        );
        assert_eq!(
            parse("pay", &body, Some("not base64!".into())).await,
            Err(PaymentError::BadSignature)
        );
    }

    #[tokio::test]
    async fn уведомление_в_json() {
        let body = json!({
            "TransactionId": 1504, "Amount": 499.00, "Currency": "RUB", "TestMode": false,
            "Status": "Completed", "OperationType": "Payment", "InvoiceId": PAYMENT.to_string()
        })
        .to_string();
        let outcome = parse("pay", &body, Some(sign(&body, SECRET)))
            .await
            .unwrap();
        assert_eq!(outcome.amount, Some(rub(49_900)));
        assert_eq!(outcome.provider_txid.as_deref(), Some("1504"));
    }

    #[tokio::test]
    async fn коды_ответа_на_check() {
        let check = |body: String| async move {
            let signature = sign(&body, SECRET);
            parse("check", &body, Some(signature)).await.unwrap()
        };
        let ok = check(pay_form("499.00", "RUB", "0")).await;
        assert_eq!(ok.reply, reply(0));
        assert_eq!(ok.status, ProviderStatus::Pending);
        assert_eq!(check(pay_form("499.00", "USD", "0")).await.reply, reply(12));
        assert_eq!(
            check(pay_form("499.001", "RUB", "0")).await.reply,
            reply(12)
        );
        assert_eq!(check(pay_form("499.00", "RUB", "1")).await.reply, reply(13));
        let foreign = pay_form("499.00", "RUB", "0").replace(&PAYMENT.to_string(), "order-77");
        assert_eq!(check(foreign).await.reply, reply(10));
    }

    #[tokio::test]
    async fn статусы_и_типы_уведомлений_cloudpayments() {
        let authorized =
            pay_form("499.00", "RUB", "0").replace("Status=Completed", "Status=Authorized");
        let outcome = parse("pay", &authorized, Some(sign(&authorized, SECRET)))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        let test_payment = pay_form("499.00", "RUB", "1");
        let outcome = parse("pay", &test_payment, Some(sign(&test_payment, SECRET)))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        assert_eq!(outcome.payment, None);
        let fail = format!(
            "TransactionId=1505&Amount=499.00&Currency=RUB&TestMode=0&OperationType=Payment&InvoiceId={PAYMENT}&Reason=InsufficientFunds&ReasonCode=5051"
        );
        let outcome = parse("fail", &fail, Some(sign(&fail, SECRET)))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        assert_eq!(outcome.provider_txid, None);
        assert_eq!(outcome.reply, reply(0));
        let refund = format!(
            "TransactionId=1600&PaymentTransactionId=1504&Amount=100.00&Currency=RUB&TestMode=0&OperationType=Refund&InvoiceId={PAYMENT}"
        );
        let outcome = parse("refund", &refund, Some(sign(&refund, SECRET)))
            .await
            .unwrap();
        assert_eq!(
            outcome.status,
            ProviderStatus::PartiallyRefunded {
                refunded: rub(10_000)
            }
        );
        assert_eq!(outcome.provider_txid.as_deref(), Some("1504"));
        assert_eq!(outcome.event_key, "refund:1600");
        let mut no_kind = hook(&[], refund.clone());
        no_kind.route_suffix = None;
        assert!(matches!(
            CloudPayments::new()
                .parse_webhook(&settings(), &no_kind)
                .await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn недоплата_и_чужая_валюта_cloudpayments() {
        let expected = rub(49_900);
        let under = pay_form("498.99", "RUB", "0");
        let outcome = parse("pay", &under, Some(sign(&under, SECRET)))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, outcome.amount),
            AmountCheck::Underpaid { .. }
        ));
        let usd_pay = pay_form("499.00", "USD", "0");
        let outcome = parse("pay", &usd_pay, Some(sign(&usd_pay, SECRET)))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, outcome.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
        let euro = pay_form("499.00", "EUR", "0");
        assert!(matches!(
            parse("pay", &euro, Some(sign(&euro, SECRET))).await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn счёт_cloudpayments_через_orders_create() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/orders/create"))
            .and(basic_auth("pk_8f3b2c1d0e", SECRET))
            .and(header("X-Request-ID", PAYMENT.to_string().as_str()))
            .and(body_json(json!({
                "Amount": 499.0, "Currency": "RUB", "Description": "Стандарт · 3 месяца",
                "InvoiceId": PAYMENT.to_string(), "AccountId": CLIENT.to_string(),
                "RequireConfirmation": false, "SendEmail": false, "CultureName": "ru-RU",
                "Email": "client@example.com",
                "SuccessRedirectUrl": "https://panel.example.com/cabinet/payment-wait",
                "FailRedirectUrl": "https://panel.example.com/cabinet/payment-wait"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Model": {
                    "Id": "f2K8LV6reGE9WBFn", "Number": 61, "Amount": 499.0, "Currency": "RUB",
                    "Url": "https://orders.cloudpayments.ru/d/f2K8LV6reGE9WBFn", "Status": "Created"
                },
                "Success": true, "Message": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        let created = CloudPayments::with_api_base(server.uri())
            .create_invoice(
                &settings(),
                &invoice(rub(49_900), Some("client@example.com")),
            )
            .await
            .unwrap();
        assert_eq!(
            created.pay_url,
            "https://orders.cloudpayments.ru/d/f2K8LV6reGE9WBFn"
        );
        assert_eq!(created.external_id, None);
        assert_eq!(created.payload["order_id"], "f2K8LV6reGE9WBFn");
    }

    #[tokio::test]
    async fn отказ_cloudpayments_при_создании_счёта() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/orders/create"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Success": false, "Message": "Amount is required"
            })))
            .mount(&server)
            .await;
        let result = CloudPayments::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(rub(49_900), None))
            .await;
        assert_eq!(
            result,
            Err(PaymentError::Rejected("Amount is required".into()))
        );
    }

    #[tokio::test]
    async fn статус_и_возврат_cloudpayments() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/payments/find"))
            .and(body_json(json!({"InvoiceId": PAYMENT.to_string()})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Model": {"TransactionId": 1504, "Amount": 499.00, "Currency": "RUB",
                          "Status": "Completed", "InvoiceId": PAYMENT.to_string()},
                "Success": true, "Message": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payments/get"))
            .and(body_json(json!({"TransactionId": 9})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Success": false, "Message": "Not found"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payments/refund"))
            .and(body_json(json!({"TransactionId": 1504, "Amount": 100.0})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Model": {"TransactionId": 1601}, "Success": true, "Message": null
            })))
            .mount(&server)
            .await;
        let cloud = CloudPayments::with_api_base(server.uri());
        let found = cloud
            .fetch_status(&settings(), &PAYMENT.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, ProviderStatus::Paid);
        assert_eq!(found.provider_txid.as_deref(), Some("1504"));
        assert_eq!(found.amount, Some(rub(49_900)));
        assert_eq!(cloud.fetch_status(&settings(), "009").await, Ok(None));
        let refund = cloud
            .refund(
                &settings(),
                &RefundRequest {
                    payment_id: PaymentId(41),
                    provider_txid: "1504".into(),
                    amount: rub(10_000),
                    reason: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(refund.provider_refund_id.as_deref(), Some("1601"));
    }

    #[test]
    fn настройки_cloudpayments() {
        assert!(CloudPayments::new().is_configured(&settings()));
        assert!(!CloudPayments::new().is_configured(&config(&[
            ("public_id", json!("test_api_00000000000000000000002")),
            ("api_secret", json!(SECRET))
        ])));
    }
}
