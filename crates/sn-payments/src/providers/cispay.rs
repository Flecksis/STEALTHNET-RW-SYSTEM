use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use uuid::Uuid;

const API: &str = "https://api.cispay.app";
const SHOP_ID: &str = "shop_id";
const API_KEY: &str = "api_key";
const TEST_MODE: &str = "test_mode";
const KEY_MAX: usize = 512;
const DESCRIPTION_MAX: usize = 512;
const SBP: PayMethod = PayMethod::new("sbp", "СБП", "Через приложение банка");
const CARD: PayMethod = PayMethod::new("card", "Карта", "Карты российских банков").min_minor(5_000);
const CRYPTO: PayMethod = PayMethod::new("crypto", "Криптовалюта", "Перевод с криптокошелька");
const PAGE: PayMethod = PayMethod::new(
    "page",
    "Все способы",
    "Способ выбирается на странице оплаты",
);
const CHANNELS: [(PayMethod, Option<&str>); 4] = [
    (SBP, Some("SBP")),
    (CARD, Some("CARD")),
    (CRYPTO, Some("CRYPTO")),
    (PAGE, None),
];
const RUB_LABEL: ClientLabel = ClientLabel {
    title: "Карта или СБП",
    hint: "Способ выбирается на странице оплаты",
};
const USD_LABEL: ClientLabel = ClientLabel {
    title: "Криптовалюта",
    hint: "Перевод с криптокошелька",
};

pub fn signature(api_key: &str, body: &[u8]) -> String {
    Hmac::<Sha256>::new_from_slice(api_key.as_bytes())
        .map(|mut mac| {
            mac.update(body);
            hex::encode(mac.finalize().into_bytes())
        })
        .unwrap_or_default()
}

fn verify(api_key: &str, body: &[u8], header: &str) -> bool {
    let Ok(expected) = hex::decode(header.trim()) else {
        return false;
    };
    Hmac::<Sha256>::new_from_slice(api_key.as_bytes())
        .map(|mut mac| {
            mac.update(body);
            mac.verify_slice(&expected).is_ok()
        })
        .unwrap_or(false)
}

fn text<'a>(object: &'a Json, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Json::scalar_text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn minor(object: &Json, key: &str) -> Option<i64> {
    match object.get(key)? {
        Json::Number(raw) => raw.trim().parse::<i64>().ok().filter(|value| *value > 0),
        _ => None,
    }
}

fn shop(config: &ProviderConfig) -> PaymentResult<Uuid> {
    config
        .get(SHOP_ID)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            PaymentError::NotConfigured(
                "Shop ID CisPay — это UUID из настроек магазина в кабинете".into(),
            )
        })
}

fn key_ok(value: &str) -> bool {
    value.len() <= KEY_MAX && value.bytes().all(|b| b.is_ascii_graphic())
}

