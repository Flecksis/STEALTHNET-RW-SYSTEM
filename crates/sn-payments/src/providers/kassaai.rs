use crate::error::{PaymentError, PaymentResult};
use crate::form;
use crate::json::Json;
use crate::md5;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use std::net::IpAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

const API: &str = "https://api.fk.life/v1";
const SHOP_ID: &str = "shop_id";
const API_KEY: &str = "api_key";
const SECRET_WORD: &str = "secret_word_2";
const KEY_MAX: usize = 256;
const SBP: PayMethod = PayMethod::new("sbp", "СБП", "Через приложение банка");
const CARD: PayMethod = PayMethod::new("card", "Карта", "Карты российских банков");
const SBERPAY: PayMethod = PayMethod::new("sberpay", "SberPay", "Через СберБанк Онлайн");
const CHANNELS: [(PayMethod, i64); 3] = [(SBP, 44), (CARD, 36), (SBERPAY, 43)];
const DEFAULT_CHANNEL: i64 = 44;
const FALLBACK_IP: &str = "127.0.0.1";
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
const NOTIFY_IPS: &str = "168.119.157.136, 168.119.60.227, 178.154.197.79, 51.250.54.238";
const REFUND_TWICE: &str = "по этому заказу в Kassa.ai уже был возврат: второй возврат через панель не отправлен, \
     проверьте заказ в кабинете Kassa.ai";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Field {
    Int(i64),
    Text(String),
    Number(String),
}

impl Field {
    fn text(&self) -> String {
        match self {
            Field::Int(value) => value.to_string(),
            Field::Text(value) | Field::Number(value) => value.clone(),
        }
    }

    fn json(&self) -> Json {
        match self {
            Field::Int(value) => Json::from(*value),
            Field::Text(value) => Json::from(value.as_str()),
            Field::Number(value) => Json::number(value.clone()),
        }
    }
}

