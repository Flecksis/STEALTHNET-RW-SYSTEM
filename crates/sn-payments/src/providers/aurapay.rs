use super::sorted_hmac::{self, Style};
use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use sn_domain::money::{Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use uuid::Uuid;

const API: &str = "https://app.aurapay.tech";
const SHOP_ID: &str = "shop_id";
const API_KEY: &str = "api_key";
const SECRET_KEY: &str = "secret_key";
const KEY_MAX: usize = 512;
const LIFETIME_MAX_MINUTES: i64 = 43_200;
const SBP: PayMethod = PayMethod::new("sbp", "СБП", "Через приложение банка");
const CARD: PayMethod = PayMethod::new("card", "Карта", "Карты российских банков");
const PAGE: PayMethod = PayMethod::new(
    "page",
    "Все способы",
    "Способ выбирается на странице оплаты",
);
const METHODS: [PayMethod; 3] = [SBP, CARD, PAGE];
const LABEL: ClientLabel = ClientLabel {
    title: "Карта или СБП",
    hint: "Способ выбирается на странице оплаты",
};

pub fn signature(secret: &str, body: &[u8]) -> String {
    sorted_hmac::signature(secret, body, Style::Raw)
}

fn verify(secret: &str, body: &Json, header: &str) -> bool {
    sorted_hmac::verify(secret, body, header, Style::Raw)
}

fn text<'a>(object: &'a Json, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn shop(config: &ProviderConfig) -> PaymentResult<Uuid> {
    config
        .get(SHOP_ID)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            PaymentError::NotConfigured("ID кассы AuraPay — это UUID из настроек кассы".into())
        })
}

fn key_ok(value: &str) -> bool {
    value.len() <= KEY_MAX && value.bytes().all(|b| b.is_ascii_graphic())
}

fn service(chosen: Option<&str>) -> PaymentResult<Option<&'static str>> {
    match chosen {
        None => Ok(None),
        Some(code) if code == PAGE.code => Ok(None),
        Some(code) => METHODS
            .iter()
            .find(|m| m.code == code && m.code != PAGE.code)
            .map(|m| Some(m.code))
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

fn lifetime(request: &InvoiceRequest) -> i64 {
    let minutes = (request.expires_in.num_seconds() + 59) / 60;
    minutes.clamp(1, LIFETIME_MAX_MINUTES)
}

fn error_text(body: &[u8]) -> Option<String> {
    Json::parse(body)?
        .get("error")
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

fn plain(message: &str) -> String {
    match message.to_ascii_lowercase().as_str() {
        "shop id not correct" => "AuraPay не нашла кассу с таким ID".into(),
        "api key not correct" => "AuraPay не приняла API-ключ".into(),
        "order id is not unique" => "в AuraPay уже есть счёт с таким номером заказа".into(),
        "invoice not found" => "счёт не найден в AuraPay".into(),
        _ => util::clip(message, 200),
    }
}

fn failure(reply: &util::Reply) -> PaymentError {
    let message = error_text(&reply.body).map(|t| plain(&t));
    if reply.status == 401 {
        return PaymentError::Rejected(message.unwrap_or_else(|| util::KEYS_REFUSED.into()));
    }
    reply.failure(message.as_deref())
}

fn invoice_outcome(invoice: &Json) -> PaymentResult<WebhookOutcome> {
    let id = util::safe_id(text(invoice, "id").ok_or_else(|| util::bad("нет id счёта"))?)?;
    let status = text(invoice, "status")
        .ok_or_else(|| util::bad("нет status"))?
        .to_ascii_uppercase();
    let status = util::clip(&status, 32);
    let amount = util::json_amount(invoice.get("amount"), Some(&Json::from("RUB")))?;
    let (event_key, state) = match status.as_str() {
        "PAID" => (format!("{id}:paid"), ProviderStatus::Paid),
        "EXPIRED" => (format!("{id}:expired"), ProviderStatus::Canceled),
        "REFUNDED" => (format!("refund:{id}"), ProviderStatus::Refunded),
        "ERROR" => (format!("{id}:error"), ProviderStatus::Failed),
        _ => (
            format!("{id}:{}", status.to_ascii_lowercase()),
            ProviderStatus::Pending,
        ),
    };
    Ok(WebhookOutcome {
        event_key,
        payment: util::public_ref(text(invoice, "order_id"))
            .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string()))),
        provider_txid: Some(id.to_string()),
        status: state,
        amount: Some(amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    })
}