fn channel(currency: PayCurrency, chosen: Option<&str>) -> PaymentResult<Option<&'static str>> {
    if currency == PayCurrency::Usd {
        return match chosen {
            None => Ok(None),
            Some(code) if code == CRYPTO.code || code == PAGE.code => Ok(Some("CRYPTO")),
            Some(_) => Err(PaymentError::Rejected(
                "в долларах CisPay принимает только криптовалюту".into(),
            )),
        };
    }
    match chosen {
        None => Ok(None),
        Some(code) => CHANNELS
            .iter()
            .find(|(method, _)| method.code == code)
            .map(|(_, wire)| *wire)
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

fn detail(body: &Json) -> Option<String> {
    match body.get("detail")? {
        Json::String(text) => Some(text.trim().to_string()),
        Json::Array(items) => items.first().and_then(|item| {
            let message = item.get("msg").and_then(Json::as_str)?;
            let place = item
                .get("loc")
                .and_then(Json::as_array)
                .and_then(|loc| loc.last())
                .and_then(Json::scalar_text);
            Some(match place {
                Some(place) => format!("{place}: {message}"),
                None => message.to_string(),
            })
        }),
        _ => None,
    }
    .filter(|t| !t.is_empty())
    .map(|t| util::clip(&t, 200))
}

fn failure(reply: &util::Reply) -> PaymentError {
    let message = Json::parse(&reply.body).as_ref().and_then(detail);
    if reply.status == 401 {
        return PaymentError::Rejected(util::KEYS_REFUSED.into());
    }
    reply.failure(message.as_deref())
}

fn paid_amount(payment: &Json, fallback: PayCurrency) -> PaymentResult<Amount> {
    if let (Some(source), Some(currency)) = (
        minor(payment, "source_amount"),
        text(payment, "source_currency"),
    ) {
        return Ok(Amount::new(source, util::pay_currency(currency)?));
    }
    let currency = match text(payment, "currency") {
        Some(code) => util::pay_currency(code)?,
        None => fallback,
    };
    let amount = minor(payment, "amount").ok_or_else(|| util::bad("нет суммы платежа"))?;
    Ok(Amount::new(amount, currency))
}

fn flagged(payment: &Json, key: &str) -> bool {
    matches!(payment.get(key), Some(Json::Bool(true)))
}

fn payment_outcome(payment: &Json, test_mode: bool) -> PaymentResult<WebhookOutcome> {
    let id = util::safe_id(text(payment, "id").ok_or_else(|| util::bad("нет id платежа"))?)?;
    let status = text(payment, "status")
        .ok_or_else(|| util::bad("нет статуса платежа"))?
        .to_ascii_uppercase();
    let status = util::clip(&status, 32);
    let reference = util::public_ref(text(payment, "order_id"))
        .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string())));
    let blocked = text(payment, "block_reason").is_some();
    if blocked || (flagged(payment, "is_sandbox") && !test_mode) {
        let tag = if blocked { "blocked" } else { "sandbox" };
        return Ok(util::skipped(
            format!("{id}:{}:{tag}", status.to_ascii_lowercase()),
            reference,
            Some(id.to_string()),
            WebhookReply::ok(),
        ));
    }
    let amount = paid_amount(payment, PayCurrency::Rub)?;
    let (event_key, state) = match status.as_str() {
        "PAID" => (format!("{id}:paid"), ProviderStatus::Paid),
        "FAILED" => (format!("{id}:failed"), ProviderStatus::Failed),
        "EXPIRED" => (format!("{id}:expired"), ProviderStatus::Canceled),
        "REFUNDED" => (format!("refund:{id}"), ProviderStatus::Refunded),
        other => (
            format!("{id}:{}", other.to_ascii_lowercase()),
            ProviderStatus::Pending,
        ),
    };
    Ok(WebhookOutcome {
        event_key,
        payment: reference,
        provider_txid: Some(id.to_string()),
        status: state,
        amount: Some(amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    })
}

pub struct CisPay {
    http: sn_egress::Http,
    api_base: String,
}

impl Default for CisPay {
    fn default() -> Self {
        Self::new()
    }
}

impl CisPay {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        CisPay {
            http: http_client(sn_domain::payment::providers::CISPAY),
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
            .header("X-Shop-ID", shop(config)?.to_string())
            .header("X-Api-Key", config.require(API_KEY)?)
            .header(reqwest::header::ACCEPT, "application/json"))
    }

    async fn status(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Option<Json>> {
        let id = util::safe_id(id.trim())?;
        let reply = util::send(self.request(
            config,
            reqwest::Method::GET,
            &format!("/payments/status?id={id}"),
        )?)
        .await?;
        if reply.status == 404 {
            return Ok(None);
        }
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        if text(&parsed, "id") != Some(id) {
            return Err(util::bad("CisPay вернула другой платёж"));
        }
        Ok(Some(parsed))
    }
}

#[async_trait]
impl PaymentProvider for CisPay {
    fn api_url(&self, _config: &ProviderConfig) -> Option<String> {
        Some(self.api_base.clone())
    }