pub fn signature(api_key: &str, fields: &[(&str, &str)]) -> String {
    let mut sorted: Vec<&(&str, &str)> = fields.iter().filter(|(k, _)| *k != "signature").collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let joined = sorted
        .iter()
        .map(|(_, value)| *value)
        .collect::<Vec<_>>()
        .join("|");
    Hmac::<Sha256>::new_from_slice(api_key.as_bytes())
        .map(|mut mac| {
            mac.update(joined.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        })
        .unwrap_or_default()
}

pub fn notice_signature(shop_id: &str, amount: &str, secret: &str, order: &str) -> String {
    md5::hex(format!("{shop_id}:{amount}:{secret}:{order}").as_bytes())
}

fn signed_body(api_key: &str, fields: Vec<(&'static str, Field)>) -> Json {
    let texts: Vec<(&str, String)> = fields.iter().map(|(k, v)| (*k, v.text())).collect();
    let pairs: Vec<(&str, &str)> = texts.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let sign = signature(api_key, &pairs);
    let mut body = Json::object(fields.into_iter().map(|(k, v)| (k, v.json())));
    body.push("signature", Json::from(sign));
    body
}

fn plain_amount(amount: Amount) -> String {
    let units = amount.units();
    match units.split_once('.') {
        Some((whole, frac)) => {
            let frac = frac.trim_end_matches('0');
            if frac.is_empty() {
                whole.to_string()
            } else {
                format!("{whole}.{frac}")
            }
        }
        None => units,
    }
}

fn shop_id(config: &ProviderConfig) -> PaymentResult<i64> {
    config
        .get(SHOP_ID)
        .filter(|s| s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .ok_or_else(|| {
            PaymentError::NotConfigured("ID магазина Kassa.ai — это число из кабинета".into())
        })
}

fn key_ok(value: &str) -> bool {
    value.len() <= KEY_MAX && value.bytes().all(|b| b.is_ascii_graphic())
}

fn channel(chosen: Option<&str>) -> PaymentResult<i64> {
    match chosen {
        None => Ok(DEFAULT_CHANNEL),
        Some(code) => CHANNELS
            .iter()
            .find(|(method, _)| method.code == code)
            .map(|(_, number)| *number)
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

fn payer_email(request: &InvoiceRequest) -> String {
    match util::client_email(request) {
        Some(email) => util::clip(email, 128),
        None => match request.client.telegram_id {
            Some(id) => format!("{id}@telegram.org"),
            None => format!("{}@telegram.org", request.client.public_id.simple()),
        },
    }
}

fn panel_host(webhook_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(webhook_url).ok()?;
    let host = url.host_str()?;
    Some(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase(),
    )
}

async fn resolve(host: &str) -> Option<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(ip);
    }
    let found = tokio::time::timeout(LOOKUP_TIMEOUT, tokio::net::lookup_host((host, 443)))
        .await
        .ok()?
        .ok()?;
    let addresses: Vec<IpAddr> = found.map(|a| a.ip()).collect();
    addresses
        .iter()
        .find(|ip| ip.is_ipv4())
        .or_else(|| addresses.first())
        .copied()
}

fn message(body: &Json) -> Option<&str> {
    body.get("message")
        .or_else(|| body.get("msg"))
        .or_else(|| body.get("error"))
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
}

fn plain(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if lower.contains("signature") || lower.contains("подпис") {
        return "Kassa.ai не приняла подпись запроса: проверьте API-ключ".into();
    }
    if lower.contains("nonce") {
        return "Kassa.ai отклонила запрос как повторный (nonce), повторите попытку".into();
    }
    if lower.contains("shop") && (lower.contains("not found") || lower.contains("invalid")) {
        return "Kassa.ai не нашла магазин с таким ID".into();
    }
    util::clip(text, 200)
}

fn failure(reply: &util::Reply, body: Option<&Json>) -> PaymentError {
    let text = body.and_then(message).map(plain);
    if reply.status == 401 && text.is_none() {
        return PaymentError::Rejected(util::KEYS_REFUSED.into());
    }
    reply.failure(text.as_deref())
}

fn status_code(order: &Json) -> Option<i64> {
    order
        .get("status")
        .and_then(Json::scalar_text)
        .and_then(|s| s.trim().parse().ok())
}

fn order_number(order: &Json) -> Option<String> {
    order
        .get("fk_order_id")
        .or_else(|| order.get("orderId"))
        .and_then(Json::scalar_text)
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_string)
}

fn order_outcome(order: &Json, number: &str) -> PaymentResult<WebhookOutcome> {
    let code = status_code(order).ok_or_else(|| util::bad("нет статуса заказа"))?;
    let currency = order
        .get("currency")
        .cloned()
        .unwrap_or_else(|| Json::from("RUB"));
    let amount = util::json_amount(order.get("amount"), Some(&currency))?;
    let status = match code {
        1 => ProviderStatus::Paid,
        6 => ProviderStatus::Refunded,
        8 => ProviderStatus::Failed,
        9 => ProviderStatus::Canceled,
        _ => ProviderStatus::Pending,
    };
    let event_key = match code {
        1 => format!("{number}:paid"),
        6 => format!("refund:{number}"),
        other => format!("{number}:{other}"),
    };
    Ok(WebhookOutcome {
        event_key,
        payment: util::public_ref(order.get("merchant_order_id").and_then(Json::as_str))
            .or_else(|| Some(PaymentRef::ProviderTxid(number.to_string()))),
        provider_txid: Some(number.to_string()),
        status,
        amount: Some(amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    })
}

struct Nonce(AtomicI64);

impl Nonce {
    fn next(&self) -> i64 {
        let now = Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_else(|| Utc::now().timestamp_micros().saturating_mul(1000));
        let mut last = self.0.load(Ordering::Relaxed);
        loop {
            let candidate = now.max(last.saturating_add(1));
            match self.0.compare_exchange_weak(
                last,
                candidate,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return candidate,
                Err(seen) => last = seen,
            }
        }
    }
}

pub struct KassaAi {
    http: sn_egress::Http,
    api_base: String,
    nonce: Nonce,
    panel_ip: Mutex<Option<(String, IpAddr)>>,
}

impl Default for KassaAi {
    fn default() -> Self {
        Self::new()
    }
}

impl KassaAi {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        KassaAi {
            http: http_client(sn_domain::payment::providers::KASSA_AI),
            api_base: api_base.into().trim_end_matches('/').to_string(),
            nonce: Nonce(AtomicI64::new(0)),
            panel_ip: Mutex::new(None),
        }
    }

    async fn payer_ip(&self, request: &InvoiceRequest) -> String {
        let Some(host) = panel_host(&request.webhook_url) else {
            return FALLBACK_IP.into();
        };
        if let Ok(known) = self.panel_ip.lock()
            && let Some((cached, ip)) = known.as_ref()
            && *cached == host
        {
            return ip.to_string();
        }
        match resolve(&host).await {
            Some(ip) => {
                if let Ok(mut known) = self.panel_ip.lock() {
                    *known = Some((host, ip));
                }
                ip.to_string()
            }
            None => FALLBACK_IP.into(),
        }
    }

    async fn call(
        &self,
        config: &ProviderConfig,
        path: &str,
        mut fields: Vec<(&'static str, Field)>,
    ) -> PaymentResult<(util::Reply, Option<Json>)> {
        let key = config.require(API_KEY)?;
        fields.push(("shopId", Field::Int(shop_id(config)?)));
        fields.push(("nonce", Field::Int(self.nonce.next())));
        let body = signed_body(key, fields);
        let reply = util::send(
            self.http
                .post(format!("{}{path}", self.api_base))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        let parsed = Json::parse(&reply.body);
        Ok((reply, parsed))
    }

    async fn success(
        &self,
        config: &ProviderConfig,
        path: &str,
        fields: Vec<(&'static str, Field)>,
    ) -> PaymentResult<Json> {
        let (reply, parsed) = self.call(config, path, fields).await?;
        match parsed {
            Some(body)
                if reply.is_success()
                    && body.get("type").and_then(Json::as_str) == Some("success") =>
            {
                Ok(body)
            }
            Some(body) => Err(failure(&reply, Some(&body))),
            None if reply.is_success() => Err(util::bad("ответ Kassa.ai не JSON")),
            None => Err(failure(&reply, None)),
        }
    }

    async fn order(&self, config: &ProviderConfig, number: &str) -> PaymentResult<Json> {
        let id: i64 = number
            .trim()
            .parse()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| util::bad("номер заказа Kassa.ai не число"))?;
        let body = self
            .success(config, "/orders", vec![("orderId", Field::Int(id))])
            .await?;
        body.get("orders")
            .and_then(Json::as_array)
            .and_then(|orders| {
                orders
                    .iter()
                    .find(|o| order_number(o).as_deref() == Some(number.trim()))
            })
            .cloned()
            .ok_or_else(|| PaymentError::Rejected("заказ не найден в Kassa.ai".into()))
    }
}

#[async_trait]
impl PaymentProvider for KassaAi {
    fn api_url(&self, _config: &ProviderConfig) -> Option<String> {
        Some(self.api_base.clone())
    }

    fn id(&self) -> &'static str {
        providers::KASSA_AI
    }

    fn title(&self) -> &'static str {
        "Kassa.ai"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: SBP.title,
            hint: SBP.hint,
        }
    }

    fn methods(&self, currency: Currency) -> Vec<PayMethod> {
        if currency == Currency::Rub {
            CHANNELS.iter().map(|(method, _)| *method).collect()
        } else {
            Vec::new()
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                SHOP_ID,
                "ID магазина",
                "Число из кабинета Kassa.ai: список магазинов или настройки магазина",
            ),
            SettingField::secret(
                API_KEY,
                "API-ключ",
                "Кабинет Kassa.ai → настройки магазина → API-ключ. По нему панель создаёт заказы, проверяет их статус и делает возвраты",
            ),
            SettingField::secret(
                SECRET_WORD,
                "Секретное слово 2",
                "Кабинет Kassa.ai → настройки магазина. По нему панель проверяет, что уведомление об оплате прислала Kassa.ai",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        shop_id(config)?;
        for (key, title) in [(API_KEY, "API-ключ"), (SECRET_WORD, "Секретное слово 2")]
        {
            if !config.get(key).is_some_and(key_ok) {
                return Err(PaymentError::NotConfigured(format!(
                    "{title} Kassa.ai вставлен с лишними символами: нужны латинские буквы, цифры и знаки без пробелов"
                )));
            }
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
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let method = channel(request.method.as_deref())?;
        let order = request.payment_public_id.to_string();
        let mut fields = vec![
            ("paymentId", Field::Text(order.clone())),
            ("i", Field::Int(method)),
            ("email", Field::Text(payer_email(request))),
            ("ip", Field::Text(self.payer_ip(request).await)),
            ("amount", Field::Number(plain_amount(request.amount))),
            ("currency", Field::Text("RUB".into())),
            ("notification_url", Field::Text(request.webhook_url.clone())),
        ];
        if let Some(url) = util::return_url(request) {
            fields.push(("success_url", Field::Text(url.clone())));
            fields.push(("failure_url", Field::Text(url)));
        }
        let body = self.success(config, "/orders/create", fields).await?;
        let number =
            order_number(&body).ok_or_else(|| util::bad("Kassa.ai не вернула номер заказа"))?;
        let pay_url = body
            .get("location")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("Kassa.ai не вернула ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(number.clone()),
            expires_at: None,
            payload: serde_json::json!({
                "orderId": number,
                "paymentId": order,
                "i": method,
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let secret = config.require(SECRET_WORD)?;
        let shop = shop_id(config)?.to_string();
        if request
            .route_suffix
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(util::bad(
                "у Kassa.ai один адрес уведомлений, без дополнительной части",
            ));
        }
        let fields = form::fields(request)?;
        let get = |key: &str| util::field(&fields, key).map(str::trim);
        let (Some(merchant), Some(amount), Some(order), Some(sign)) = (
            get("MERCHANT_ID"),
            get("AMOUNT"),
            get("MERCHANT_ORDER_ID"),
            get("SIGN"),
        ) else {
            return Err(PaymentError::BadSignature);
        };
        let expected = notice_signature(merchant, amount, secret, order);
        if !util::secret_eq(&expected, &sign.to_ascii_lowercase()) {
            return Err(PaymentError::BadSignature);
        }
        if merchant != shop {
            return Err(util::bad(format!(
                "уведомление другого магазина Kassa.ai ({})",
                util::clip(merchant, 16)
            )));
        }
        let intid = get("intid")
            .filter(|i| !i.is_empty())
            .map(util::safe_id)
            .transpose()?
            .ok_or_else(|| util::bad("нет intid"))?;
        let amount = Amount::new(util::minor(amount, PayCurrency::Rub)?, PayCurrency::Rub);
        Ok(WebhookOutcome {
            event_key: format!("{intid}:paid"),
            payment: util::public_ref(Some(order)),
            provider_txid: None,
            status: ProviderStatus::Paid,
            amount: Some(amount),
            saved_method: None,
            reply: WebhookReply::Text {
                status: 200,
                body: "YES".into(),
            },
            signal: None,
        })
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let order = self.order(config, external_id).await?;
        order_outcome(&order, external_id.trim()).map(Some)
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let number = request.provider_txid.trim();
        let order = self.order(config, number).await?;
        match status_code(&order) {
            Some(1) => {}
            Some(6) => return Err(PaymentError::Rejected(REFUND_TWICE.into())),
            _ => {
                return Err(PaymentError::Rejected(
                    "заказ в Kassa.ai не оплачен, возвращать нечего".into(),
                ));
            }
        }
        let currency = order
            .get("currency")
            .cloned()
            .unwrap_or_else(|| Json::from("RUB"));
        let paid = util::json_amount(order.get("amount"), Some(&currency))?;
        if request.amount.minor <= 0 || request.amount.minor > paid.minor {
            return Err(PaymentError::Rejected(format!(
                "в Kassa.ai по этому заказу можно вернуть не больше {}",
                util::amount_text(paid)
            )));
        }
        let id: i64 = number
            .parse()
            .map_err(|_| util::bad("номер заказа Kassa.ai не число"))?;
        let body = self
            .success(
                config,
                "/orders/refund",
                vec![
                    ("orderId", Field::Int(id)),
                    ("orderAmount", Field::Number(plain_amount(request.amount))),
                ],
            )
            .await?;
        let refund = body
            .get("id")
            .and_then(Json::scalar_text)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| util::clip(s, 32))
            .unwrap_or_default();
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("{number}:refund:{refund}")),
            completed: true,
        })
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let balance = match self.success(config, "/balance", Vec::new()).await {
            Ok(body) => body,
            Err(err) if util::keys_refused(&err) => {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "Kassa.ai не приняла ID магазина и API-ключ",
                )
                .detail("ID магазина и API-ключ есть в кабинете Kassa.ai, в настройках магазина");
            }
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let mut check = ConnectionCheck::ok("ID магазина и API-ключ приняты Kassa.ai");
        if let Some(rub) = balance
            .get("balance")
            .and_then(Json::as_array)
            .and_then(|list| {
                list.iter()
                    .find(|b| b.get("currency").and_then(Json::as_str) == Some("RUB"))
            })
            .and_then(|b| util::json_amount(b.get("value"), Some(&Json::from("RUB"))).ok())
        {
            check = check.detail(format!("Баланс магазина: {}", util::amount_text(rub)));
        }
        if let Ok(list) = self.success(config, "/currencies", Vec::new()).await {
            let enabled: Vec<i64> = list
                .get("currencies")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .filter(|c| {
                    c.get("is_enabled")
                        .and_then(Json::scalar_text)
                        .is_some_and(|v| v.trim() == "1")
                })
                .filter_map(|c| c.get("id").and_then(Json::scalar_text)?.trim().parse().ok())
                .collect();
            let off: Vec<&str> = CHANNELS
                .iter()
                .filter(|(_, number)| !enabled.contains(number))
                .map(|(method, _)| method.title)
                .collect();
            if !off.is_empty() {
                check = check.detail(format!(
                    "В магазине Kassa.ai не включены способы: {}. Кнопки с ними не создадут счёт, пока способ не подключат",
                    off.join(", ")
                ));
            }
        }
        check = match context.webhook_url.as_deref() {
            Some(url) => check.detail(format!(
                "В настройках магазина Kassa.ai укажите URL оповещения {url}, метод POST. Уведомления приходят с адресов {NOTIFY_IPS}"
            )),
            None => check.detail(
                "Адрес панели не задан в основных настройках: Kassa.ai некуда присылать уведомления",
            ),
        };
        check.detail(
            "Секретное слово 2 этим запросом не проверить: оно проверяется на уведомлениях. Чтобы проверить его, оплатите небольшой счёт",
        )
    }
}

#[cfg(test)]
mod tests;
