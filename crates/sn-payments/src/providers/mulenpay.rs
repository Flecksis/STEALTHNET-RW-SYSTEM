use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use sha1::{Digest, Sha1};
use sn_domain::money::{Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;

const API: &str = "https://mulenpay.ru/api";
const SHOP_ID: &str = "shop_id";
const API_KEY: &str = "api_key";
const SECRET_KEY: &str = "secret_key";
const VAT_CODE: &str = "vat_code";
const VAT_CODES: [&str; 8] = ["0", "1", "2", "3", "4", "5", "6", "7"];
const VAT_LABELS: [&str; 8] = [
    "Без НДС",
    "НДС 0%",
    "НДС 10%",
    "НДС 18%",
    "НДС 10/110",
    "НДС 18/118",
    "НДС 20%",
    "НДС 20/120",
];
const SERVICE_SUBJECT: i64 = 4;
const FULL_PAYMENT: i64 = 4;
const KEY_MAX: usize = 512;
const DESCRIPTION_MAX: usize = 128;
const LABEL: ClientLabel = ClientLabel {
    title: "Карта или СБП",
    hint: "Карты, СБП, SberPay и T-Pay",
};
const PARTIAL_REFUND: &str = "MulenPay возвращает через API только всю сумму платежа. \
     Часть суммы можно вернуть в кабинете MulenPay";

pub fn signature(currency: &str, amount: &str, shop_id: &str, secret: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(currency.as_bytes());
    hasher.update(amount.as_bytes());
    hasher.update(shop_id.as_bytes());
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

fn shop_id(config: &ProviderConfig) -> PaymentResult<i64> {
    config
        .get(SHOP_ID)
        .filter(|s| s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .ok_or_else(|| {
            PaymentError::NotConfigured("ID магазина MulenPay — это число из кабинета".into())
        })
}

fn vat_code(config: &ProviderConfig) -> PaymentResult<i64> {
    match config.get(VAT_CODE) {
        None => Ok(0),
        Some(code) if VAT_CODES.contains(&code) => Ok(code.parse().unwrap_or(0)),
        Some(_) => Err(PaymentError::NotConfigured(
            "ставка НДС MulenPay — код от 0 до 7".into(),
        )),
    }
}

fn key_ok(value: &str) -> bool {
    value.len() <= KEY_MAX && value.bytes().all(|b| b.is_ascii_graphic())
}

fn payment_number(raw: &str) -> PaymentResult<&str> {
    let raw = raw.trim();
    if !raw.is_empty() && raw.len() <= 18 && raw.bytes().all(|b| b.is_ascii_digit()) {
        Ok(raw)
    } else {
        Err(util::bad("номер платежа MulenPay не число"))
    }
}

fn number_text(value: Option<&Json>) -> Option<String> {
    value
        .and_then(Json::scalar_text)
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 18 && s.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_string)
}

fn notified_id(request: &WebhookRequest) -> PaymentResult<String> {
    let from_json = Json::parse(&request.body).and_then(|body| number_text(body.get("id")));
    let from_form = || {
        util::form_pairs(request.body.trim_ascii())
            .ok()
            .and_then(|pairs| {
                util::field(&pairs, "id")
                    .and_then(|id| payment_number(id).ok())
                    .map(str::to_string)
            })
    };
    from_json
        .or_else(from_form)
        .ok_or_else(|| util::bad("в уведомлении нет номера платежа MulenPay"))
}

fn message(body: &Json) -> Option<&str> {
    body.get("message")
        .or_else(|| body.get("error"))
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
}

fn plain(text: &str) -> String {
    let lower = text.to_lowercase();
    if lower == "unauthorized" {
        return util::KEYS_REFUSED.into();
    }
    if lower.contains("sign") {
        return "MulenPay не приняла подпись счёта: проверьте секретный ключ и ID магазина".into();
    }
    if lower.contains("магазин не найден") {
        return "MulenPay не нашла магазин с таким ID у владельца этого API-ключа".into();
    }
    util::clip(text, 200)
}

fn failure(reply: &util::Reply) -> PaymentError {
    let text = Json::parse(&reply.body)
        .as_ref()
        .and_then(message)
        .map(plain);
    reply.failure(text.as_deref())
}

fn payment_outcome(payment: &Json, id: &str) -> PaymentResult<WebhookOutcome> {
    let code = payment
        .get("status")
        .and_then(Json::scalar_text)
        .and_then(|s| s.trim().parse::<i64>().ok())
        .ok_or_else(|| util::bad("нет статуса платежа"))?;
    let amount = util::json_amount(payment.get("amount"), payment.get("currency"))?;
    let (event_key, status) = match code {
        3 => (format!("{id}:paid"), ProviderStatus::Paid),
        2 => (format!("{id}:canceled"), ProviderStatus::Canceled),
        4 => (format!("{id}:failed"), ProviderStatus::Failed),
        other => (format!("{id}:{other}"), ProviderStatus::Pending),
    };
    Ok(WebhookOutcome {
        event_key,
        payment: util::public_ref(payment.get("uuid").and_then(Json::as_str))
            .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string()))),
        provider_txid: Some(id.to_string()),
        status,
        amount: Some(amount),
        saved_method: None,
        reply: WebhookReply::Json {
            status: 200,
            body: serde_json::json!({ "success": true }),
        },
        signal: None,
    })
}

