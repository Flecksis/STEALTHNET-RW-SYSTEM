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

const API: &str = "https://api.paritypay.net";
const SHOP_ID: &str = "shop_id";
const SECRET_KEY: &str = "secret_key";
const CALLBACK_SECRET: &str = "callback_secret";
const KEY_MAX: usize = 512;
const EXPIRE_MAX_MINUTES: i64 = 43_200;
const URL_MAX: usize = 500;
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
    sorted_hmac::signature(secret, body, Style::Php)
}

fn verify(secret: &str, body: &Json, header: &str) -> bool {
    sorted_hmac::verify(secret, body, header, Style::Php)
        || sorted_hmac::verify(secret, body, header, Style::Raw)
}

fn text<'a>(object: &'a Json, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Json::scalar_text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn shop(config: &ProviderConfig) -> PaymentResult<Uuid> {
    config
        .get(SHOP_ID)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            PaymentError::NotConfigured("ID кассы ParityPay — это UUID из настроек кассы".into())
        })
}

fn same_shop(object: &Json, own: Uuid) -> bool {
    text(object, "shop_id").is_none_or(|s| Uuid::parse_str(s) == Ok(own))
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

fn expire(request: &InvoiceRequest) -> i64 {
    ((request.expires_in.num_seconds() + 59) / 60).clamp(1, EXPIRE_MAX_MINUTES)
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
    match message.to_lowercase().as_str() {
        "shopid is incorrect" => "ParityPay не нашла кассу с таким ID".into(),
        "secretkey is incorrect" => util::KEYS_REFUSED.into(),
        "shop is inactive" => "касса ParityPay неактивна".into(),
        "order id is not unique" => "в ParityPay уже есть счёт с таким номером заказа".into(),
        "invoice not found" => "счёт не найден в ParityPay".into(),
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
        .ok_or_else(|| util::bad("нет статуса счёта"))?
        .to_ascii_uppercase();
    let status = util::clip(&status, 32);
    let amount = util::json_amount(invoice.get("amount"), Some(&Json::from("RUB")))?;
    let (event_key, state) = match status.as_str() {
        "PAID" => (format!("{id}:paid"), ProviderStatus::Paid),
        "EXPIRED" => (format!("{id}:expired"), ProviderStatus::Canceled),
        "ERROR" => (format!("{id}:error"), ProviderStatus::Failed),
        "REFUNDED" => (format!("refund:{id}"), ProviderStatus::Refunded),
        other => (
            format!("{id}:{}", other.to_ascii_lowercase()),
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

pub struct ParityPay {
    http: reqwest::Client,
    api_base: String,
}

impl Default for ParityPay {
    fn default() -> Self {
        Self::new()
    }
}

impl ParityPay {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        ParityPay {
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
            .header("X-ShopId", shop(config)?.to_string())
            .header("X-SecretKey", config.require(SECRET_KEY)?)
            .header(reqwest::header::ACCEPT, "application/json"))
    }

    async fn invoice(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Option<Json>> {
        let id = util::safe_id(id.trim())?;
        let reply = util::send(self.request(
            config,
            reqwest::Method::GET,
            &format!("/v2/invoice/status?id={id}"),
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
            return Err(util::bad("ParityPay вернула другой счёт"));
        }
        if !same_shop(&parsed, shop(config)?) {
            return Err(util::bad("ParityPay вернула счёт другой кассы"));
        }
        Ok(Some(parsed))
    }
}

#[async_trait]
impl PaymentProvider for ParityPay {
    fn id(&self) -> &'static str {
        providers::PARITYPAY
    }

    fn title(&self) -> &'static str {
        "ParityPay"
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
                "Кабинет ParityPay → настройки кассы, это UUID",
            ),
            SettingField::secret(
                SECRET_KEY,
                "Секретный ключ №1",
                "Там же. По нему панель создаёт счета и проверяет их статус",
            ),
            SettingField::secret(
                CALLBACK_SECRET,
                "Секретный ключ №2",
                "Там же, второй ключ. По нему панель проверяет, что уведомление прислала ParityPay",
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
            (SECRET_KEY, "Секретный ключ №1"),
            (CALLBACK_SECRET, "Секретный ключ №2"),
        ] {
            if !config.get(key).is_some_and(key_ok) {
                return Err(PaymentError::NotConfigured(format!(
                    "{title} ParityPay вставлен с лишними символами: нужны латинские буквы, цифры и знаки без пробелов"
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
        let own = shop(config)?;
        let order = request.payment_public_id.to_string();
        let mut body = Json::object([
            ("order_id", Json::from(order.clone())),
            ("amount", Json::number(request.amount.units())),
            ("comment", Json::from(util::clip(&request.description, 255))),
            ("expire", Json::from(expire(request))),
        ]);
        if request.webhook_url.len() <= URL_MAX {
            body.push("callback_url", Json::from(request.webhook_url.clone()));
        }
        if let Some(url) = util::return_url(request).filter(|u| u.len() <= URL_MAX) {
            body.push("success_url", Json::from(url.clone()));
            body.push("fail_url", Json::from(url));
        }
        if let Some(code) = chosen {
            body.push("service", Json::from(code));
        }
        let reply = util::send(
            self.request(config, reqwest::Method::POST, "/v2/invoice/create")?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        let id = util::safe_id(
            text(&parsed, "id").ok_or_else(|| util::bad("ParityPay не вернула номер счёта"))?,
        )?;
        if text(&parsed, "order_id").is_some_and(|o| o != order) {
            return Err(util::bad("ParityPay вернула счёт другого заказа"));
        }
        if !same_shop(&parsed, own) {
            return Err(util::bad("ParityPay вернула счёт другой кассы"));
        }
        let billed = util::json_amount(parsed.get("amount"), Some(&Json::from("RUB")))?;
        if billed != request.amount {
            return Err(util::bad(format!(
                "ParityPay выставила счёт на {} вместо {}",
                util::amount_text(billed),
                util::amount_text(request.amount)
            )));
        }
        let pay_url = parsed
            .get("link")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("ParityPay не вернула ссылку на оплату"))?;
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
        let secret = config.require(CALLBACK_SECRET)?;
        let own = shop(config)?;
        if request
            .route_suffix
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(util::bad(
                "у ParityPay один адрес уведомлений, без дополнительной части",
            ));
        }
        let Some(signed) = request.header("x-signature") else {
            return Err(PaymentError::BadSignature);
        };
        let body = Json::parse(&request.body).ok_or(PaymentError::BadSignature)?;
        if !verify(secret, &body, signed) {
            return Err(PaymentError::BadSignature);
        }
        if !same_shop(&body, own) {
            return Err(util::bad("уведомление другой кассы ParityPay"));
        }
        if body.get("shop_subscription_id").is_some() || body.get("interval").is_some() {
            return Ok(util::skipped(
                format!(
                    "subscription:{}:{}",
                    text(&body, "id")
                        .map(|i| util::clip(i, 64))
                        .unwrap_or_default(),
                    text(&body, "status")
                        .map(|s| util::clip(s, 32))
                        .unwrap_or_default()
                ),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        if body.get("amount_to_payoff").is_some() || body.get("debit").is_some() {
            return Ok(util::skipped(
                format!(
                    "payoff:{}",
                    text(&body, "id")
                        .map(|i| util::clip(i, 64))
                        .unwrap_or_default()
                ),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        let id = text(&body, "id").ok_or_else(|| util::bad("нет id счёта"))?;
        let invoice = self
            .invoice(config, id)
            .await?
            .ok_or_else(|| util::bad("ParityPay не знает счёт из уведомления"))?;
        if text(&invoice, "order_id") != text(&body, "order_id") {
            return Err(util::bad(
                "номер заказа в уведомлении и в ParityPay не совпал",
            ));
        }
        invoice_outcome(&invoice)
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        match self.invoice(config, external_id).await? {
            Some(invoice) => invoice_outcome(&invoice).map(Some),
            None => Err(PaymentError::Rejected("счёт не найден в ParityPay".into())),
        }
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let request = match self.request(config, reqwest::Method::GET, "/v2/shop/balance") {
            Ok(request) => request,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let reply = match util::send(request).await {
            Ok(reply) => reply,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        if !reply.is_success() {
            let said = error_text(&reply.body).map(|t| t.to_lowercase());
            return match said.as_deref() {
                Some("secretkey is incorrect") => ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "ParityPay не приняла секретный ключ №1",
                )
                .detail("Ключи есть в кабинете ParityPay, в настройках кассы"),
                Some("shopid is incorrect" | "магазин не найден") => {
                    ConnectionCheck::new(
                        CheckStatus::SettingsRejected,
                        "ParityPay не нашла кассу с таким ID",
                    )
                }
                Some("shop is inactive") => ConnectionCheck::new(
                    CheckStatus::SettingsRejected,
                    "Касса ParityPay неактивна: счета создаваться не будут",
                ),
                _ => ConnectionCheck::failed(&failure(&reply)),
            };
        }
        let parsed = match reply.json() {
            Ok(parsed) if parsed.get("balance").is_some() => parsed,
            Ok(_) => {
                return ConnectionCheck::failed(&util::bad("в ответе ParityPay нет баланса кассы"));
            }
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let mut check = ConnectionCheck::ok("ID кассы и секретный ключ №1 приняты ParityPay");
        if let Ok(balance) = util::json_amount(parsed.get("balance"), Some(&Json::from("RUB"))) {
            check = check.detail(format!("Баланс кассы: {}", util::amount_text(balance)));
        }
        check = match context.webhook_url.as_deref() {
            Some(url) => check.detail(format!(
                "Панель передаёт адрес уведомлений {url} с каждым счётом. Если в настройках кассы ParityPay указан адрес старого бота, замените его этим"
            )),
            None => check.detail(
                "Адрес панели не задан в основных настройках: ParityPay некуда присылать уведомления",
            ),
        };
        check.detail(
            "Секретный ключ №2 этим запросом не проверить: он проверяется на уведомлениях. Чтобы проверить его, оплатите небольшой счёт",
        )
    }
}

#[cfg(test)]
mod tests;