    fn id(&self) -> &'static str {
        providers::CISPAY
    }

    fn title(&self) -> &'static str {
        "CisPay"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub, Currency::Usd]
    }

    fn client_label(&self, currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        match currency {
            Currency::Rub => RUB_LABEL,
            Currency::Usd => USD_LABEL,
        }
    }

    fn methods(&self, currency: Currency) -> Vec<PayMethod> {
        match currency {
            Currency::Rub => CHANNELS.iter().map(|(method, _)| *method).collect(),
            Currency::Usd => Vec::new(),
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                SHOP_ID,
                "Shop ID",
                "Кабинет CisPay → «Настройки магазина» → Shop ID, это UUID",
            ),
            SettingField::secret(
                API_KEY,
                "API-ключ",
                "Там же, ключ вида cis_sec_…. Им же CisPay подписывает уведомления",
            ),
            SettingField::flag(
                TEST_MODE,
                "Засчитывать тестовые платежи",
                "Платежи из «Песочницы» кабинета CisPay приходят с отметкой теста. Включите только на время проверки: тестовые оплаты будут засчитываться как настоящие",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        shop(config)?;
        if !config.get(API_KEY).is_some_and(key_ok) {
            return Err(PaymentError::NotConfigured(
                "API-ключ CisPay вставлен с лишними символами: нужны латинские буквы, цифры и знаки без пробелов".into(),
            ));
        }
        Ok(())
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
        let currency = request.amount.currency;
        if !matches!(currency, PayCurrency::Rub | PayCurrency::Usd) {
            return Err(PaymentError::UnsupportedCurrency(currency.code().into()));
        }
        let wire = channel(currency, request.method.as_deref())?;
        if wire == Some("CARD") && request.amount.minor < 5_000 {
            return Err(PaymentError::Rejected(
                "картой через CisPay можно оплатить от 50 ₽".into(),
            ));
        }
        let order = request.payment_public_id.to_string();
        let mut body = Json::object([
            ("amount", Json::from(request.amount.minor)),
            ("currency", Json::from(currency.code())),
            ("order_id", Json::from(order.clone())),
            (
                "customer_id",
                Json::from(request.client.public_id.to_string()),
            ),
            (
                "description",
                Json::from(util::clip(&request.description, DESCRIPTION_MAX)),
            ),
        ]);
        if let Some(code) = wire {
            body.push("payment_method", Json::from(code));
        }
        if let Some(url) = util::return_url(request) {
            body.push("redirect_success_url", Json::from(url.clone()));
            body.push("redirect_fail_url", Json::from(url));
        }
        let reply = util::send(
            self.request(config, reqwest::Method::POST, "/payments")?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        let id = util::safe_id(
            text(&parsed, "id").ok_or_else(|| util::bad("CisPay не вернула номер платежа"))?,
        )?;
        if text(&parsed, "order_id").is_some_and(|o| o != order) {
            return Err(util::bad("CisPay вернула платёж другого заказа"));
        }
        let billed = paid_amount(&parsed, currency)?;
        if billed != request.amount {
            return Err(util::bad(format!(
                "CisPay выставила счёт на {} вместо {}",
                util::amount_text(billed),
                util::amount_text(request.amount)
            )));
        }
        let pay_url = parsed
            .get("payment_url")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("CisPay не вернула ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "id": id,
                "payment_method": wire,
                "charged_amount": minor(&parsed, "charged_amount"),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let key = config.require(API_KEY)?;
        let own = shop(config)?;
        if request
            .route_suffix
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(util::bad(
                "у CisPay один адрес уведомлений, без дополнительной части",
            ));
        }
        let Some(signed) = request.header("x-signature") else {
            return Err(PaymentError::BadSignature);
        };
        if !verify(key, &request.body, signed) {
            return Err(PaymentError::BadSignature);
        }
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        if text(&body, "store_id").is_some_and(|s| Uuid::parse_str(s) != Ok(own)) {
            return Err(util::bad("уведомление другого магазина CisPay"));
        }
        if let Some(event) = text(&body, "event") {
            return Ok(util::skipped(
                format!(
                    "subscription:{}:{}",
                    util::clip(event, 48),
                    text(&body, "id")
                        .map(|i| util::clip(i, 64))
                        .unwrap_or_default()
                ),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        let id = text(&body, "id").ok_or_else(|| util::bad("нет id платежа"))?;
        let payment = self
            .status(config, id)
            .await?
            .ok_or_else(|| util::bad("CisPay не знает платёж из уведомления"))?;
        if text(&payment, "order_id") != text(&body, "order_id") {
            return Err(util::bad("номер заказа в уведомлении и в CisPay не совпал"));
        }
        payment_outcome(&payment, config.flag(TEST_MODE))
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        match self.status(config, external_id).await? {
            Some(payment) => payment_outcome(&payment, config.flag(TEST_MODE)).map(Some),
            None => Err(PaymentError::Rejected("платёж не найден в CisPay".into())),
        }
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        let id = util::safe_id(request.provider_txid.trim())?;
        let payment = self
            .status(config, id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("платёж не найден в CisPay".into()))?;
        if text(&payment, "payment_method") == Some("CRYPTO") {
            return Err(PaymentError::Rejected(
                "оплату криптовалютой CisPay не возвращает: верните деньги клиенту сами".into(),
            ));
        }
        match text(&payment, "status") {
            Some("PAID") => {}
            Some("REFUNDED") => {
                return Err(PaymentError::Rejected(
                    "платёж уже полностью возвращён в CisPay".into(),
                ));
            }
            _ => {
                return Err(PaymentError::Rejected(
                    "CisPay возвращает деньги только по оплаченному платежу".into(),
                ));
            }
        }
        let paid = paid_amount(&payment, PayCurrency::Rub)?;
        if request.amount.currency != paid.currency
            || request.amount.minor <= 0
            || request.amount.minor > paid.minor
        {
            return Err(PaymentError::Rejected(format!(
                "в CisPay по этому платежу можно вернуть не больше {}",
                util::amount_text(paid)
            )));
        }
        let charged = minor(&payment, "charged_amount").unwrap_or(paid.minor);
        let body = if request.amount == paid && charged == paid.minor {
            Json::object([("confirm_amount", Json::from(charged))])
        } else {
            Json::object([
                ("amount", Json::from(request.amount.minor)),
                ("confirm_amount", Json::from(request.amount.minor)),
            ])
        };
        let reply = util::send(
            self.request(
                config,
                reqwest::Method::POST,
                &format!("/payments/refund?id={id}"),
            )?
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_bytes()),
        )
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        if text(&parsed, "transaction_id").is_some_and(|t| t != id) {
            return Err(util::bad("CisPay ответила о возврате другого платежа"));
        }
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("{id}:refund:{}", request.key.simple())),
            completed: true,
        })
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let own = match shop(config) {
            Ok(own) => own,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let request = match self.request(config, reqwest::Method::GET, "/store/capabilities") {
            Ok(request) => request,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let reply = match util::send(request).await {
            Ok(reply) => reply,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        match reply.status {
            401 | 403 => {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "CisPay не приняла Shop ID и API-ключ",
                )
                .detail("Оба значения есть в кабинете CisPay, раздел «Настройки магазина»");
            }
            422 => {
                return ConnectionCheck::new(
                    CheckStatus::SettingsRejected,
                    "CisPay не приняла формат Shop ID или API-ключа",
                );
            }
            _ if !reply.is_success() => return ConnectionCheck::failed(&failure(&reply)),
            _ => {}
        }
        let store = match reply.json() {
            Ok(store) if store.get("is_active").is_some() => store,
            Ok(_) => {
                return ConnectionCheck::failed(&util::bad("в ответе CisPay нет данных магазина"));
            }
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let name = text(&store, "store_name").map(|n| util::clip(n, 80));
        if text(&store, "store_id").is_some_and(|s| Uuid::parse_str(s) != Ok(own)) {
            return ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                "API-ключ CisPay относится к другому магазину",
            )
            .detail("Проверьте, что Shop ID и API-ключ взяты из одного магазина");
        }
        if store.get("is_active").and_then(Json::as_bool) == Some(false) {
            return ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                "Магазин в CisPay отключён: все платежи будут отклонены",
            );
        }
        let mut check = ConnectionCheck::ok(match &name {
            Some(name) => format!("Ключи приняты CisPay, магазин «{name}» активен"),
            None => "Ключи приняты CisPay, магазин активен".to_string(),
        });
        let active: Vec<&str> = store
            .get("payment_methods")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .filter(|m| m.get("is_active").and_then(Json::as_bool) == Some(true))
            .filter_map(|m| text(m, "payment_method"))
            .collect();
        let shown: Vec<&str> = [
            ("SBP", "СБП"),
            ("CARD", "карта"),
            ("CRYPTO", "криптовалюта"),
        ]
        .iter()
        .filter(|(wire, _)| active.contains(wire))
        .map(|(_, title)| *title)
        .collect();
        check = if shown.is_empty() {
            check.detail("В магазине не включён ни один способ оплаты")
        } else {
            check.detail(format!("Включены способы: {}", shown.join(", ")))
        };
        if config.flag(TEST_MODE) {
            check = check.detail(
                "Включено «Засчитывать тестовые платежи»: оплаты из Песочницы засчитываются как настоящие. Перед приёмом настоящих оплат выключите",
            );
        }
        match context.webhook_url.as_deref() {
            Some(url) => check.detail(format!(
                "В настройках магазина CisPay укажите адрес для вебхуков {url}"
            )),
            None => check.detail(
                "Адрес панели не задан в основных настройках: CisPay некуда присылать уведомления",
            ),
        }
    }
}

#[cfg(test)]
mod tests;