pub struct AuraPay {
    http: reqwest::Client,
    api_base: String,
}

impl Default for AuraPay {
    fn default() -> Self {
        Self::new()
    }
}

impl AuraPay {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        AuraPay {
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
            .header("X-ApiKey", config.require(API_KEY)?)
            .header("X-ShopId", shop(config)?.to_string())
            .header(reqwest::header::ACCEPT, "application/json"))
    }

    async fn post(
        &self,
        config: &ProviderConfig,
        path: &str,
        body: &Json,
    ) -> PaymentResult<util::Reply> {
        util::send(
            self.request(config, reqwest::Method::POST, path)?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await
    }
}

#[async_trait]
impl PaymentProvider for AuraPay {
    fn id(&self) -> &'static str {
        providers::AURAPAY
    }

    fn title(&self) -> &'static str {
        "AuraPay"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        LABEL
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
            SettingField::text(
                SHOP_ID,
                "ID кассы",
                "Кабинет AuraPay → настройки кассы → «Уникальный ID кассы»",
            ),
            SettingField::secret(
                API_KEY,
                "API-ключ",
                "Кабинет AuraPay → настройки кассы → «Секретные ключи API» → «API ключ». Ключ показывается один раз при выпуске",
            ),
            SettingField::secret(
                SECRET_KEY,
                "Секретный ключ для Callback",
                "Там же, второй ключ. По нему панель проверяет, что уведомление прислала AuraPay",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        shop(config)?;
        for (key, title) in [
            (API_KEY, "API-ключ"),
            (SECRET_KEY, "Секретный ключ для Callback"),
        ] {
            if !config.get(key).is_some_and(key_ok) {
                return Err(PaymentError::NotConfigured(format!(
                    "{title} AuraPay вставлен с лишними символами: нужны латинские буквы, цифры и знаки без пробелов"
                )));
            }
        }
        Ok(())
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let chosen = service(request.method.as_deref())?;
        let order = request.payment_public_id.to_string();
        let mut body = Json::object([
            ("amount", Json::number(request.amount.units())),
            ("order_id", Json::from(order.clone())),
            ("comment", Json::from(util::clip(&request.description, 255))),
            ("lifetime", Json::from(lifetime(request))),
            ("callback_url", Json::from(request.webhook_url.clone())),
        ]);
        if let Some(url) = util::return_url(request) {
            body.push("success_url", Json::from(url.clone()));
            body.push("fail_url", Json::from(url));
        }
        if let Some(code) = chosen {
            body.push("service", Json::from(code));
        }
        let reply = self.post(config, "/invoice/create", &body).await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        let id = util::safe_id(
            text(&parsed, "id").ok_or_else(|| util::bad("AuraPay не вернула номер счёта"))?,
        )?;
        if text(&parsed, "order_id").is_some_and(|o| o != order) {
            return Err(util::bad("AuraPay вернула счёт другого заказа"));
        }
        let pay_url = parsed
            .at(&["payment_data", "url"])
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("AuraPay не вернула ссылку на оплату"))?;
        if parsed.get("amount").is_some() {
            let billed = util::json_amount(parsed.get("amount"), Some(&Json::from("RUB")))?;
            if billed != request.amount {
                return Err(util::bad(format!(
                    "AuraPay выставила счёт на {} вместо {}",
                    util::amount_text(billed),
                    util::amount_text(request.amount)
                )));
            }
        }
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "id": id,
                "status": text(&parsed, "status"),
                "service": chosen,
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let secret = config.require(SECRET_KEY)?;
        let own = shop(config)?;
        if request
            .route_suffix
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(util::bad(
                "у AuraPay один адрес уведомлений, без дополнительной части",
            ));
        }
        let Some(signed) = request.header("x-signature") else {
            return Err(PaymentError::BadSignature);
        };
        let body = Json::parse(&request.body).ok_or(PaymentError::BadSignature)?;
        if !verify(secret, &body, signed) {
            return Err(PaymentError::BadSignature);
        }
        if text(&body, "shop_id").is_some_and(|s| Uuid::parse_str(s) != Ok(own)) {
            return Err(util::bad("уведомление другой кассы AuraPay"));
        }
        if let Some(event) = text(&body, "event") {
            return Ok(util::skipped(
                format!("subscription:{}", util::clip(event, 32)),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        if body.get("amount_to_payout").is_some() {
            return Ok(util::skipped(
                format!(
                    "payout:{}",
                    text(&body, "id")
                        .map(|i| util::clip(i, 64))
                        .unwrap_or_default()
                ),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        invoice_outcome(&body)
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let id = util::safe_id(external_id.trim())?;
        let reply = self
            .post(
                config,
                "/invoice/status",
                &Json::object([("id", Json::from(id))]),
            )
            .await?;
        if reply.status == 404 {
            return Err(PaymentError::Rejected("счёт не найден в AuraPay".into()));
        }
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        if text(&parsed, "id") != Some(id) {
            return Err(util::bad("AuraPay вернула другой счёт"));
        }
        invoice_outcome(&parsed).map(Some)
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let request = match self.request(config, reqwest::Method::GET, "/shop/balance") {
            Ok(request) => request,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let reply = match util::send(request).await {
            Ok(reply) => reply,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        if reply.status == 401 {
            let said = error_text(&reply.body).map(|t| t.to_ascii_lowercase());
            return if said.as_deref() == Some("shop id not correct") {
                ConnectionCheck::new(
                    CheckStatus::SettingsRejected,
                    "AuraPay не нашла кассу с таким ID",
                )
                .detail("ID кассы есть в кабинете AuraPay: настройки кассы → «Уникальный ID кассы»")
            } else {
                ConnectionCheck::new(CheckStatus::KeysRejected, "AuraPay не приняла API-ключ")
                    .detail("API-ключ выпускается в настройках кассы AuraPay. При перевыпуске старый ключ перестаёт работать")
            };
        }
        if !reply.is_success() {
            return ConnectionCheck::failed(&failure(&reply));
        }
        let parsed = match reply.json() {
            Ok(parsed) if parsed.get("balance").is_some() => parsed,
            Ok(_) => {
                return ConnectionCheck::failed(&util::bad("в ответе AuraPay нет баланса кассы"));
            }
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let mut check = ConnectionCheck::ok("ID кассы и API-ключ приняты AuraPay");
        if let Ok(balance) = util::json_amount(parsed.get("balance"), Some(&Json::from("RUB"))) {
            check = check.detail(format!("Баланс кассы: {}", util::amount_text(balance)));
        }
        check = match context.webhook_url.as_deref() {
            Some(url) => check.detail(format!(
                "Панель передаёт адрес уведомлений {url} с каждым счётом. Если в настройках кассы AuraPay указан адрес старого бота, замените его этим адресом"
            )),
            None => check.detail(
                "Адрес панели не задан в основных настройках: AuraPay некуда присылать уведомления",
            ),
        };
        check.detail(
            "Секретный ключ для Callback этим запросом не проверить: он проверяется на уведомлениях. Чтобы проверить его, оплатите небольшой счёт",
        )
    }
}

#[cfg(test)]
mod tests;