pub struct MulenPay {
    http: sn_egress::Http,
    api_base: String,
}

impl Default for MulenPay {
    fn default() -> Self {
        Self::new()
    }
}

impl MulenPay {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        MulenPay {
            http: http_client(sn_domain::payment::providers::MULENPAY),
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
            .bearer_auth(config.require(API_KEY)?)
            .header(reqwest::header::ACCEPT, "application/json"))
    }

    async fn payment(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Option<Json>> {
        let id = payment_number(id)?;
        let reply = util::send(self.request(
            config,
            reqwest::Method::GET,
            &format!("/v2/payments/{id}"),
        )?)
        .await?;
        if matches!(reply.status, 404 | 422) {
            return Ok(None);
        }
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        let payment = parsed
            .get("payment")
            .cloned()
            .ok_or_else(|| util::bad("в ответе MulenPay нет платежа"))?;
        if number_text(payment.get("id")).as_deref() != Some(id) {
            return Err(util::bad("MulenPay вернула другой платёж"));
        }
        Ok(Some(payment))
    }
}

#[async_trait]
impl PaymentProvider for MulenPay {
    fn api_url(&self, _config: &ProviderConfig) -> Option<String> {
        Some(self.api_base.clone())
    }

    fn id(&self) -> &'static str {
        providers::MULENPAY
    }

    fn title(&self) -> &'static str {
        "MulenPay"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        LABEL
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                SHOP_ID,
                "ID магазина",
                "Число из кабинета MulenPay: раздел «Магазины»",
            ),
            SettingField::secret(
                API_KEY,
                "API-ключ",
                "Кабинет MulenPay → настройки магазина → API-ключ. По нему панель создаёт счета, проверяет их статус и делает возвраты",
            ),
            SettingField::secret(
                SECRET_KEY,
                "Секретный ключ",
                "Кабинет MulenPay → настройки магазина → секретный ключ. Им подписывается каждый счёт",
            ),
            SettingField::select(
                VAT_CODE,
                "Ставка НДС в чеке",
                "MulenPay формирует чек по 54-ФЗ на каждую оплату. Если не знаете ставку, оставьте «Без НДС» или спросите бухгалтера",
                &VAT_CODES,
            )
            .labels(&VAT_LABELS)
            .with_default("0"),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        shop_id(config)?;
        vat_code(config)?;
        for (key, title) in [(API_KEY, "API-ключ"), (SECRET_KEY, "Секретный ключ")]
        {
            if !config.get(key).is_some_and(key_ok) {
                return Err(PaymentError::NotConfigured(format!(
                    "{title} MulenPay вставлен с лишними символами: нужны латинские буквы, цифры и знаки без пробелов"
                )));
            }
        }
        Ok(())
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn verifies_webhooks_by_api(&self) -> bool {
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
        let shop = shop_id(config)?;
        let secret = config.require(SECRET_KEY)?;
        let amount = request.amount.units();
        let description = util::clip(&request.description, DESCRIPTION_MAX);
        let order = request.payment_public_id.to_string();
        let mut body = Json::object([
            ("currency", Json::from("rub")),
            ("amount", Json::from(amount.clone())),
            ("uuid", Json::from(order.clone())),
            ("shopId", Json::from(shop)),
            ("description", Json::from(description.clone())),
            ("language", Json::from("ru")),
            (
                "items",
                Json::Array(vec![Json::object([
                    ("description", Json::from(description)),
                    ("quantity", Json::from(1)),
                    ("price", Json::number(amount.clone())),
                    ("vat_code", Json::from(vat_code(config)?)),
                    ("payment_subject", Json::from(SERVICE_SUBJECT)),
                    ("payment_mode", Json::from(FULL_PAYMENT)),
                ])]),
            ),
            (
                "sign",
                Json::from(signature("rub", &amount, &shop.to_string(), secret)),
            ),
        ]);
        if let Some(url) = util::return_url(request) {
            body.push("website_url", Json::from(url));
        }
        let reply = util::send(
            self.request(config, reqwest::Method::POST, "/v2/payments")?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        if parsed.get("success").and_then(Json::as_bool) != Some(true) {
            return Err(PaymentError::Rejected(
                message(&parsed).map_or_else(|| "MulenPay не создала счёт".to_string(), plain),
            ));
        }
        let id = number_text(parsed.get("id"))
            .ok_or_else(|| util::bad("MulenPay не вернула номер платежа"))?;
        let pay_url = parsed
            .get("paymentUrl")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("MulenPay не вернула ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.clone()),
            expires_at: None,
            payload: serde_json::json!({ "id": id, "uuid": order }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        config.require(API_KEY)?;
        if request
            .route_suffix
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(util::bad(
                "у MulenPay один адрес уведомлений, без дополнительной части",
            ));
        }
        let id = notified_id(request)?;
        let payment = self
            .payment(config, &id)
            .await?
            .ok_or(PaymentError::BadSignature)?;
        payment_outcome(&payment, &id)
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let id = payment_number(external_id)?;
        match self.payment(config, id).await? {
            Some(payment) => payment_outcome(&payment, id).map(Some),
            None => Err(PaymentError::Rejected("платёж не найден в MulenPay".into())),
        }
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let id = payment_number(&request.provider_txid)?;
        let payment = self
            .payment(config, id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("платёж не найден в MulenPay".into()))?;
        let state = payment_outcome(&payment, id)?;
        if state.status != ProviderStatus::Paid {
            return Err(PaymentError::Rejected(
                "MulenPay возвращает деньги только по платежу в статусе «Обработан»".into(),
            ));
        }
        if state.amount != Some(request.amount) {
            return Err(PaymentError::Rejected(PARTIAL_REFUND.into()));
        }
        let reply = util::send(self.request(
            config,
            reqwest::Method::PUT,
            &format!("/v2/payments/{id}/refund"),
        )?)
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        if parsed.get("success").and_then(Json::as_bool) != Some(true) {
            return Err(PaymentError::Rejected(message(&parsed).map_or_else(
                || "MulenPay не подтвердила возврат".to_string(),
                plain,
            )));
        }
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("{id}:refund")),
            completed: true,
        })
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let shop = match shop_id(config) {
            Ok(shop) => shop,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let request = match self.request(
            config,
            reqwest::Method::GET,
            &format!("/v2/shops/{shop}/balances"),
        ) {
            Ok(request) => request,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let reply = match util::send(request).await {
            Ok(reply) => reply,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        match reply.status {
            401 => {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "MulenPay не приняла API-ключ",
                )
                .detail("API-ключ есть в кабинете MulenPay, в настройках магазина");
            }
            403 | 404 => {
                return ConnectionCheck::new(
                    CheckStatus::SettingsRejected,
                    format!("MulenPay не нашла магазин {shop} у владельца этого API-ключа"),
                )
                .detail("Проверьте ID магазина: он есть в кабинете MulenPay в разделе «Магазины»");
            }
            _ if !reply.is_success() => return ConnectionCheck::failed(&failure(&reply)),
            _ => {}
        }
        let parsed = match reply.json() {
            Ok(parsed) => parsed,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let mut check =
            ConnectionCheck::ok(format!("API-ключ принят, магазин {shop} найден в MulenPay"));
        if let Some(rub) = parsed
            .at(&["data", "balances"])
            .and_then(Json::as_array)
            .and_then(|list| {
                list.iter().find(|b| {
                    b.get("currency")
                        .and_then(Json::as_str)
                        .is_some_and(|c| c.eq_ignore_ascii_case("rub"))
                })
            })
            .and_then(|b| util::json_amount(b.get("available"), Some(&Json::from("RUB"))).ok())
        {
            check = check.detail(format!("Доступно к выводу: {}", util::amount_text(rub)));
        }
        check = match context.webhook_url.as_deref() {
            Some(url) => check.detail(format!(
                "В настройках магазина MulenPay укажите адрес для уведомлений {url}. Каждое уведомление панель перепроверяет запросом к API MulenPay"
            )),
            None => check.detail(
                "Адрес панели не задан в основных настройках: MulenPay некуда присылать уведомления",
            ),
        };
        check.detail(
            "Секретный ключ этим запросом не проверить: он подписывает счёт. Чтобы проверить его, создайте небольшой счёт",
        )
    }
}

#[cfg(test)]
mod tests;
