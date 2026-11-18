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
const DECODED_HMAC_HEADER: &str = "x-content-hmac";
const PAGE: &str = "page";
const SBP: &str = "sbp";
const MAX_TTL_MINUTES: i64 = 43_200;
const MAX_REDIRECT_LEN: usize = 1024;

const HOOKS: [(&str, &str); 4] = [
    ("check", "Check"),
    ("pay", "Pay"),
    ("fail", "Fail"),
    ("refund", "Refund"),
];

const METHODS: [PayMethod; 2] = [
    PayMethod::new(
        PAGE,
        "Все способы",
        "Карта и другие способы на странице CloudPayments",
    ),
    PayMethod::new(SBP, "СБП", "Оплата в приложении банка"),
];

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

    async fn transaction(&self, config: &ProviderConfig, id: i64) -> PaymentResult<Option<Json>> {
        self.call(
            config,
            "payments/get",
            &Json::object([("TransactionId", Json::from(id))]),
            None,
        )
        .await
    }

    async fn order(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
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

    async fn sbp_link(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        let mut body = Json::object([
            ("PublicId", Json::from(config.require("public_id")?)),
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
            ("Scheme", Json::from("charge")),
            (
                "TtlMinutes",
                Json::from(request.expires_in.num_minutes().clamp(1, MAX_TTL_MINUTES)),
            ),
        ]);
        if let Some(email) = util::client_email(request) {
            body.push("Email", Json::from(email));
        }
        if let Some(url) = util::return_url(request)
            .filter(|u| util::is_https(u) && u.is_ascii() && u.len() <= MAX_REDIRECT_LEN)
        {
            body.push("SuccessRedirectUrl", Json::from(url));
        }
        let link = self
            .call(
                config,
                "payments/qr/sbp/link",
                &body,
                Some(&request.payment_public_id.to_string()),
            )
            .await?
            .ok_or_else(|| PaymentError::Rejected("CloudPayments не выдал ссылку СБП".into()))?;
        let pay_url = link
            .get("QrUrl")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("CloudPayments не вернул ссылку на оплату"))?;
        let transaction = link
            .get("TransactionId")
            .and_then(Json::scalar_text)
            .filter(|id| transaction_number(id).is_some())
            .ok_or_else(|| util::bad("CloudPayments не вернул номер транзакции"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(transaction.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "method": SBP,
                "transaction_id": transaction,
                "qr_id": link.get("ProviderQrId").and_then(Json::as_str),
            }),
        })
    }
}

pub fn signature(secret: &str, message: &[u8]) -> String {
    match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(mut mac) => {
            mac.update(message);
            STANDARD.encode(mac.finalize().into_bytes())
        }
        Err(_) => String::new(),
    }
}

fn hook_check(label: &str, model: &Json, expected: &str) -> HookCheck {
    let enabled = model.get("IsEnabled").and_then(Json::as_bool) == Some(true);
    let address = model
        .get("Address")
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|a| !a.is_empty());
    let off = |address: Option<&str>| HookCheck {
        label: label.to_string(),
        state: HookState::Off,
        address: address.map(hook_address),
        note: None,
    };
    let Some(address) = address.filter(|_| enabled) else {
        return off(address);
    };
    if !same_hook(expected, address) {
        return HookCheck {
            label: label.to_string(),
            state: HookState::Other,
            address: Some(hook_address(address)),
            note: None,
        };
    }
    let mut notes = Vec::new();
    if let Some(format) = model
        .get("Format")
        .and_then(Json::as_str)
        .filter(|f| !f.eq_ignore_ascii_case("CloudPayments"))
    {
        notes.push(format!(
            "формат {}, нужен CloudPayments",
            util::clip(format, 24)
        ));
    }
    if let Some(encoding) = model
        .get("Encoding")
        .and_then(Json::as_str)
        .filter(|e| !e.eq_ignore_ascii_case("UTF8"))
    {
        notes.push(format!(
            "кодировка {}, нужна UTF-8",
            util::clip(encoding, 24)
        ));
    }
    HookCheck {
        label: label.to_string(),
        state: HookState::Same,
        address: None,
        note: (!notes.is_empty()).then(|| notes.join(", ")),
    }
}

fn reply(code: i64) -> WebhookReply {
    WebhookReply::Json {
        status: 200,
        body: serde_json::json!({ "code": code }),
    }
}

fn hmac_valid(secret: &str, message: &[u8], header: &str) -> bool {
    let Ok(expected) = STANDARD.decode(header.trim()) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(message);
    mac.verify_slice(&expected).is_ok()
}

