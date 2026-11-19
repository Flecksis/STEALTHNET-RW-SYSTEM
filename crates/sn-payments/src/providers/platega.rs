use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::{NoticeKind, providers};
use sn_domain::settings::SettingField;
use uuid::Uuid;

const API: &str = "https://app.platega.io";
const METHODS: &[&str] = &["2", "3", "11", "12", "13", "14"];
const DEFAULT_METHOD: i64 = 2;
const ERIP: i64 = 3;
const SBP: PayMethod = PayMethod::new("sbp", "СБП", "Через приложение банка");
const CARD: PayMethod = PayMethod::new("card", "Карта", "Карты российских банков");
const INTL_CARD: PayMethod =
    PayMethod::new("intl_card", "Иностранная карта", "Карты зарубежных банков");
const CRYPTO: PayMethod = PayMethod::new("crypto", "Криптовалюта", "Перевод с криптокошелька");
const SBERPAY: PayMethod = PayMethod::new("sberpay", "SberPay", "Через СберБанк Онлайн");
const PAGE: PayMethod = PayMethod::new(
    "page",
    "Все способы",
    "Способ выбирается на странице оплаты",
);
const CHANNELS: [(PayMethod, i64); 5] = [
    (SBP, 2),
    (CARD, 11),
    (INTL_CARD, 12),
    (CRYPTO, 13),
    (SBERPAY, 14),
];
const ERIP_LABEL: ClientLabel = ClientLabel {
    title: "ЕРИП",
    hint: "Через интернет-банк",
};
const IN_PROGRESS: &str = "возврат по этому платежу уже выполняется, дождитесь его результата";

pub struct Platega {
    http: reqwest::Client,
    api_base: String,
}

impl Default for Platega {
    fn default() -> Self {
        Self::new()
    }
}

impl Platega {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Platega {
            http: http_client(),
            api_base: api_base.into().trim_end_matches('/').to_string(),
        }
    }

    fn request(
        &self,
        config: &ProviderConfig,
        method: reqwest::Method,
        path: &str,
    ) -> PaymentResult<reqwest::RequestBuilder> {
        Ok(self
            .http
            .request(method, format!("{}{path}", self.api_base))
            .header("X-MerchantId", config.require("merchant_id")?)
            .header("X-Secret", config.require("secret")?))
    }

    async fn transaction(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Option<Json>> {
        let id = util::safe_id(id)?;
        let reply = util::send(self.request(
            config,
            reqwest::Method::GET,
            &format!("/transaction/{id}"),
        )?)
        .await?;
        if reply.status == 404 {
            return Ok(None);
        }
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(message(&parsed)));
        }
        if parsed.get("id").and_then(Json::as_str) != Some(id) {
            return Err(util::bad("Platega вернула другую транзакцию"));
        }
        Ok(Some(parsed))
    }

    async fn refund_check(
        &self,
        config: &ProviderConfig,
        id: &str,
        amount: Option<Amount>,
    ) -> PaymentResult<(util::Reply, Json)> {
        let path = match amount {
            Some(amount) => format!(
                "/transaction/{id}/cancel-supported?amount={}",
                amount.units()
            ),
            None => format!("/transaction/{id}/cancel-supported"),
        };
        let reply = util::send(self.request(config, reqwest::Method::GET, &path)?).await?;
        let parsed = reply.json()?;
        Ok((reply, parsed))
    }

    async fn fully_refundable(
        &self,
        config: &ProviderConfig,
        id: &str,
        paid: Amount,
    ) -> PaymentResult<bool> {
        let (reply, check) = self.refund_check(config, id, Some(paid)).await?;
        if reply.is_success() {
            return Ok(check.get("supported").and_then(Json::as_bool) == Some(true));
        }
        match check.get("code").and_then(Json::as_str) {
            Some("AMOUNT_EXCEEDS_REFUNDABLE") => Ok(false),
            _ => Err(reply.failure(message(&check))),
        }
    }
}

fn message(body: &Json) -> Option<&str> {
    body.get("message")
        .or_else(|| body.get("code"))
        .and_then(Json::as_str)
}

fn transaction_amount(tx: &Json) -> PaymentResult<Amount> {
    match tx.get("paymentDetails") {
        Some(details @ Json::Object(_)) => {
            util::json_amount(details.get("amount"), details.get("currency"))
        }
        _ => util::json_amount(tx.get("amount"), tx.get("currency")),
    }
}

fn reference(payload: Option<&str>, id: &str) -> Option<PaymentRef> {
    util::public_ref(payload).or_else(|| Some(PaymentRef::ProviderTxid(id.to_string())))
}

fn legacy_method(config: &ProviderConfig) -> PaymentResult<i64> {
    let blank = match config.0.get("method") {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(text)) => text.trim().is_empty(),
        Some(_) => false,
    };
    if blank {
        return Ok(DEFAULT_METHOD);
    }
    config
        .integer("method")
        .filter(|code| METHODS.contains(&code.to_string().as_str()))
        .ok_or_else(|| {
            PaymentError::NotConfigured(format!("способ оплаты Platega: {}", METHODS.join(", ")))
        })
}