fn is_json(message: &[u8]) -> bool {
    message.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{')
}

fn decoded(message: &[u8]) -> Option<String> {
    if is_json(message) {
        return None;
    }
    let pairs = util::form_pairs(message).ok()?;
    if pairs
        .iter()
        .any(|(key, value)| key.contains(['&', '=']) || value.contains('&'))
    {
        return None;
    }
    Some(
        pairs
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&"),
    )
}

fn signed(secret: &str, message: &[u8], request: &WebhookRequest) -> bool {
    let encoded = request.header(HMAC_HEADER);
    let plain = request.header(DECODED_HMAC_HEADER);
    if encoded
        .into_iter()
        .chain(plain)
        .any(|header| hmac_valid(secret, message, header))
    {
        return true;
    }
    plain.is_some_and(|header| {
        decoded(message).is_some_and(|text| hmac_valid(secret, text.as_bytes(), header))
    })
}

fn message(request: &WebhookRequest) -> &[u8] {
    if request.body.is_empty() {
        request.query.as_deref().unwrap_or_default().as_bytes()
    } else {
        &request.body
    }
}

fn notification_fields(message: &[u8]) -> PaymentResult<Vec<(String, String)>> {
    if !is_json(message) {
        return util::form_pairs(message);
    }
    match Json::parse(message) {
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

fn truthy(value: &str) -> bool {
    value == "1" || value.eq_ignore_ascii_case("true")
}

fn is_test(fields: &[(String, String)]) -> bool {
    field(fields, "TestMode").is_some_and(truthy)
}

fn model_test(model: &Json) -> bool {
    match model.get("TestMode") {
        Some(Json::Bool(test)) => *test,
        Some(other) => other.scalar_text().is_some_and(truthy),
        None => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    Payment,
    Refund,
    Other,
}

fn operation_named(name: &str) -> Operation {
    match name {
        "Payment" => Operation::Payment,
        "Refund" => Operation::Refund,
        _ => Operation::Other,
    }
}

fn notified_operation(fields: &[(String, String)]) -> Option<Operation> {
    field(fields, "OperationType").map(operation_named)
}

fn model_operation(model: &Json) -> Option<Operation> {
    if let Some(name) = model.get("OperationType").and_then(Json::as_str) {
        return Some(operation_named(name));
    }
    model
        .get("Type")
        .and_then(Json::scalar_text)
        .map(|code| match code {
            "0" => Operation::Payment,
            "1" => Operation::Refund,
            _ => Operation::Other,
        })
}

fn amount(fields: &[(String, String)], amount_key: &str) -> PaymentResult<Amount> {
    let currency = util::pay_currency(field(fields, "Currency").unwrap_or("RUB"))?;
    let raw = field(fields, amount_key).ok_or_else(|| util::bad("нет суммы"))?;
    Ok(Amount::new(util::minor(raw, currency)?, currency))
}

fn model_amount(model: &Json) -> PaymentResult<Amount> {
    let currency = model.get("Currency").cloned().unwrap_or(Json::from("RUB"));
    util::json_amount(model.get("Amount"), Some(&currency))
}

fn check_code(fields: &[(String, String)], test_mode: bool) -> i64 {
    match notified_operation(fields) {
        Some(Operation::Refund) => return 0,
        Some(Operation::Other) => return 13,
        Some(Operation::Payment) | None => {}
    }
    if is_test(fields) != test_mode {
        return 13;
    }
    if util::public_ref(field(fields, "InvoiceId")).is_none() {
        return 10;
    }
    if field(fields, "AccountId")
        .map(str::trim)
        .is_some_and(|id| !id.is_empty() && Uuid::parse_str(id).is_err())
    {
        return 11;
    }
    match amount(fields, "Amount") {
        Ok(paid) if paid.currency == PayCurrency::Rub && paid.minor > 0 => 0,
        _ => 12,
    }
}

fn original_transaction(fields: &[(String, String)]) -> Option<String> {
    field(fields, "PaymentTransactionId")
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
}

fn refund_notice(
    fields: &[(String, String)],
    event_key: String,
    payment: Option<PaymentRef>,
) -> PaymentResult<WebhookOutcome> {
    let original =
        original_transaction(fields).ok_or_else(|| util::bad("нет PaymentTransactionId"))?;
    let refunded = amount(fields, "Amount")?;
    let payment = payment.or_else(|| Some(PaymentRef::ProviderTxid(original.clone())));
    let status = match field(fields, "Status") {
        None | Some("Completed") => ProviderStatus::PartiallyRefunded { refunded },
        Some("Declined" | "Cancelled") => ProviderStatus::RefundDeclined,
        Some(_) => return Ok(util::skipped(event_key, payment, Some(original), reply(0))),
    };
    Ok(WebhookOutcome {
        event_key,
        payment,
        provider_txid: Some(original),
        status,
        amount: Some(refunded),
        saved_method: None,
        reply: reply(0),
        signal: None,
    })
}

fn refund_failure(
    fields: &[(String, String)],
    transaction: &str,
    payment: Option<PaymentRef>,
) -> WebhookOutcome {
    let original = original_transaction(fields);
    WebhookOutcome {
        event_key: format!("fail:refund:{transaction}"),
        payment: payment.or_else(|| original.clone().map(PaymentRef::ProviderTxid)),
        provider_txid: original,
        status: ProviderStatus::RefundDeclined,
        amount: amount(fields, "Amount").ok(),
        saved_method: None,
        reply: reply(0),
        signal: None,
    }
}

fn payment_outcome(model: &Json, test_mode: bool) -> PaymentResult<Option<WebhookOutcome>> {
    if model_operation(model).is_some_and(|op| op != Operation::Payment)
        || model_test(model) != test_mode
    {
        return Ok(None);
    }
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
        "Declined" => ProviderStatus::Failed,
        "Cancelled" => ProviderStatus::Canceled,
        _ => ProviderStatus::Pending,
    };
    Ok(Some(WebhookOutcome {
        event_key: format!("transaction.{status_text}:{id}"),
        payment: util::public_ref(model.get("InvoiceId").and_then(Json::as_str))
            .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string()))),
        provider_txid: Some(id.to_string()),
        status,
        amount: Some(model_amount(model)?),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    }))
}

fn refund_outcome(model: &Json, refund_id: &str) -> PaymentResult<WebhookOutcome> {
    if model_operation(model).is_some_and(|op| op != Operation::Refund) {
        return Err(util::bad("транзакция CloudPayments не возврат"));
    }
    let event_key = format!("refund:{refund_id}");
    let original = model
        .get("OriginalTransactionId")
        .and_then(Json::scalar_text)
        .filter(|id| transaction_number(id).is_some())
        .map(str::to_string);
    let payment = util::public_ref(model.get("InvoiceId").and_then(Json::as_str))
        .or_else(|| original.clone().map(PaymentRef::ProviderTxid));
    let refunded = model_amount(model)?;
    let status = match model.get("Status").and_then(Json::as_str) {
        Some("Completed") => ProviderStatus::PartiallyRefunded { refunded },
        Some("Declined" | "Cancelled") => ProviderStatus::RefundDeclined,
        _ => {
            return Ok(util::skipped(
                event_key,
                payment,
                original,
                WebhookReply::ok(),
            ));
        }
    };
    Ok(WebhookOutcome {
        event_key,
        payment,
        provider_txid: original,
        status,
        amount: Some(refunded),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
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

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: "Карты российских банков",
        }
    }

    fn methods(&self, currency: Currency) -> Vec<PayMethod> {
        if currency == Currency::Rub {
            METHODS.to_vec()
        } else {
            Vec::new()
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

    fn supports_refund_polling(&self) -> bool {
        true
    }

    fn polls_by_payment_id(&self) -> bool {
        true
    }

    fn precheck_reply(&self, verdict: PrecheckVerdict) -> WebhookReply {
        reply(match verdict {
            PrecheckVerdict::Accept => 0,
            PrecheckVerdict::UnknownInvoice => 10,
            PrecheckVerdict::WrongAccount => 11,
            PrecheckVerdict::WrongAmount => 12,
            PrecheckVerdict::Rejected => 13,
            PrecheckVerdict::Expired => 20,
        })
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        match request.method.as_deref() {
            None | Some(PAGE) => self.order(config, request).await,
            Some(SBP) => self.sbp_link(config, request).await,
            Some(_) => Err(PaymentError::Rejected(
                "этот способ оплаты недоступен".into(),
            )),
        }
    }

    async fn cancel_invoice(
        &self,
        config: &ProviderConfig,
        invoice: &IssuedInvoice<'_>,
    ) -> PaymentResult<()> {
        let order = invoice
            .payload
            .get("order_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| PaymentError::BadPayload("нет номера счёта CloudPayments".into()))?;
        self.call(
            config,
            "orders/cancel",
            &Json::object([("Id", Json::from(order))]),
            None,
        )
        .await?;
        Ok(())
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
        let message = message(request);
        if !signed(secret, message, request) {
            return Err(PaymentError::BadSignature);
        }
        let fields = notification_fields(message)?;
        let transaction = field(&fields, "TransactionId").unwrap_or_default();
        let event_key = format!("{kind}:{transaction}");
        let test_mode = config.flag("test_mode");
        let payment = util::public_ref(field(&fields, "InvoiceId"));
        let operation = notified_operation(&fields);
        match kind.as_str() {
            "check" => {
                let code = check_code(&fields, test_mode);
                let mut outcome = util::skipped(event_key, payment, None, reply(code));
                if code == 0 && operation != Some(Operation::Refund) {
                    outcome.amount = amount(&fields, "Amount").ok();
                    outcome.signal = Some(OutcomeSignal::Precheck {
                        account: field(&fields, "AccountId")
                            .map(str::trim)
                            .filter(|id| !id.is_empty())
                            .map(str::to_string),
                    });
                }
                Ok(outcome)
            }
            "pay"
                if is_test(&fields) == test_mode
                    && operation.is_none_or(|op| op == Operation::Payment) =>
            {
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
                    signal: None,
                })
            }
            "refund" if operation.is_none_or(|op| op == Operation::Refund) => {
                refund_notice(&fields, event_key, payment)
            }
            "fail" if operation == Some(Operation::Refund) => {
                Ok(refund_failure(&fields, transaction, payment))
            }
            _ => Ok(util::skipped(event_key, None, None, reply(0))),
        }
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let test_mode = config.flag("test_mode");
        if let Some(transaction) = transaction_number(external_id) {
            return match self.transaction(config, transaction).await? {
                Some(model) => payment_outcome(&model, test_mode),
                None => Ok(None),
            };
        }
        let invoice = Uuid::parse_str(external_id)
            .map_err(|_| util::bad("ожидался номер транзакции или наш id платежа"))?;
        let found = self
            .call(
                config,
                "v2/payments/find",
                &Json::object([("InvoiceId", Json::from(invoice.to_string()))]),
                None,
            )
            .await?;
        match found {
            Some(model)
                if util::public_ref(model.get("InvoiceId").and_then(Json::as_str))
                    == Some(PaymentRef::PublicId(invoice)) =>
            {
                payment_outcome(&model, test_mode)
            }
            _ => Ok(None),
        }
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
                Some(&request.key.to_string()),
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
            completed: false,
        })
    }

    async fn fetch_refund(
        &self,
        config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let Some(transaction) = transaction_number(provider_refund_id) else {
            return Ok(None);
        };
        let model = self
            .transaction(config, transaction)
            .await?
            .ok_or_else(|| PaymentError::Rejected("возврат не найден в CloudPayments".into()))?;
        refund_outcome(&model, provider_refund_id.trim()).map(Some)
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        match self
            .call(config, "test", &Json::Object(Vec::new()), None)
            .await
        {
            Ok(_) => {}
            Err(err) if util::keys_refused(&err) => {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "CloudPayments не принял Public ID и пароль для API",
                )
                .detail("Оба значения берутся в кабинете CloudPayments → Сайты → настройки сайта. Пароль для API не совпадает с паролем от кабинета");
            }
            Err(err) => return ConnectionCheck::failed(&err),
        }
        let mut check = ConnectionCheck::ok("Public ID и пароль для API приняты CloudPayments");
        if config.flag("test_mode") {
            check = check.detail(
                "Включён тестовый режим: засчитываются только тестовые оплаты, настоящие панель не примет",
            );
        }
        let Some(base) = context.webhook_url.as_deref() else {
            return check.detail(
                "Адрес панели не задан в основных настройках, поэтому адреса уведомлений не сравнивались",
            );
        };
        for (kind, label) in HOOKS {
            match self
                .call(
                    config,
                    &format!("site/notifications/{kind}/get"),
                    &Json::Object(Vec::new()),
                    None,
                )
                .await
            {
                Ok(Some(model)) => {
                    check
                        .hooks
                        .push(hook_check(label, &model, &format!("{base}/{kind}")));
                }
                Ok(None) => check.hooks.push(HookCheck {
                    label: label.to_string(),
                    state: HookState::Off,
                    address: None,
                    note: None,
                }),
                Err(err) => {
                    check.hooks.clear();
                    return check.detail(format!(
                        "Настройки уведомлений CloudPayments не получены: {}",
                        ConnectionCheck::failed(&err).summary
                    ));
                }
            }
        }
        check
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use bytes::Bytes;
    use serde_json::json;
    use sn_domain::ids::PaymentId;
    use sn_domain::payment::{AmountCheck, check_paid_amount};
    use wiremock::matchers::{basic_auth, body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SECRET: &str = "cp-api-secret";

    fn notifications(kind: &str, model: serde_json::Value) -> Mock {
        Mock::given(method("POST"))
            .and(path(format!("/site/notifications/{kind}/get")))
            .and(basic_auth("pk_8f3b2c1d0e", SECRET))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Model": model, "Success": true, "Message": null
            })))
    }

    fn hook_model(enabled: bool, address: &str, encoding: &str) -> serde_json::Value {
        json!({
            "IsEnabled": enabled, "Address": address, "HttpMethod": "POST",
            "Encoding": encoding, "Format": "CloudPayments"
        })
    }

    #[tokio::test]
    async fn проверка_подключения_cloudpayments_и_адресов_уведомлений() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/test"))
            .and(basic_auth("pk_8f3b2c1d0e", SECRET))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Success": true, "Message": "bd6353c3-0ed6-4a65-946f-083664bf8dbd"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "Success": false, "Message": "Authentication failed.", "ErrorCode": null
            })))
            .with_priority(10)
            .mount(&server)
            .await;
        let base = "https://panel.example.com/api/webhooks/pay/cloudpayments";
        notifications("check", hook_model(true, &format!("{base}/check"), "UTF8"))
            .mount(&server)
            .await;
        notifications(
            "pay",
            hook_model(true, &format!("{base}/pay/"), "Windows1251"),
        )
        .mount(&server)
        .await;
        notifications("fail", hook_model(false, "", "UTF8"))
            .mount(&server)
            .await;
        notifications(
            "refund",
            hook_model(true, "https://old.example.com/cp/refund?key=1", "UTF8"),
        )
        .mount(&server)
        .await;
        let provider = CloudPayments::with_api_base(server.uri());
        let context = CheckContext {
            webhook_url: Some(base.into()),
        };
        let ok = provider.check_connection(&settings(), &context).await;
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
        let states: Vec<(&str, HookState)> = ok
            .hooks
            .iter()
            .map(|h| (h.label.as_str(), h.state))
            .collect();
        assert_eq!(
            states,
            [
                ("Check", HookState::Same),
                ("Pay", HookState::Same),
                ("Fail", HookState::Off),
                ("Refund", HookState::Other)
            ]
        );
        assert_eq!(ok.hooks[0].note, None);
        assert_eq!(
            ok.hooks[1].note.as_deref(),
            Some("кодировка Windows1251, нужна UTF-8")
        );
        assert_eq!(
            ok.hooks[3].address.as_deref(),
            Some("https://old.example.com/cp/refund")
        );
        let mut test_mode = settings();
        test_mode.set("test_mode", json!(true));
        let without_url = provider
            .check_connection(&test_mode, &CheckContext::default())
            .await;
        assert_eq!(without_url.status, CheckStatus::Ok);
        assert!(without_url.hooks.is_empty());
        assert_eq!(without_url.details.len(), 2);
        let mut wrong = settings();
        wrong.set("api_secret", json!("wrong-secret"));
        let rejected = provider.check_connection(&wrong, &context).await;
        assert_eq!(rejected.status, CheckStatus::KeysRejected);
        assert!(rejected.hooks.is_empty());
        let down = crate::test_support::failing_api(503).await;
        let unreachable = CloudPayments::with_api_base(down.uri())
            .check_connection(&settings(), &context)
            .await;
        assert_eq!(unreachable.status, CheckStatus::Unreachable);
        let closed = CloudPayments::with_api_base(crate::test_support::closed_api())
            .check_connection(&settings(), &context)
            .await;
        assert_eq!(closed.status, CheckStatus::Unreachable);
    }

    fn settings() -> ProviderConfig {
        config(&[
            ("public_id", json!("pk_8f3b2c1d0e")),
            ("api_secret", json!(SECRET)),
        ])
    }

    fn test_settings() -> ProviderConfig {
        let mut config = settings();
        config.set("test_mode", json!(true));
        config
    }

    fn sign(body: &str, secret: &str) -> String {
        signature(secret, body.as_bytes())
    }

    fn pay_form(amount: &str, currency: &str, test: &str) -> String {
        format!(
            "TransactionId=1504&Amount={amount}&Currency={currency}&PaymentAmount={amount}&PaymentCurrency={currency}&DateTime=2026-11-16+14%3A52%3A00&CardFirstSix=424242&CardLastFour=4242&CardType=Visa&TestMode={test}&Status=Completed&OperationType=Payment&InvoiceId={PAYMENT}&AccountId={CLIENT}&Email=client%40example.com&Token=tk_secret"
        )
    }

    fn plain(form: &str) -> String {
        form.replace('+', " ")
            .replace("%3A", ":")
            .replace("%40", "@")
    }

    fn signed_hook(header_name: &str, signed_text: &str, body: &str) -> WebhookRequest {
        let signature = sign(signed_text, SECRET);
        hook(&[(header_name, signature.as_str())], body.to_string())
    }

    async fn deliver(
        config: &ProviderConfig,
        kind: &str,
        mut request: WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        request.route_suffix = Some(kind.into());
        CloudPayments::new().parse_webhook(config, &request).await
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
        deliver(&settings(), kind, hook(&headers, body.to_string())).await
    }

    fn model(value: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "Model": value, "Success": true, "Message": null
        }))
    }

    fn not_found() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"Success": false, "Message": "Not found"}))
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
        assert_eq!(
            parse("pay", &body, Some(sign(&plain(&body), SECRET))).await,
            Err(PaymentError::BadSignature)
        );
    }

    #[tokio::test]
    async fn x_content_hmac_по_раскодированным_параметрам() {
        let body = pay_form("499.00", "RUB", "0");
        let decoded = deliver(
            &settings(),
            "pay",
            signed_hook("X-Content-HMAC", &plain(&body), &body),
        )
        .await
        .unwrap();
        assert_eq!(decoded.status, ProviderStatus::Paid);
        assert_eq!(decoded.amount, Some(rub(49_900)));
        let raw = deliver(
            &settings(),
            "pay",
            signed_hook("X-Content-HMAC", &body, &body),
        )
        .await
        .unwrap();
        assert_eq!(raw.status, ProviderStatus::Paid);
        let mut foreign = signed_hook("X-Content-HMAC", &plain(&body), &body);
        foreign.headers = vec![("X-Content-HMAC".into(), sign(&plain(&body), "other"))];
        assert_eq!(
            deliver(&settings(), "pay", foreign).await,
            Err(PaymentError::BadSignature)
        );
        let json_body =
            json!({"TransactionId": 1504, "Amount": "1+1%", "Currency": "RUB"}).to_string();
        let json_hook = signed_hook("X-Content-HMAC", &json_body, &json_body);
        assert!(deliver(&settings(), "check", json_hook).await.is_ok());
    }

    #[tokio::test]
    async fn x_content_hmac_не_даёт_переставить_границы_полей() {
        let genuine = pay_form("499.00", "RUB", "1");
        let ignored = deliver(
            &settings(),
            "pay",
            signed_hook("X-Content-HMAC", &plain(&genuine), &genuine),
        )
        .await
        .unwrap();
        assert_eq!(ignored.status, ProviderStatus::Pending);
        assert_eq!(ignored.payment, None);
        let merged = genuine.replace("CardType=Visa&TestMode=1", "CardType=Visa%26TestMode%3D1");
        assert_eq!(
            plain(&merged).replace("%26", "&").replace("%3D", "="),
            plain(&genuine)
        );
        assert_eq!(
            deliver(
                &settings(),
                "pay",
                signed_hook("X-Content-HMAC", &plain(&genuine), &merged),
            )
            .await,
            Err(PaymentError::BadSignature)
        );
        let with_ampersand = genuine.replace("CardType=Visa", "CardType=Visa%26Co");
        let both = sign(&with_ampersand, SECRET);
        let request = hook(
            &[("Content-HMAC", both.as_str()), ("X-Content-HMAC", "AAAA")],
            with_ampersand,
        );
        assert!(deliver(&test_settings(), "pay", request).await.is_ok());
    }

    #[tokio::test]
    async fn уведомление_методом_get_по_строке_запроса() {
        let query = pay_form("499.00", "RUB", "0");
        let mut request = signed_hook("Content-HMAC", &query, "");
        request.query = Some(query.clone());
        let outcome = deliver(&settings(), "pay", request).await.unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(rub(49_900)));
        assert_eq!(outcome.provider_txid.as_deref(), Some("1504"));
        let mut decoded = signed_hook("X-Content-HMAC", &plain(&query), "");
        decoded.query = Some(query.clone());
        assert_eq!(
            deliver(&settings(), "pay", decoded).await.unwrap().status,
            ProviderStatus::Paid
        );
        let mut forged = signed_hook("Content-HMAC", &query, "");
        forged.query = Some(query.replace("Amount=499.00", "Amount=4999.00"));
        assert_eq!(
            deliver(&settings(), "pay", forged).await,
            Err(PaymentError::BadSignature)
        );
        assert_eq!(
            deliver(&settings(), "pay", hook(&[], Bytes::new())).await,
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
        let check = |config: ProviderConfig, body: String| async move {
            let signature = sign(&body, SECRET);
            deliver(
                &config,
                "check",
                hook(&[("Content-HMAC", signature.as_str())], body),
            )
            .await
            .unwrap()
        };
        let ok = check(settings(), pay_form("499.00", "RUB", "0")).await;
        assert_eq!(ok.reply, reply(0));
        assert_eq!(ok.status, ProviderStatus::Pending);
        assert_eq!(ok.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(ok.amount, Some(rub(49_900)));
        assert_eq!(
            ok.signal,
            Some(OutcomeSignal::Precheck {
                account: Some(CLIENT.to_string())
            })
        );
        let wrong = check(settings(), pay_form("499.00", "USD", "0")).await;
        assert_eq!(wrong.signal, None);
        let refund_check = check(
            settings(),
            pay_form("100.00", "RUB", "1").replace("OperationType=Payment", "OperationType=Refund"),
        )
        .await;
        assert_eq!(refund_check.signal, None);
        let anonymous_check = check(
            settings(),
            pay_form("499.00", "RUB", "0").replace(&format!("&AccountId={CLIENT}"), ""),
        )
        .await;
        assert_eq!(
            anonymous_check.signal,
            Some(OutcomeSignal::Precheck { account: None })
        );
        let code =
            |config: ProviderConfig, body: String| async move { check(config, body).await.reply };
        assert_eq!(
            code(settings(), pay_form("499.00", "USD", "0")).await,
            reply(12)
        );
        assert_eq!(
            code(settings(), pay_form("499.001", "RUB", "0")).await,
            reply(12)
        );
        assert_eq!(
            code(settings(), pay_form("0.00", "RUB", "0")).await,
            reply(12)
        );
        assert_eq!(
            code(settings(), pay_form("499.00", "RUB", "1")).await,
            reply(13)
        );
        assert_eq!(
            code(test_settings(), pay_form("499.00", "RUB", "1")).await,
            reply(0)
        );
        assert_eq!(
            code(test_settings(), pay_form("499.00", "RUB", "0")).await,
            reply(13)
        );
        let foreign = pay_form("499.00", "RUB", "0").replace(&PAYMENT.to_string(), "order-77");
        assert_eq!(code(settings(), foreign).await, reply(10));
        let stranger = pay_form("499.00", "RUB", "0").replace(&CLIENT.to_string(), "user_x");
        assert_eq!(code(settings(), stranger).await, reply(11));
        let anonymous = pay_form("499.00", "RUB", "0").replace(&format!("&AccountId={CLIENT}"), "");
        assert_eq!(code(settings(), anonymous).await, reply(0));
        let refund =
            pay_form("100.00", "RUB", "1").replace("OperationType=Payment", "OperationType=Refund");
        assert_eq!(code(settings(), refund).await, reply(0));
        let payout = pay_form("499.00", "RUB", "0")
            .replace("OperationType=Payment", "OperationType=CardPayout");
        assert_eq!(code(settings(), payout).await, reply(13));
    }

    #[test]
    fn сверка_check_с_базой_отвечает_кодами_cloudpayments() {
        let cp = CloudPayments::new();
        for (verdict, code) in [
            (PrecheckVerdict::Accept, 0),
            (PrecheckVerdict::UnknownInvoice, 10),
            (PrecheckVerdict::WrongAccount, 11),
            (PrecheckVerdict::WrongAmount, 12),
            (PrecheckVerdict::Rejected, 13),
            (PrecheckVerdict::Expired, 20),
        ] {
            assert_eq!(cp.precheck_reply(verdict), reply(code), "{verdict:?}");
        }
        assert!(cp.polls_by_payment_id());
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
        let payout = pay_form("499.00", "RUB", "0")
            .replace("OperationType=Payment", "OperationType=CardPayout");
        let outcome = parse("pay", &payout, Some(sign(&payout, SECRET)))
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
        assert_eq!(outcome.payment, None);
        assert_eq!(outcome.reply, reply(0));
        let mut no_kind = hook(&[], fail);
        no_kind.route_suffix = None;
        assert!(matches!(
            CloudPayments::new()
                .parse_webhook(&settings(), &no_kind)
                .await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn возврат_по_уведомлению_без_testmode_и_валюты() {
        let refund = format!(
            "TransactionId=1600&PaymentTransactionId=1504&Amount=100.00&DateTime=2026-11-16+15%3A00%3A00&OperationType=Refund&InvoiceId={PAYMENT}&AccountId={CLIENT}"
        );
        for config in [settings(), test_settings()] {
            let outcome = deliver(
                &config,
                "refund",
                signed_hook("Content-HMAC", &refund, &refund),
            )
            .await
            .unwrap();
            assert_eq!(
                outcome.status,
                ProviderStatus::PartiallyRefunded {
                    refunded: rub(10_000)
                }
            );
            assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
            assert_eq!(outcome.provider_txid.as_deref(), Some("1504"));
            assert_eq!(outcome.event_key, "refund:1600");
            assert_eq!(outcome.reply, reply(0));
        }
        let orphan = refund.replace(&format!("&InvoiceId={PAYMENT}"), "");
        let outcome = deliver(
            &settings(),
            "refund",
            signed_hook("Content-HMAC", &orphan, &orphan),
        )
        .await
        .unwrap();
        assert_eq!(
            outcome.payment,
            Some(PaymentRef::ProviderTxid("1504".into()))
        );
        let declined = format!("{refund}&Status=Declined");
        let outcome = deliver(
            &settings(),
            "refund",
            signed_hook("Content-HMAC", &declined, &declined),
        )
        .await
        .unwrap();
        assert_eq!(outcome.status, ProviderStatus::RefundDeclined);
        let payout = refund.replace("OperationType=Refund", "OperationType=CardPayout");
        let outcome = deliver(
            &settings(),
            "refund",
            signed_hook("Content-HMAC", &payout, &payout),
        )
        .await
        .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        assert_eq!(outcome.payment, None);
    }

    #[tokio::test]
    async fn отказ_по_возврату_приходит_как_fail() {
        let fail = format!(
            "TransactionId=1601&Amount=100.00&Currency=RUB&DateTime=2026-11-16+15%3A00%3A00&TestMode=1&Reason=Rejected&ReasonCode=5006&OperationType=Refund&InvoiceId={PAYMENT}&AccountId={CLIENT}"
        );
        let outcome = deliver(
            &settings(),
            "fail",
            signed_hook("Content-HMAC", &fail, &fail),
        )
        .await
        .unwrap();
        assert_eq!(outcome.status, ProviderStatus::RefundDeclined);
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.amount, Some(rub(10_000)));
        assert_eq!(outcome.provider_txid, None);
        assert_eq!(outcome.reply, reply(0));
        assert_eq!(
            outcome.event_key.rsplit_once("refund:").map(|(_, id)| id),
            Some("1601")
        );
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

    #[test]
    fn способы_оплаты_cloudpayments() {
        let cloud = CloudPayments::new();
        let rub_methods = cloud.methods(Currency::Rub);
        let codes: Vec<&str> = rub_methods.iter().map(|m| m.code).collect();
        assert_eq!(codes, ["page", "sbp"]);
        for method in &rub_methods {
            assert!(
                method.code.len() <= 20
                    && method
                        .code
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{}",
                method.code
            );
        }
        assert_eq!(
            cloud.method(Currency::Rub, "page").map(|m| m.title),
            Some("Все способы")
        );
        assert_eq!(
            cloud.method(Currency::Rub, "sbp").map(|m| m.title),
            Some("СБП")
        );
        assert_eq!(cloud.method(Currency::Rub, "tpay"), None);
        assert!(cloud.methods(Currency::Usd).is_empty());
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
            .respond_with(model(json!({
                "Id": "f2K8LV6reGE9WBFn", "Number": 61, "Amount": 499.0, "Currency": "RUB",
                "Url": "https://orders.cloudpayments.ru/d/f2K8LV6reGE9WBFn", "Status": "Created"
            })))
            .expect(2)
            .mount(&server)
            .await;
        for method_code in [None, Some(PAGE)] {
            let mut request = invoice(rub(49_900), Some("client@example.com"));
            request.method = method_code.map(str::to_string);
            let created = CloudPayments::with_api_base(server.uri())
                .create_invoice(&settings(), &request)
                .await
                .unwrap();
            assert_eq!(
                created.pay_url,
                "https://orders.cloudpayments.ru/d/f2K8LV6reGE9WBFn"
            );
            assert_eq!(created.external_id, None);
            assert_eq!(created.payload["order_id"], "f2K8LV6reGE9WBFn");
        }
    }

    #[tokio::test]
    async fn ссылка_сбп_через_payments_qr_sbp_link() {
        let server = MockServer::start().await;
        let qr = "https://qr.nspk.ru/AS1A003RHC2DBF6Q8LBPLN1VAGGV3PA2?type=02&bank=100000000111&sum=49900&cur=RUB&crc=AB75";
        Mock::given(method("POST"))
            .and(path("/payments/qr/sbp/link"))
            .and(basic_auth("pk_8f3b2c1d0e", SECRET))
            .and(header("X-Request-ID", PAYMENT.to_string().as_str()))
            .and(body_json(json!({
                "PublicId": "pk_8f3b2c1d0e", "Amount": 499.0, "Currency": "RUB",
                "Description": "Стандарт · 3 месяца", "InvoiceId": PAYMENT.to_string(),
                "AccountId": CLIENT.to_string(), "Scheme": "charge", "TtlMinutes": 60,
                "Email": "client@example.com",
                "SuccessRedirectUrl": "https://panel.example.com/cabinet/payment-wait"
            })))
            .respond_with(model(json!({
                "QrUrl": qr, "QrImage": null, "TransactionId": 11_122_233_344_i64,
                "MerchantOrderId": PAYMENT.to_string(), "ProviderQrId": "AS1A003RHC2DBF6Q8LBPLN1VAGGV3PA2",
                "Amount": 499, "Message": "Created", "IsTest": false
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut request = invoice(rub(49_900), Some("client@example.com"));
        request.method = Some(SBP.into());
        let created = CloudPayments::with_api_base(server.uri())
            .create_invoice(&settings(), &request)
            .await
            .unwrap();
        assert_eq!(created.pay_url, qr);
        assert_eq!(created.external_id.as_deref(), Some("11122233344"));
        assert_eq!(created.payload["method"], "sbp");
        assert_eq!(created.payload["transaction_id"], "11122233344");
    }

    #[tokio::test]
    async fn ссылка_сбп_без_номера_транзакции_не_принимается() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/payments/qr/sbp/link"))
            .respond_with(model(
                json!({"QrUrl": "https://qr.nspk.ru/AS1", "TransactionId": null}),
            ))
            .mount(&server)
            .await;
        let mut request = invoice(rub(49_900), None);
        request.method = Some(SBP.into());
        assert!(matches!(
            CloudPayments::with_api_base(server.uri())
                .create_invoice(&settings(), &request)
                .await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn неизвестный_способ_оплаты_отклоняется() {
        let cloud = CloudPayments::with_api_base("http://127.0.0.1:9");
        let mut request = invoice(rub(49_900), None);
        request.method = Some("tpay".into());
        assert_eq!(
            cloud.create_invoice(&settings(), &request).await,
            Err(PaymentError::Rejected(
                "этот способ оплаты недоступен".into()
            ))
        );
        request.amount = usd(500);
        assert_eq!(
            cloud.create_invoice(&settings(), &request).await,
            Err(PaymentError::UnsupportedCurrency("USD".into()))
        );
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
    async fn опрос_оплаты_берёт_только_оплаты_своего_режима() {
        let server = MockServer::start().await;
        let find = |invoice: Uuid, found: ResponseTemplate| {
            Mock::given(method("POST"))
                .and(path("/v2/payments/find"))
                .and(basic_auth("pk_8f3b2c1d0e", SECRET))
                .and(body_json(json!({"InvoiceId": invoice.to_string()})))
                .respond_with(found)
        };
        let payment = |invoice: Uuid, status: &str, test: bool| {
            model(json!({
                "TransactionId": 1504, "Amount": 499.00, "Currency": "RUB", "Status": status,
                "InvoiceId": invoice.to_string(), "TestMode": test, "Type": 0
            }))
        };
        let declined = Uuid::from_u128(2);
        let cancelled = Uuid::from_u128(3);
        let refunded = Uuid::from_u128(4);
        let test_only = Uuid::from_u128(5);
        let other = Uuid::from_u128(6);
        let missing = Uuid::from_u128(7);
        for mock in [
            find(PAYMENT, payment(PAYMENT, "Completed", false)),
            find(declined, payment(declined, "Declined", false)),
            find(cancelled, payment(cancelled, "Cancelled", false)),
            find(
                refunded,
                model(json!({
                    "TransactionId": 1700, "Amount": 100.00, "Currency": "RUB", "Status": "Completed",
                    "InvoiceId": refunded.to_string(), "TestMode": false, "Type": 1,
                    "OriginalTransactionId": 1650
                })),
            ),
            find(test_only, payment(test_only, "Completed", true)),
            find(other, payment(PAYMENT, "Completed", false)),
            find(missing, not_found()),
        ] {
            mock.mount(&server).await;
        }
        let cloud = CloudPayments::with_api_base(server.uri());
        let status = |config: ProviderConfig, invoice: Uuid| {
            let cloud = &cloud;
            async move { cloud.fetch_status(&config, &invoice.to_string()).await }
        };
        let paid = status(settings(), PAYMENT).await.unwrap().unwrap();
        assert_eq!(paid.status, ProviderStatus::Paid);
        assert_eq!(paid.provider_txid.as_deref(), Some("1504"));
        assert_eq!(paid.amount, Some(rub(49_900)));
        assert_eq!(paid.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(
            status(settings(), declined)
                .await
                .unwrap()
                .map(|o| o.status),
            Some(ProviderStatus::Failed)
        );
        assert_eq!(
            status(settings(), cancelled)
                .await
                .unwrap()
                .map(|o| o.status),
            Some(ProviderStatus::Canceled)
        );
        assert_eq!(status(settings(), refunded).await, Ok(None));
        assert_eq!(status(settings(), test_only).await, Ok(None));
        assert_eq!(
            status(test_settings(), test_only)
                .await
                .unwrap()
                .map(|o| o.status),
            Some(ProviderStatus::Paid)
        );
        assert_eq!(status(test_settings(), PAYMENT).await, Ok(None));
        assert_eq!(status(settings(), other).await, Ok(None));
        assert_eq!(status(settings(), missing).await, Ok(None));
        assert!(matches!(
            cloud.fetch_status(&settings(), "order-77").await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn опрос_оплаты_сбп_по_номеру_транзакции() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/payments/get"))
            .and(body_json(json!({"TransactionId": 11_122_233_344_i64})))
            .respond_with(model(json!({
                "TransactionId": 11_122_233_344_i64, "Amount": 499, "Currency": "RUB",
                "Status": "AwaitingAuthentication", "InvoiceId": PAYMENT.to_string(),
                "TestMode": false, "Type": 0
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/payments/get"))
            .and(body_json(json!({"TransactionId": 9})))
            .respond_with(not_found())
            .mount(&server)
            .await;
        let cloud = CloudPayments::with_api_base(server.uri());
        let waiting = cloud
            .fetch_status(&settings(), "11122233344")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.status, ProviderStatus::Pending);
        assert_eq!(waiting.provider_txid.as_deref(), Some("11122233344"));
        assert_eq!(cloud.fetch_status(&settings(), "009").await, Ok(None));
    }

    #[tokio::test]
    async fn возврат_ждёт_подтверждения_и_у_каждого_свой_ключ() {
        let server = MockServer::start().await;
        for (key, refund_id) in [(5_u128, 1601), (6, 1602)] {
            Mock::given(method("POST"))
                .and(path("/payments/refund"))
                .and(basic_auth("pk_8f3b2c1d0e", SECRET))
                .and(header(
                    "X-Request-ID",
                    Uuid::from_u128(key).to_string().as_str(),
                ))
                .and(body_json(json!({"TransactionId": 1504, "Amount": 100.0})))
                .respond_with(model(json!({"TransactionId": refund_id})))
                .expect(1)
                .mount(&server)
                .await;
        }
        let cloud = CloudPayments::with_api_base(server.uri());
        let request = |key: u128| RefundRequest {
            payment_id: PaymentId(41),
            provider_txid: "1504".into(),
            amount: rub(10_000),
            reason: Some("по просьбе клиента".into()),
            key: Uuid::from_u128(key),
            email: None,
            description: None,
        };
        let first = cloud.refund(&settings(), &request(5)).await.unwrap();
        assert_eq!(
            first,
            RefundOutcome {
                provider_refund_id: Some("1601".into()),
                completed: false
            }
        );
        let second = cloud.refund(&settings(), &request(6)).await.unwrap();
        assert_eq!(second.provider_refund_id.as_deref(), Some("1602"));
        assert!(!second.completed);
        let mut without_txid = request(7);
        without_txid.provider_txid = "tx-1504".into();
        assert!(matches!(
            cloud.refund(&settings(), &without_txid).await,
            Err(PaymentError::Rejected(_))
        ));
    }

    #[tokio::test]
    async fn опрос_возврата_cloudpayments() {
        let server = MockServer::start().await;
        let refund = |id: i64, status: &str| {
            model(json!({
                "TransactionId": id, "Amount": 100.00, "Currency": "RUB", "Status": status,
                "InvoiceId": PAYMENT.to_string(), "TestMode": true, "Type": 1,
                "OriginalTransactionId": 1504
            }))
        };
        for (id, reply) in [
            (1601, refund(1601, "Completed")),
            (1602, refund(1602, "Declined")),
            (1603, refund(1603, "AwaitingAuthentication")),
            (
                1504,
                model(json!({
                    "TransactionId": 1504, "Amount": 499.00, "Currency": "RUB",
                    "Status": "Completed", "InvoiceId": PAYMENT.to_string(), "Type": 0
                })),
            ),
            (1609, not_found()),
        ] {
            Mock::given(method("POST"))
                .and(path("/payments/get"))
                .and(body_json(json!({"TransactionId": id})))
                .respond_with(reply)
                .mount(&server)
                .await;
        }
        let cloud = CloudPayments::with_api_base(server.uri());
        let done = cloud
            .fetch_refund(&settings(), "1601")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            done.status,
            ProviderStatus::PartiallyRefunded {
                refunded: rub(10_000)
            }
        );
        assert_eq!(done.event_key, "refund:1601");
        assert_eq!(done.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(done.provider_txid.as_deref(), Some("1504"));
        assert_eq!(done.amount, Some(rub(10_000)));
        let declined = cloud
            .fetch_refund(&settings(), "1602")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(declined.status, ProviderStatus::RefundDeclined);
        assert_eq!(declined.event_key, "refund:1602");
        let waiting = cloud
            .fetch_refund(&settings(), "1603")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.status, ProviderStatus::Pending);
        assert!(matches!(
            cloud.fetch_refund(&settings(), "1504").await,
            Err(PaymentError::BadPayload(_))
        ));
        assert!(matches!(
            cloud.fetch_refund(&settings(), "1609").await,
            Err(PaymentError::Rejected(_))
        ));
        assert_eq!(cloud.fetch_refund(&settings(), "rf-1").await, Ok(None));
    }

    #[test]
    fn настройки_cloudpayments() {
        let cloud = CloudPayments::new();
        assert!(cloud.is_configured(&settings()));
        assert!(!cloud.is_configured(&config(&[
            ("public_id", json!("test_api_00000000000000000000002")),
            ("api_secret", json!(SECRET))
        ])));
        assert!(cloud.supports_refunds());
        assert!(cloud.supports_refund_polling());
        assert!(cloud.accepts_http_webhooks());
    }

    #[tokio::test]
    async fn заменённый_счёт_отменяется_в_cloudpayments() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/orders/cancel"))
            .and(basic_auth("pk_8f3b2c1d0e", SECRET))
            .and(body_json(json!({ "Id": "f2K8LV6reGE9WBFn" })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "Success": true, "Message": null })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let provider = CloudPayments::with_api_base(server.uri());
        let payload = json!({ "order_id": "f2K8LV6reGE9WBFn" });
        provider
            .cancel_invoice(
                &settings(),
                &IssuedInvoice {
                    external_id: None,
                    payload: &payload,
                },
            )
            .await
            .unwrap();
        let empty = json!({});
        assert!(
            provider
                .cancel_invoice(
                    &settings(),
                    &IssuedInvoice {
                        external_id: None,
                        payload: &empty,
                    },
                )
                .await
                .is_err()
        );
    }
}