fn label(code: i64) -> ClientLabel {
    if code == ERIP {
        return ERIP_LABEL;
    }
    let method = CHANNELS
        .iter()
        .find(|(_, number)| *number == code)
        .map_or(SBP, |(method, _)| *method);
    ClientLabel {
        title: method.title,
        hint: method.hint,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Method(i64),
    Page,
}

fn route(config: &ProviderConfig, chosen: Option<&str>) -> PaymentResult<Route> {
    match chosen {
        None => legacy_method(config).map(Route::Method),
        Some(code) if code == PAGE.code => Ok(Route::Page),
        Some(code) => CHANNELS
            .iter()
            .find(|(method, _)| method.code == code)
            .map(|(_, number)| Route::Method(*number))
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

fn lifetime(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    let (days, clock) = match raw.split_once('.') {
        Some((days, clock)) if !days.contains(':') => (days, clock),
        _ => ("0", raw),
    };
    let clock = clock.split('.').next().unwrap_or_default();
    let mut parts = clock.split(':');
    let (Some(hours), Some(minutes), Some(seconds), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let number = |text: &str| {
        (!text.is_empty() && text.len() <= 4 && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<i64>().ok())
            .flatten()
    };
    let (minutes, seconds) = (
        number(minutes).filter(|m| *m < 60)?,
        number(seconds).filter(|s| *s < 60)?,
    );
    let total = number(days)? * 86_400 + number(hours)? * 3_600 + minutes * 60 + seconds;
    (total > 0).then(|| Duration::seconds(total))
}

fn blocked(reason: Option<&str>) -> PaymentError {
    PaymentError::Rejected(match reason.map(str::trim).filter(|r| !r.is_empty()) {
        Some("INSUFFICIENT_FUNDS") => "на USDT-балансе Platega не хватает денег на возврат".into(),
        Some("REFUND_IN_PROGRESS") => IN_PROGRESS.into(),
        Some(reason) => format!(
            "Platega не разрешает возврат по этому платежу: {}",
            util::clip(reason, 64)
        ),
        None => "Platega не разрешает возврат по этому платежу".into(),
    })
}

fn refund_failure(reply: &util::Reply, body: &Json) -> PaymentError {
    let text = match body.get("code").and_then(Json::as_str) {
        Some("REFUND_IN_PROGRESS") => IN_PROGRESS,
        Some("PARTIAL_REFUND_EXISTS") => {
            "по этому платежу в Platega уже был частичный возврат, всю сумму вернуть нельзя"
        }
        Some("AMOUNT_EXCEEDS_REFUNDABLE") => {
            "сумма больше той, что Platega может вернуть по этому платежу"
        }
        _ => return reply.failure(message(body)),
    };
    PaymentError::Rejected(text.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefundState {
    Succeeded,
    Declined,
    InProgress,
}

fn refund_state(raw: Option<&str>) -> RefundState {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("succeeded") => RefundState::Succeeded,
        Some("declined" | "failed" | "canceled" | "cancelled" | "rejected") => {
            RefundState::Declined
        }
        _ => RefundState::InProgress,
    }
}

fn transaction_outcome(tx: &Json, id: &str) -> PaymentResult<WebhookOutcome> {
    let status_text = tx
        .get("status")
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет статуса транзакции"))?;
    let status = match status_text {
        "CONFIRMED" => ProviderStatus::Paid,
        "CANCELED" => ProviderStatus::Canceled,
        "CHARGEBACKED" => ProviderStatus::Refunded,
        _ => ProviderStatus::Pending,
    };
    let amount = transaction_amount(tx)?;
    let outcome = WebhookOutcome {
        event_key: format!("{id}:{status_text}"),
        payment: reference(tx.get("payload").and_then(Json::as_str), id),
        provider_txid: Some(id.to_string()),
        status,
        amount: Some(amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    };
    Ok(match status_text {
        "DISPUTED" => outcome.with_notice(
            NoticeKind::Dispute,
            format!(
                "Platega открыла спор по платежу {id}: деньги зарезервированы до конца проверки, \
                 вернуть их сейчас нельзя. Подробности — в кабинете Platega"
            ),
        ),
        _ => outcome,
    })
}

#[async_trait]
impl PaymentProvider for Platega {
    fn id(&self) -> &'static str {
        providers::PLATEGA
    }

    fn title(&self) -> &'static str {
        "Platega"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, config: &ProviderConfig) -> ClientLabel {
        label(legacy_method(config).unwrap_or(DEFAULT_METHOD))
    }

    fn methods(&self, currency: Currency) -> Vec<PayMethod> {
        if currency != Currency::Rub {
            return Vec::new();
        }
        CHANNELS
            .iter()
            .map(|(method, _)| *method)
            .chain([PAGE])
            .collect()
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text("merchant_id", "MerchantId", "Кабинет Platega → Настройки"),
            SettingField::secret("secret", "Секрет", "Кабинет Platega → Настройки"),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config)
            .and_then(|()| legacy_method(config))
            .is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if !matches!(config.get("merchant_id").map(Uuid::parse_str), Some(Ok(_))) {
            return Err(PaymentError::NotConfigured(
                "MerchantId Platega — это UUID из кабинета".into(),
            ));
        }
        legacy_method(config).map(|_| ())
    }

    fn enabled_by_default(&self) -> bool {
        true
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

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let route = route(config, request.method.as_deref())?;
        let return_url = util::return_url(request)
            .ok_or_else(|| PaymentError::NotConfigured("не задан адрес панели".into()))?;
        let mut metadata =
            Json::object([("userId", Json::from(request.client.public_id.to_string()))]);
        if let Some(email) = util::client_email(request) {
            metadata.push("user_email", Json::from(email));
        }
        let mut body = Json::object([
            (
                "paymentDetails",
                Json::object([
                    ("amount", Json::number(request.amount.units())),
                    ("currency", Json::from("RUB")),
                ]),
            ),
            (
                "description",
                Json::from(util::clip(&request.description, 255)),
            ),
            ("return", Json::from(return_url.clone())),
            ("failedUrl", Json::from(return_url)),
            ("payload", Json::from(request.payment_public_id.to_string())),
            ("orderId", Json::from(request.payment_public_id.to_string())),
            ("metadata", metadata),
        ]);
        let (path, links) = match route {
            Route::Method(code) => {
                body.push("paymentMethod", Json::from(code));
                ("/transaction/process", ["redirect", "url"])
            }
            Route::Page => ("/v2/transaction/process", ["url", "redirect"]),
        };
        let reply = util::send(
            self.request(config, reqwest::Method::POST, path)?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(message(&parsed)));
        }
        let id = parsed
            .get("transactionId")
            .or_else(|| parsed.get("id"))
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| util::bad("Platega не вернула id транзакции"))?;
        let pay_url = links
            .iter()
            .filter_map(|key| parsed.get(key).and_then(Json::as_str))
            .find(|u| util::is_https(u))
            .ok_or_else(|| util::bad("Platega не вернула ссылку на оплату"))?;
        let expires_in = parsed.get("expiresIn").and_then(Json::as_str);
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: expires_in.and_then(lifetime).map(|ttl| Utc::now() + ttl),
            payload: serde_json::json!({
                "transactionId": id,
                "status": parsed.get("status").and_then(Json::as_str),
                "expiresIn": expires_in,
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let merchant = config.require("merchant_id")?;
        let secret = config.require("secret")?;
        let (Some(got_merchant), Some(got_secret)) =
            (request.header("x-merchantid"), request.header("x-secret"))
        else {
            return Err(PaymentError::BadSignature);
        };
        let merchant_ok = util::secret_eq(merchant, got_merchant.trim());
        let secret_ok = util::secret_eq(secret, got_secret.trim());
        if !(merchant_ok & secret_ok) {
            return Err(PaymentError::BadSignature);
        }
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        let id = body
            .get("id")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет id"))?;
        let id = util::safe_id(id)?;
        let status = body
            .get("status")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет status"))?;
        if status.starts_with("SUBSCRIPTION_") {
            return Ok(util::skipped(
                format!("{id}:{status}"),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        let payload = body.get("payload").and_then(Json::as_str);
        if let Some(refund_id) = body.get("refundId").and_then(Json::as_str) {
            let event_key = format!("{id}:refund:{refund_id}");
            let refunded = || util::json_amount(body.get("refundAmount"), body.get("currency"));
            let (refund_status, amount) =
                match refund_state(body.get("refundStatus").and_then(Json::as_str)) {
                    RefundState::InProgress => {
                        return Ok(util::skipped(
                            event_key,
                            reference(payload, id),
                            Some(id.to_string()),
                            WebhookReply::ok(),
                        ));
                    }
                    RefundState::Declined => (ProviderStatus::RefundDeclined, refunded().ok()),
                    RefundState::Succeeded if status == "CHARGEBACKED" => {
                        (ProviderStatus::Refunded, Some(refunded()?))
                    }
                    RefundState::Succeeded => {
                        let refunded = refunded()?;
                        (
                            ProviderStatus::PartiallyRefunded { refunded },
                            Some(refunded),
                        )
                    }
                };
            return Ok(WebhookOutcome {
                event_key,
                payment: reference(payload, id),
                provider_txid: Some(id.to_string()),
                status: refund_status,
                amount,
                saved_method: None,
                reply: WebhookReply::ok(),
                signal: None,
            });
        }
        let tx = self
            .transaction(config, id)
            .await?
            .ok_or(PaymentError::BadSignature)?;
        if tx.get("status").and_then(Json::as_str) == Some("DISPUTED") {
            tracing::warn!(
                transaction = id,
                "Platega поставила платёж на проверку: деньги зарезервированы, возвраты по нему недоступны"
            );
        }
        transaction_outcome(&tx, id)
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        match self.transaction(config, external_id).await? {
            Some(tx) => transaction_outcome(&tx, external_id).map(Some),
            None => Err(PaymentError::Rejected(
                "транзакция не найдена в Platega".into(),
            )),
        }
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let id = util::safe_id(&request.provider_txid)?;
        let tx = self
            .transaction(config, id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("транзакция не найдена в Platega".into()))?;
        if tx.get("status").and_then(Json::as_str) == Some("DISPUTED") {
            return Err(PaymentError::Rejected(
                "платёж на проверке в Platega, вернуть деньги можно после её окончания".into(),
            ));
        }
        let full = transaction_amount(&tx)? == request.amount;
        let (reply, check) = self
            .refund_check(config, id, (!full).then_some(request.amount))
            .await?;
        if !reply.is_success() {
            return Err(refund_failure(&reply, &check));
        }
        let allowed = |key: &str| check.get(key).and_then(Json::as_bool) == Some(true);
        if !allowed("supported") {
            return Err(blocked(check.get("blockReason").and_then(Json::as_str)));
        }
        if !full && !allowed("partialSupported") {
            return Err(PaymentError::Rejected(
                "Platega не разрешает вернуть часть суммы по этому платежу".into(),
            ));
        }
        let builder = if full {
            self.request(
                config,
                reqwest::Method::POST,
                &format!("/transaction/{id}/cancel"),
            )?
        } else {
            self.request(
                config,
                reqwest::Method::POST,
                &format!("/transaction/{id}/cancel-partial"),
            )?
            .header("Idempotency-Key", request.key.to_string())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(Json::object([("amount", Json::number(request.amount.units()))]).to_bytes())
        };
        let reply = util::send(builder).await?;
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(refund_failure(&reply, &parsed));
        }
        match parsed.get("accepted").and_then(Json::as_bool) {
            Some(true) => {}
            Some(false) => return Err(PaymentError::Rejected(IN_PROGRESS.into())),
            None => return Err(util::bad("Platega не ответила, принят ли возврат")),
        }
        let operation = parsed
            .get("refundOperationId")
            .and_then(Json::as_str)
            .map(str::trim)
            .unwrap_or_default();
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("{id}:refund:{operation}")),
            completed: false,
        })
    }

    async fn fetch_refund(
        &self,
        config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let Some(id) = provider_refund_id
            .split_once(":refund:")
            .and_then(|(id, _)| util::safe_id(id).ok())
        else {
            return Ok(None);
        };
        let tx = self
            .transaction(config, id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("транзакция не найдена в Platega".into()))?;
        let paid = transaction_amount(&tx)?;
        let status = match tx.get("status").and_then(Json::as_str) {
            Some("CHARGEBACKED") => ProviderStatus::Refunded,
            Some("CONFIRMED") => {
                if self.fully_refundable(config, id, paid).await? {
                    ProviderStatus::RefundDeclined
                } else {
                    ProviderStatus::Pending
                }
            }
            _ => ProviderStatus::Pending,
        };
        Ok(Some(WebhookOutcome {
            event_key: provider_refund_id.to_string(),
            payment: reference(tx.get("payload").and_then(Json::as_str), id),
            provider_txid: Some(id.to_string()),
            amount: (status == ProviderStatus::Refunded).then_some(paid),
            status,
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        }))
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        _context: &CheckContext,
    ) -> ConnectionCheck {
        let request = match self.request(config, reqwest::Method::GET, "/balance/all") {
            Ok(request) => request,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let reply = match util::send(request).await {
            Ok(reply) => reply,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let parsed = Json::parse(&reply.body);
        if matches!(reply.status, 401 | 403) {
            let text = parsed.as_ref().and_then(message).map(str::trim);
            if text.is_some_and(|t| t.to_ascii_lowercase().contains("merchant not exists")) {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "Platega не знает мерчанта с таким MerchantId",
                )
                .detail("MerchantId и секрет есть в кабинете Platega: Настройки");
            }
            let check = ConnectionCheck::new(
                CheckStatus::KeysRejected,
                "Platega не приняла MerchantId и секрет",
            );
            return match text.filter(|t| !t.is_empty()) {
                Some(text) => check.detail(format!("Ответ Platega: «{}»", util::clip(text, 200))),
                None => check,
            };
        }
        match parsed {
            Some(Json::Array(_)) if reply.is_success() => {
                ConnectionCheck::ok("MerchantId и секрет приняты Platega")
            }
            Some(body) if !reply.is_success() => {
                ConnectionCheck::failed(&reply.failure(message(&body)))
            }
            _ => ConnectionCheck::failed(&reply.failure(None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;
    use sn_domain::ids::PaymentId;
    use sn_domain::payment::{AmountCheck, PaymentStatus, check_paid_amount};
    use wiremock::matchers::{
        body_json, body_partial_json, header, method, path, query_param, query_param_is_missing,
    };
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const MERCHANT: &str = "29ef6fa6-0d2b-466c-9604-0363a30436cc";
    const TX: &str = "3fa85f64-5717-4562-b3fc-2c963f66afa6";

    fn settings() -> ProviderConfig {
        config(&[
            ("merchant_id", json!(MERCHANT)),
            ("secret", json!("platega-secret")),
        ])
    }

    fn headers() -> Vec<(&'static str, &'static str)> {
        vec![("X-MerchantId", MERCHANT), ("X-Secret", "platega-secret")]
    }

    fn callback(status: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": TX, "amount": 499.0, "currency": "RUB", "status": status,
            "paymentMethod": 2, "payload": PAYMENT.to_string()
        }))
        .unwrap()
    }

    fn reply(body: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(body)
    }

    #[tokio::test]
    async fn проверка_подключения_platega_по_балансам() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/balance/all"))
            .and(header("X-MerchantId", MERCHANT))
            .and(header("X-Secret", "platega-secret"))
            .respond_with(reply(json!([
                {"amount": 15000.5, "currency": "RUB"},
                {"amount": 8420.31, "frozenBalance": 500, "currency": "USDT"}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/balance/all"))
            .and(header(
                "X-MerchantId",
                "00000000-0000-4000-8000-000000000000",
            ))
            .respond_with(problem(401, "Auth:SIGN_1002", "Merchant not exists."))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/balance/all"))
            .respond_with(problem(401, "Auth:GEN_1001", "Access denied."))
            .with_priority(10)
            .mount(&server)
            .await;
        let provider = Platega::with_api_base(server.uri());
        let context = CheckContext::default();
        let ok = provider.check_connection(&settings(), &context).await;
        assert_eq!(ok.status, CheckStatus::Ok, "{}", ok.summary);
        assert!(ok.details.is_empty());
        let unknown = provider
            .check_connection(
                &config(&[
                    ("merchant_id", json!("00000000-0000-4000-8000-000000000000")),
                    ("secret", json!("platega-secret")),
                ]),
                &context,
            )
            .await;
        assert_eq!(unknown.status, CheckStatus::KeysRejected);
        assert!(unknown.summary.contains("не знает мерчанта"));
        let wrong = provider
            .check_connection(
                &config(&[("merchant_id", json!(MERCHANT)), ("secret", json!("other"))]),
                &context,
            )
            .await;
        assert_eq!(wrong.status, CheckStatus::KeysRejected);
        assert_eq!(wrong.details, ["Ответ Platega: «Access denied.»"]);
        for request in server.received_requests().await.unwrap() {
            assert_eq!(request.method, wiremock::http::Method::GET);
        }
        let down = failing_api(502).await;
        let unreachable = Platega::with_api_base(down.uri())
            .check_connection(&settings(), &context)
            .await;
        assert_eq!(unreachable.status, CheckStatus::Unreachable);
        let closed = Platega::with_api_base(closed_api())
            .check_connection(&settings(), &context)
            .await;
        assert_eq!(closed.status, CheckStatus::Unreachable);
    }

    fn problem(status: u16, code: &str, text: &str) -> ResponseTemplate {
        ResponseTemplate::new(status).set_body_json(json!({
            "code": code, "type": 4000 + status, "message": text,
            "data": [{"key": "reasonCode", "message": code}],
            "traceId": "af95e45f8c350d4211ac5c7b6d2e1f04"
        }))
    }

    fn created() -> ResponseTemplate {
        reply(json!({
            "paymentMethod": "SBPQR", "transactionId": TX,
            "redirect": "https://pay.platega.io/?id=3fa85f64",
            "status": "PENDING", "expiresIn": "00:15:00"
        }))
    }

    fn allowed() -> serde_json::Value {
        json!({
            "supported": true, "partialSupported": true, "totalDeductUsdt": 5.4,
            "penaltyNativeAmount": null, "penaltyNativeCurrency": null,
            "penaltyUsdt": null, "penaltyConversionRate": null, "blockReason": null
        })
    }

    fn refund_request(amount: Amount) -> RefundRequest {
        RefundRequest {
            payment_id: PaymentId(41),
            provider_txid: TX.into(),
            amount,
            reason: None,
            key: Uuid::from_u128(amount.minor as u128),
            email: None,
            description: None,
        }
    }

    async fn api(status: &str, amount: serde_json::Value, currency: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/transaction/{TX}")))
            .and(header("X-MerchantId", MERCHANT))
            .and(header("X-Secret", "platega-secret"))
            .respond_with(reply(json!({
                "id": TX, "status": status,
                "paymentDetails": {"amount": amount, "currency": currency},
                "paymentMethod": "SBPQR", "payload": PAYMENT.to_string()
            })))
            .mount(&server)
            .await;
        server
    }

    async fn refundable(server: &MockServer, amount: Option<&str>, answer: ResponseTemplate) {
        let mock = Mock::given(method("GET"))
            .and(path(format!("/transaction/{TX}/cancel-supported")))
            .and(header("X-MerchantId", MERCHANT))
            .and(header("X-Secret", "platega-secret"));
        let mock = match amount {
            Some(amount) => mock.and(query_param("amount", amount)),
            None => mock.and(query_param_is_missing("amount")),
        };
        mock.respond_with(answer).mount(server).await;
    }

    async fn parse(
        server: &MockServer,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> PaymentResult<WebhookOutcome> {
        Platega::with_api_base(server.uri())
            .parse_webhook(&settings(), &hook(headers, body))
            .await
    }

    async fn poll(server: &MockServer, refund_id: &str) -> PaymentResult<Option<WebhookOutcome>> {
        Platega::with_api_base(server.uri())
            .fetch_refund(&settings(), refund_id)
            .await
    }

    #[tokio::test]
    async fn заголовки_platega_и_повторный_запрос_статуса() {
        let server = api("CONFIRMED", json!(499), "RUB").await;
        let outcome = parse(&server, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(rub(49_900)));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.provider_txid.as_deref(), Some(TX));
        let wrong = parse(
            &server,
            &[("X-MerchantId", MERCHANT), ("X-Secret", "guess")],
            callback("CONFIRMED"),
        )
        .await;
        assert_eq!(wrong, Err(PaymentError::BadSignature));
        let foreign = parse(
            &server,
            &[
                ("X-MerchantId", "11111111-1111-4111-8111-111111111111"),
                ("X-Secret", "platega-secret"),
            ],
            callback("CONFIRMED"),
        )
        .await;
        assert_eq!(foreign, Err(PaymentError::BadSignature));
        assert_eq!(
            parse(&server, &[], callback("CONFIRMED")).await,
            Err(PaymentError::BadSignature)
        );
    }

    #[tokio::test]
    async fn статус_берётся_из_api_а_не_из_тела() {
        let server = api("PENDING", json!(499), "RUB").await;
        let outcome = parse(&server, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        let unknown = MockServer::start().await;
        assert_eq!(
            parse(&unknown, &headers(), callback("CONFIRMED")).await,
            Err(PaymentError::BadSignature)
        );
    }

    #[tokio::test]
    async fn отмена_и_подтверждение_той_же_транзакции() {
        let canceled = api("CANCELED", json!(499), "RUB").await;
        let first = parse(&canceled, &headers(), callback("CANCELED"))
            .await
            .unwrap();
        assert_eq!(first.status, ProviderStatus::Canceled);
        let confirmed = api("CONFIRMED", json!(499), "RUB").await;
        let second = parse(&confirmed, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert_eq!(second.status, ProviderStatus::Paid);
        assert_eq!(first.provider_txid, second.provider_txid);
        assert_ne!(first.event_key, second.event_key);
        let mut payment = PaymentStatus::Pending;
        for outcome in [&first, &second] {
            let target = match outcome.status {
                ProviderStatus::Canceled => PaymentStatus::Canceled,
                _ => PaymentStatus::Paid,
            };
            assert_eq!(payment.transition(target), Ok(true));
            payment = target;
        }
        assert!(
            PaymentStatus::Refunded
                .transition(PaymentStatus::Paid)
                .is_err()
        );
    }

    #[tokio::test]
    async fn статусы_platega() {
        for (status, expected) in [
            ("CONFIRMED", ProviderStatus::Paid),
            ("PARTIALLY_REFUNDED", ProviderStatus::Pending),
            ("CANCELED", ProviderStatus::Canceled),
            ("CHARGEBACKED", ProviderStatus::Refunded),
            ("DISPUTED", ProviderStatus::Pending),
            ("PENDING", ProviderStatus::Pending),
        ] {
            let server = api(status, json!(499), "RUB").await;
            let outcome = parse(&server, &headers(), callback(status)).await.unwrap();
            assert_eq!(outcome.status, expected, "{status}");
            assert_eq!(outcome.event_key, format!("{TX}:{status}"));
            let polled = Platega::with_api_base(server.uri())
                .fetch_status(&settings(), TX)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(polled.status, expected, "{status}");
            let disputed = status == "DISPUTED";
            for got in [&outcome, &polled] {
                assert_eq!(
                    got.notice().map(|n| n.kind),
                    disputed.then_some(NoticeKind::Dispute),
                    "{status}"
                );
            }
            if disputed {
                assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
                assert_eq!(outcome.amount, Some(rub(49_900)));
                assert!(outcome.notice().unwrap().text.contains(TX));
            }
        }
        let server = MockServer::start().await;
        let subscription = parse(&server, &headers(), callback("SUBSCRIPTION_ACTIVATED"))
            .await
            .unwrap();
        assert_eq!(subscription.payment, None);
    }

    #[tokio::test]
    async fn частично_возвращённая_транзакция_не_засчитывается_оплатой() {
        let server = api("PARTIALLY_REFUNDED", json!(499), "RUB").await;
        let late = parse(&server, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert_eq!(late.status, ProviderStatus::Pending);
        assert_eq!(late.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(late.event_key, format!("{TX}:PARTIALLY_REFUNDED"));
        assert_eq!(late.signal, None);
        let polled = Platega::with_api_base(server.uri())
            .fetch_status(&settings(), TX)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(polled.status, ProviderStatus::Pending);
    }

    #[tokio::test]
    async fn частичный_возврат_по_колбэку() {
        let server = MockServer::start().await;
        let body = serde_json::to_vec(&json!({
            "id": TX, "amount": 499.0, "currency": "RUB", "status": "PARTIALLY_REFUNDED",
            "payload": PAYMENT.to_string(), "refundId": "rf-1", "refundAmount": 100,
            "refundedAmount": 100, "refundableAmount": 399, "refundStatus": "Succeeded"
        }))
        .unwrap();
        let outcome = parse(&server, &headers(), body).await.unwrap();
        assert_eq!(
            outcome.status,
            ProviderStatus::PartiallyRefunded {
                refunded: rub(10_000)
            }
        );
        assert_eq!(outcome.event_key, format!("{TX}:refund:rf-1"));
        let declined = serde_json::to_vec(&json!({
            "id": TX, "amount": 499.0, "currency": "RUB", "status": "PARTIALLY_REFUNDED",
            "payload": PAYMENT.to_string(), "refundId": "rf-2", "refundAmount": 100,
            "refundStatus": "Declined"
        }))
        .unwrap();
        let declined = parse(&server, &headers(), declined).await.unwrap();
        assert_eq!(declined.status, ProviderStatus::RefundDeclined);
        assert_eq!(declined.event_key, format!("{TX}:refund:rf-2"));
        assert_eq!(declined.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(declined.amount, Some(rub(10_000)));
        let chargeback = serde_json::to_vec(&json!({
            "id": TX, "amount": 499.0, "currency": "RUB", "status": "CHARGEBACKED",
            "payload": PAYMENT.to_string(), "refundId": "rf-4", "refundAmount": 499,
            "refundStatus": "Succeeded"
        }))
        .unwrap();
        let chargeback = parse(&server, &headers(), chargeback).await.unwrap();
        assert_eq!(chargeback.status, ProviderStatus::Refunded);
        assert_eq!(chargeback.amount, Some(rub(49_900)));
    }

    #[tokio::test]
    async fn незавершённый_возврат_по_колбэку_не_засчитывается() {
        let server = MockServer::start().await;
        for (status, state) in [
            ("CONFIRMED", Some("Pending")),
            ("CONFIRMED", Some("InProgress")),
            ("CONFIRMED", Some("Created")),
            ("CHARGEBACKED", None),
            ("PARTIALLY_REFUNDED", None),
        ] {
            let mut body = json!({
                "id": TX, "amount": 499.0, "currency": "RUB", "status": status,
                "payload": PAYMENT.to_string(), "refundId": "rf-3", "refundAmount": 100
            });
            if let Some(state) = state {
                body["refundStatus"] = json!(state);
            }
            let outcome = parse(&server, &headers(), serde_json::to_vec(&body).unwrap())
                .await
                .unwrap();
            assert_eq!(
                outcome.status,
                ProviderStatus::Pending,
                "{status} {state:?}"
            );
            assert_eq!(outcome.event_key, format!("{TX}:refund:rf-3"));
            assert_eq!(outcome.amount, None);
        }
    }

    #[tokio::test]
    async fn недоплата_и_чужая_валюта_platega() {
        let expected = rub(49_900);
        let under = api("CONFIRMED", json!(498.99), "RUB").await;
        let outcome = parse(&under, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, outcome.amount),
            AmountCheck::Underpaid { .. }
        ));
        let usd_tx = api("CONFIRMED", json!(499), "USD").await;
        let outcome = parse(&usd_tx, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, outcome.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
        let fractional = api("CONFIRMED", json!(498.999), "RUB").await;
        assert!(matches!(
            parse(&fractional, &headers(), callback("CONFIRMED")).await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn счёт_platega() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/transaction/process"))
            .and(header("X-MerchantId", MERCHANT))
            .and(header("X-Secret", "platega-secret"))
            .and(body_json(json!({
                "paymentMethod": 2,
                "paymentDetails": {"amount": 499.5, "currency": "RUB"},
                "description": "Стандарт · 3 месяца",
                "return": "https://panel.example.com/cabinet/payment-wait",
                "failedUrl": "https://panel.example.com/cabinet/payment-wait",
                "payload": PAYMENT.to_string(),
                "orderId": PAYMENT.to_string(),
                "metadata": {"userId": CLIENT.to_string(), "user_email": "client@example.com"}
            })))
            .respond_with(created())
            .expect(1)
            .mount(&server)
            .await;
        let before = Utc::now();
        let created = Platega::with_api_base(server.uri())
            .create_invoice(
                &settings(),
                &invoice(rub(49_950), Some("client@example.com")),
            )
            .await
            .unwrap();
        let after = Utc::now();
        assert_eq!(created.external_id.as_deref(), Some(TX));
        assert_eq!(created.pay_url, "https://pay.platega.io/?id=3fa85f64");
        let expires = created.expires_at.unwrap();
        assert!(expires >= before + Duration::minutes(15));
        assert!(expires <= after + Duration::minutes(15));
        let sent = server.received_requests().await.unwrap();
        assert!(String::from_utf8_lossy(&sent[0].body).contains("\"amount\":499.50"));
    }

    #[tokio::test]
    async fn без_почты_клиента_адрес_не_выдумывается() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/transaction/process"))
            .respond_with(created())
            .expect(1)
            .mount(&server)
            .await;
        Platega::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(rub(49_900), None))
            .await
            .unwrap();
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(body["metadata"], json!({"userId": CLIENT.to_string()}));
        assert!(!String::from_utf8_lossy(&sent[0].body).contains("telegram.org"));
    }

    #[tokio::test]
    async fn способ_из_старой_настройки_строкой_и_числом() {
        for (stored, number) in [
            (json!(11), 11),
            (json!("14"), 14),
            (json!(" 12 "), 12),
            (json!(null), 2),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/transaction/process"))
                .and(body_partial_json(json!({"paymentMethod": number})))
                .respond_with(created())
                .expect(1)
                .mount(&server)
                .await;
            let mut legacy = settings();
            legacy.set("method", stored);
            Platega::with_api_base(server.uri())
                .create_invoice(&legacy, &invoice(rub(49_900), None))
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn выбранный_способ_уходит_кодом_platega() {
        let mut legacy = settings();
        legacy.set("method", json!("11"));
        for (code, number) in [
            ("sbp", 2),
            ("card", 11),
            ("intl_card", 12),
            ("crypto", 13),
            ("sberpay", 14),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/transaction/process"))
                .and(body_partial_json(json!({
                    "paymentMethod": number, "orderId": PAYMENT.to_string()
                })))
                .respond_with(created())
                .expect(1)
                .mount(&server)
                .await;
            let mut request = invoice(rub(49_900), None);
            request.method = Some(code.into());
            Platega::with_api_base(server.uri())
                .create_invoice(&legacy, &request)
                .await
                .unwrap();
        }
        let server = MockServer::start().await;
        for code in ["erip", "3", "SBP", ""] {
            let mut request = invoice(rub(49_900), None);
            request.method = Some(code.into());
            assert_eq!(
                Platega::with_api_base(server.uri())
                    .create_invoice(&legacy, &request)
                    .await,
                Err(PaymentError::Rejected(
                    "этот способ оплаты недоступен".into()
                )),
                "{code}"
            );
        }
    }

    #[tokio::test]
    async fn выбор_способа_на_странице_platega() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/transaction/process"))
            .and(header("X-MerchantId", MERCHANT))
            .and(header("X-Secret", "platega-secret"))
            .and(body_json(json!({
                "paymentDetails": {"amount": 499.0, "currency": "RUB"},
                "description": "Стандарт · 3 месяца",
                "return": "https://panel.example.com/cabinet/payment-wait",
                "failedUrl": "https://panel.example.com/cabinet/payment-wait",
                "payload": PAYMENT.to_string(),
                "orderId": PAYMENT.to_string(),
                "metadata": {"userId": CLIENT.to_string(), "user_email": "client@example.com"}
            })))
            .respond_with(reply(json!({
                "transactionId": TX, "status": "PENDING",
                "url": "https://pay.platega.io/?id=f8000067-a4a0&mh=0a0000a4",
                "expiresIn": "00:15:00", "rate": 91.2
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut request = invoice(rub(49_900), Some("client@example.com"));
        request.method = Some("page".into());
        let created = Platega::with_api_base(server.uri())
            .create_invoice(&settings(), &request)
            .await
            .unwrap();
        assert_eq!(created.external_id.as_deref(), Some(TX));
        assert_eq!(
            created.pay_url,
            "https://pay.platega.io/?id=f8000067-a4a0&mh=0a0000a4"
        );
        assert!(created.expires_at.is_some());
    }

    #[test]
    fn способы_оплаты_platega() {
        let platega = Platega::new();
        let listed: Vec<(&str, &str)> = platega
            .methods(Currency::Rub)
            .iter()
            .map(|m| (m.code, m.title))
            .collect();
        assert_eq!(
            listed,
            [
                ("sbp", "СБП"),
                ("card", "Карта"),
                ("intl_card", "Иностранная карта"),
                ("crypto", "Криптовалюта"),
                ("sberpay", "SberPay"),
                ("page", "Все способы"),
            ]
        );
        assert!(platega.methods(Currency::Usd).is_empty());
        assert_eq!(platega.method(Currency::Rub, "sberpay"), Some(SBERPAY));
        assert_eq!(platega.method(Currency::Rub, "erip"), None);
        for method in platega.methods(Currency::Rub) {
            assert!(
                method.code.len() <= 20
                    && method
                        .code
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{}",
                method.code
            );
            assert!(!method.hint.is_empty(), "{}", method.code);
        }
    }

    #[test]
    fn подпись_кнопки_по_настроенному_способу() {
        let platega = Platega::new();
        assert_eq!(
            platega.client_label(Currency::Rub, &settings()).title,
            "СБП"
        );
        for (stored, title) in [
            (json!("2"), "СБП"),
            (json!(3), "ЕРИП"),
            (json!("12"), "Иностранная карта"),
            (json!(13), "Криптовалюта"),
            (json!("14"), "SberPay"),
            (json!(11), "Карта"),
        ] {
            let mut legacy = settings();
            legacy.set("method", stored);
            assert!(platega.is_configured(&legacy));
            assert_eq!(platega.client_label(Currency::Rub, &legacy).title, title);
        }
        let mut broken = settings();
        broken.set("method", json!("99"));
        assert!(!platega.is_configured(&broken));
        assert_eq!(platega.client_label(Currency::Rub, &broken).title, "СБП");
    }

    #[test]
    fn срок_жизни_ссылки_из_ответа_platega() {
        assert_eq!(lifetime("00:15:00"), Some(Duration::minutes(15)));
        assert_eq!(lifetime("1.02:03:04"), Some(Duration::seconds(93_784)));
        assert_eq!(lifetime("00:15:00.5000000"), Some(Duration::minutes(15)));
        for bad in [
            "",
            "15",
            "00:15",
            "00:60:00",
            "aa:bb:cc",
            "00:00:00",
            "-00:15:00",
            "00:15:00:00",
        ] {
            assert_eq!(lifetime(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn возврат_platega_полный_и_частичный() {
        let server = api("CONFIRMED", json!(499), "RUB").await;
        refundable(&server, None, reply(allowed())).await;
        refundable(&server, Some("100.00"), reply(allowed())).await;
        Mock::given(method("POST"))
            .and(path(format!("/transaction/{TX}/cancel")))
            .respond_with(reply(json!({
                "transactionId": TX, "accepted": true, "manualControlRequired": false,
                "message": null, "refundOperationId": "op-full"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/transaction/{TX}/cancel-partial")))
            .and(header(
                "Idempotency-Key",
                Uuid::from_u128(10_000).to_string().as_str(),
            ))
            .and(body_json(json!({"amount": 100.0})))
            .respond_with(reply(json!({
                "transactionId": TX, "accepted": true, "manualControlRequired": false,
                "message": null, "refundOperationId": "op-part"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let platega = Platega::with_api_base(server.uri());
        let full = platega
            .refund(&settings(), &refund_request(rub(49_900)))
            .await
            .unwrap();
        assert_eq!(
            full.provider_refund_id,
            Some(format!("{TX}:refund:op-full"))
        );
        assert!(!full.completed);
        let part = platega
            .refund(&settings(), &refund_request(rub(10_000)))
            .await
            .unwrap();
        assert_eq!(
            part.provider_refund_id,
            Some(format!("{TX}:refund:op-part"))
        );
        assert!(!part.completed);
        let body = serde_json::to_vec(&json!({
            "id": TX, "amount": 499.0, "currency": "RUB", "status": "PARTIALLY_REFUNDED",
            "payload": PAYMENT.to_string(), "refundId": "op-part", "refundAmount": 100,
            "refundedAmount": 100, "refundableAmount": 399, "refundStatus": "Succeeded"
        }))
        .unwrap();
        let callback = parse(&server, &headers(), body).await.unwrap();
        assert_eq!(Some(callback.event_key), part.provider_refund_id);
    }

    #[tokio::test]
    async fn возможность_возврата_проверяется_заранее() {
        let server = api("CONFIRMED", json!(499), "RUB").await;
        refundable(
            &server,
            None,
            reply(json!({
                "supported": false, "partialSupported": false, "totalDeductUsdt": 0,
                "penaltyNativeAmount": null, "penaltyNativeCurrency": null,
                "penaltyUsdt": null, "penaltyConversionRate": null,
                "blockReason": "INSUFFICIENT_FUNDS"
            })),
        )
        .await;
        refundable(
            &server,
            Some("100.00"),
            reply(json!({
                "supported": true, "partialSupported": false, "totalDeductUsdt": 1.2,
                "blockReason": null
            })),
        )
        .await;
        refundable(
            &server,
            Some("50.00"),
            reply(json!({
                "supported": false, "partialSupported": true, "totalDeductUsdt": 0,
                "blockReason": "PROVIDER_DISABLED"
            })),
        )
        .await;
        refundable(
            &server,
            Some("600.00"),
            problem(
                400,
                "AMOUNT_EXCEEDS_REFUNDABLE",
                "Refund amount exceeds the refundable amount.",
            ),
        )
        .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let platega = Platega::with_api_base(server.uri());
        let settings = settings();
        for (minor, text) in [
            (
                49_900,
                "на USDT-балансе Platega не хватает денег на возврат",
            ),
            (
                10_000,
                "Platega не разрешает вернуть часть суммы по этому платежу",
            ),
            (
                5_000,
                "Platega не разрешает возврат по этому платежу: PROVIDER_DISABLED",
            ),
            (
                60_000,
                "сумма больше той, что Platega может вернуть по этому платежу",
            ),
        ] {
            assert_eq!(
                platega.refund(&settings, &refund_request(rub(minor))).await,
                Err(PaymentError::Rejected(text.into())),
                "{minor}"
            );
        }
    }

    #[tokio::test]
    async fn второй_возврат_поверх_идущего_не_создаётся() {
        let server = api("CONFIRMED", json!(499), "RUB").await;
        refundable(&server, None, reply(allowed())).await;
        refundable(&server, Some("100.00"), reply(allowed())).await;
        Mock::given(method("POST"))
            .and(path(format!("/transaction/{TX}/cancel")))
            .respond_with(reply(json!({
                "transactionId": TX, "accepted": false, "manualControlRequired": false,
                "message": "Возврат по этому платежу уже выполняется.",
                "refundOperationId": "b7cd73dd-bab2-4e20-b463-824d3f8bbff4"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/transaction/{TX}/cancel-partial")))
            .respond_with(problem(
                409,
                "REFUND_IN_PROGRESS",
                "An unfinished refund operation already exists for this payment.",
            ))
            .mount(&server)
            .await;
        let platega = Platega::with_api_base(server.uri());
        let settings = settings();
        let busy: PaymentResult<RefundOutcome> = Err(PaymentError::Rejected(IN_PROGRESS.into()));
        assert_eq!(
            platega
                .refund(&settings, &refund_request(rub(49_900)))
                .await,
            busy
        );
        assert_eq!(
            platega
                .refund(&settings, &refund_request(rub(10_000)))
                .await,
            busy
        );
    }

    #[tokio::test]
    async fn пока_платёж_на_проверке_возврат_недоступен() {
        let server = api("DISPUTED", json!(499), "RUB").await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        assert_eq!(
            Platega::with_api_base(server.uri())
                .refund(&settings(), &refund_request(rub(49_900)))
                .await,
            Err(PaymentError::Rejected(
                "платёж на проверке в Platega, вернуть деньги можно после её окончания".into()
            ))
        );
    }

    #[tokio::test]
    async fn опрос_возврата_platega() {
        assert!(Platega::new().supports_refund_polling());
        let key = format!("{TX}:refund:op-full");
        let done = api("CHARGEBACKED", json!(499), "RUB").await;
        let outcome = poll(&done, &key).await.unwrap().unwrap();
        assert_eq!(outcome.status, ProviderStatus::Refunded);
        assert_eq!(outcome.event_key, key);
        assert_eq!(outcome.provider_txid.as_deref(), Some(TX));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.amount, Some(rub(49_900)));
        let declined = api("CONFIRMED", json!(499), "RUB").await;
        refundable(&declined, Some("499.00"), reply(allowed())).await;
        let outcome = poll(&declined, &key).await.unwrap().unwrap();
        assert_eq!(outcome.status, ProviderStatus::RefundDeclined);
        assert_eq!(outcome.event_key, key);
        assert_eq!(outcome.amount, None);
        let running = api("CONFIRMED", json!(499), "RUB").await;
        refundable(
            &running,
            Some("499.00"),
            problem(
                400,
                "AMOUNT_EXCEEDS_REFUNDABLE",
                "Refund amount exceeds the refundable amount.",
            ),
        )
        .await;
        assert_eq!(
            poll(&running, &key).await.unwrap().unwrap().status,
            ProviderStatus::Pending
        );
        let blocked = api("CONFIRMED", json!(499), "RUB").await;
        refundable(
            &blocked,
            Some("499.00"),
            reply(json!({
                "supported": false, "partialSupported": false, "totalDeductUsdt": 0,
                "blockReason": "INSUFFICIENT_FUNDS"
            })),
        )
        .await;
        assert_eq!(
            poll(&blocked, &key).await.unwrap().unwrap().status,
            ProviderStatus::Pending
        );
        for status in ["PARTIALLY_REFUNDED", "DISPUTED"] {
            let server = api(status, json!(499), "RUB").await;
            assert_eq!(
                poll(&server, &key).await.unwrap().unwrap().status,
                ProviderStatus::Pending,
                "{status}"
            );
        }
        let broken = api("CONFIRMED", json!(499), "RUB").await;
        refundable(&broken, Some("499.00"), ResponseTemplate::new(503)).await;
        assert!(poll(&broken, &key).await.is_err());
        let empty = MockServer::start().await;
        assert_eq!(poll(&empty, "op-full").await, Ok(None));
        assert_eq!(poll(&empty, "../x:refund:op-full").await, Ok(None));
    }

    #[test]
    fn настройки_platega() {
        let platega = Platega::new();
        assert!(platega.is_configured(&settings()));
        for good in [
            json!("11"),
            json!(11),
            json!(" 14 "),
            json!(3),
            json!(""),
            json!(null),
        ] {
            let mut stored = settings();
            stored.set("method", good.clone());
            assert!(platega.is_configured(&stored), "{good}");
        }
        for bad in [
            json!("99"),
            json!(99),
            json!("card"),
            json!(11.5),
            json!(true),
        ] {
            let mut stored = settings();
            stored.set("method", bad.clone());
            assert!(!platega.is_configured(&stored), "{bad}");
        }
        assert!(!platega.is_configured(&config(&[
            ("merchant_id", json!("m-1")),
            ("secret", json!("s"))
        ])));
    }
}
