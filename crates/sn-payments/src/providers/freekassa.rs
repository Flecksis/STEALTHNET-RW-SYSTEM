use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::md5;
use crate::provider::*;
use crate::util::{self, field};
use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sn_domain::client::Locale;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use uuid::Uuid;

const API: &str = "https://api.fk.life/v1";
const FORM: &str = "https://pay.fk.money/";
const SHOP: &str = "shop_id";
const API_KEY: &str = "api_key";
const SECRET1: &str = "secret_word_1";
const SECRET2: &str = "secret_word_2";
const SERVER_IP: &str = "server_ip";
const CHECK_IP: &str = "check_ip";
const SENDER_IPS: &str = "sender_ips";
const SENDERS: [Ipv4Addr; 4] = [
    Ipv4Addr::new(168, 119, 157, 136),
    Ipv4Addr::new(168, 119, 60, 227),
    Ipv4Addr::new(178, 154, 197, 79),
    Ipv4Addr::new(51, 250, 54, 238),
];
const SBP: PayMethod = PayMethod::new("sbp", "СБП", "Через приложение банка");
const CARD: PayMethod = PayMethod::new("card", "Карта", "Карты российских банков");
const SBERPAY: PayMethod = PayMethod::new("sberpay", "SberPay", "Через СберБанк Онлайн");
const PAGE: PayMethod = PayMethod::new(
    "page",
    "Все способы",
    "Способ выбирается на странице оплаты",
);
const CHANNELS: [(PayMethod, i64); 3] = [(SBP, 44), (CARD, 36), (SBERPAY, 43)];
const DEFAULT_CHANNEL: i64 = 44;
const PAGE_LABEL: ClientLabel = ClientLabel {
    title: "Карта",
    hint: "Карты российских банков и СБП",
};
const NO_SECRET1: &str = "для кнопки «Все способы» заполните секретное слово 1 Freekassa: оно есть в настройках магазина";
const NO_IP: &str = "Freekassa требует IP-адрес при создании заказа: укажите внешний IPv4 сервера панели в настройках Freekassa";

static LAST_NONCE: AtomicI64 = AtomicI64::new(0);

fn nonce() -> i64 {
    let now = Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX / 2);
    let mut last = LAST_NONCE.load(Ordering::SeqCst);
    loop {
        let next = now.max(last.saturating_add(1));
        match LAST_NONCE.compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return next,
            Err(actual) => last = actual,
        }
    }
}

pub fn amount_text(amount: Amount) -> String {
    let units = amount.units();
    match units.split_once('.') {
        Some((whole, cents)) => {
            let cents = cents.trim_end_matches('0');
            if cents.is_empty() {
                whole.to_string()
            } else {
                format!("{whole}.{cents}")
            }
        }
        None => units,
    }
}

pub fn form_signature(
    shop: &str,
    amount: &str,
    secret: &str,
    currency: &str,
    order: &str,
) -> String {
    md5::hex(format!("{shop}:{amount}:{secret}:{currency}:{order}").as_bytes())
}

pub fn notice_signature(shop: &str, amount: &str, secret: &str, order: &str) -> String {
    md5::hex(format!("{shop}:{amount}:{secret}:{order}").as_bytes())
}

pub fn api_signature(api_key: &str, values: &[(&str, &str)]) -> String {
    let mut sorted: Vec<&(&str, &str)> = values.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let joined = sorted
        .iter()
        .map(|(_, value)| *value)
        .collect::<Vec<_>>()
        .join("|");
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(api_key.as_bytes()) else {
        return String::new();
    };
    mac.update(joined.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn trimmed_amount(raw: &str) -> Option<String> {
    let (whole, fraction) = raw.split_once('.')?;
    let fraction = fraction.trim_end_matches('0');
    Some(if fraction.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
    })
}

fn rubles(raw: &str) -> PaymentResult<Amount> {
    let raw = raw.trim();
    let normalized = trimmed_amount(raw).unwrap_or_else(|| raw.to_string());
    let (_, fraction) = normalized.split_once('.').unwrap_or(("", ""));
    if fraction.len() > 2 {
        return Err(util::bad(format!(
            "сумма «{}» точнее копейки",
            util::clip(raw, 32)
        )));
    }
    Ok(Amount::new(
        util::minor(&normalized, PayCurrency::Rub)?,
        PayCurrency::Rub,
    ))
}

fn digits(raw: &str) -> PaymentResult<&str> {
    let raw = raw.trim();
    if !raw.is_empty() && raw.len() <= 18 && raw.bytes().all(|b| b.is_ascii_digit()) {
        Ok(raw)
    } else {
        Err(util::bad("номер заказа Freekassa не число"))
    }
}

fn culture() -> &'static str {
    match sn_domain::lang::current() {
        Locale::En => "en",
        Locale::Ru => "ru",
    }
}

fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_matches(['[', ']']).to_string()))
}

fn public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

fn senders(config: &ProviderConfig) -> PaymentResult<Vec<IpAddr>> {
    match config.get(SENDER_IPS) {
        None => Ok(SENDERS.iter().map(|ip| IpAddr::V4(*ip)).collect()),
        Some(raw) => raw
            .split([',', ';', ' ', '\n', '\t'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<IpAddr>().map_err(|_| {
                    PaymentError::NotConfigured(format!(
                        "адрес Freekassa «{}» не похож на IP-адрес",
                        util::clip(s, 40)
                    ))
                })
            })
            .collect(),
    }
}

fn buyer_email(request: &InvoiceRequest) -> String {
    if let Some(email) = util::client_email(request) {
        return email.to_string();
    }
    if let Some(telegram) = request.client.telegram_id.filter(|id| *id > 0) {
        return format!("{telegram}@telegram.org");
    }
    let host = host_of(&request.webhook_url).unwrap_or_else(|| "telegram.org".into());
    format!("client{}@{host}", request.client.client_id.0)
}

fn signed(secret: &str, merchant: &str, amount: &str, order: &str, sign: &str) -> bool {
    let received = sign.trim().to_ascii_lowercase();
    let mut candidates = vec![amount.to_string()];
    if let Some(short) = trimmed_amount(amount).filter(|s| s != amount) {
        candidates.push(short);
    }
    candidates.iter().any(|candidate| {
        util::secret_eq(
            &notice_signature(merchant, candidate, secret, order),
            &received,
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Form,
    Api(i64),
}

fn route(config: &ProviderConfig, chosen: Option<&str>) -> PaymentResult<Route> {
    match chosen {
        None if config.get(SECRET1).is_some() => Ok(Route::Form),
        None => Ok(Route::Api(DEFAULT_CHANNEL)),
        Some(code) if code == PAGE.code => Ok(Route::Form),
        Some(code) => CHANNELS
            .iter()
            .find(|(method, _)| method.code == code)
            .map(|(_, id)| Route::Api(*id))
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

fn text(object: &Json, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Json::scalar_text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn message(body: Option<&Json>) -> Option<String> {
    let body = body?;
    ["message", "error", "msg"]
        .iter()
        .find_map(|key| body.get(key).and_then(Json::as_str))
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
}

fn nonce_rejected(reply: &util::Reply) -> bool {
    message(Json::parse(&reply.body).as_ref())
        .is_some_and(|m| m.to_ascii_lowercase().contains("nonce") && !m.starts_with("empty_"))
}

fn plain(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("nonce") {
        return "Freekassa отклонила номер запроса (nonce): с этим API-ключом уже работает другая программа с большими номерами. Подождите минуту и повторите, а если ошибка не уходит — создайте в Freekassa новый API-ключ".into();
    }
    match lower.as_str() {
        "empty_shopid" => "Freekassa не получила ID магазина".into(),
        "empty_signature" => "Freekassa не получила подпись запроса".into(),
        "op-sp-7" => "Freekassa не приняла почту покупателя".into(),
        _ => util::clip(message, 200),
    }
}

fn failure(reply: &util::Reply) -> PaymentError {
    let text = message(Json::parse(&reply.body).as_ref());
    if text
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("wrong signature"))
    {
        return PaymentError::Rejected(util::KEYS_REFUSED.into());
    }
    let explained = text.as_deref().map(plain);
    reply.failure(explained.as_deref())
}

fn order_outcome(order: &Json) -> PaymentResult<WebhookOutcome> {
    let fk = text(order, "fk_order_id").ok_or_else(|| util::bad("нет fk_order_id"))?;
    let fk = digits(&fk)?.to_string();
    let code = text(order, "status").unwrap_or_else(|| "0".into());
    let status = match code.as_str() {
        "1" => ProviderStatus::Paid,
        "6" => ProviderStatus::Refunded,
        "8" => ProviderStatus::Failed,
        "9" => ProviderStatus::Canceled,
        _ => ProviderStatus::Pending,
    };
    let amount = if matches!(status, ProviderStatus::Paid | ProviderStatus::Refunded) {
        Some(util::json_amount(
            order.get("amount"),
            order.get("currency"),
        )?)
    } else {
        None
    };
    let payment = util::public_ref(text(order, "merchant_order_id").as_deref())
        .or_else(|| Some(PaymentRef::ProviderTxid(fk.clone())));
    Ok(WebhookOutcome {
        event_key: format!("order:{fk}:{}", util::clip(&code, 4)),
        payment,
        provider_txid: Some(fk),
        status,
        amount,
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    })
}

#[derive(Debug, Clone)]
struct Param {
    key: &'static str,
    value: String,
    number: bool,
}

impl Param {
    fn text(key: &'static str, value: impl Into<String>) -> Self {
        Param {
            key,
            value: value.into(),
            number: false,
        }
    }

    fn number(key: &'static str, value: impl ToString) -> Self {
        Param {
            key,
            value: value.to_string(),
            number: true,
        }
    }
}

pub struct Freekassa {
    http: reqwest::Client,
    api_base: String,
    refunds: Mutex<HashMap<Uuid, String>>,
}

impl Default for Freekassa {
    fn default() -> Self {
        Self::new()
    }
}

impl Freekassa {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Freekassa {
            http: http_client(),
            api_base: api_base.into().trim_end_matches('/').to_string(),
            refunds: Mutex::new(HashMap::new()),
        }
    }

    async fn send_once(
        &self,
        shop: &str,
        key: &str,
        path: &str,
        params: &[Param],
    ) -> PaymentResult<util::Reply> {
        let nonce = nonce().to_string();
        let mut all = vec![
            Param::number("shopId", shop),
            Param::number("nonce", &nonce),
        ];
        all.extend(params.iter().cloned());
        all.sort_by(|a, b| a.key.cmp(b.key));
        let pairs: Vec<(&str, &str)> = all.iter().map(|p| (p.key, p.value.as_str())).collect();
        let signature = api_signature(key, &pairs);
        let mut body = Json::object(all.iter().map(|p| {
            (
                p.key,
                if p.number {
                    Json::number(p.value.clone())
                } else {
                    Json::from(p.value.clone())
                },
            )
        }));
        body.push("signature", Json::from(signature));
        util::send(
            self.http
                .post(format!("{}{path}", self.api_base))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .body(body.to_bytes()),
        )
        .await
    }

    async fn call(
        &self,
        config: &ProviderConfig,
        path: &str,
        params: &[Param],
    ) -> PaymentResult<Json> {
        let shop = shop_id(config)?;
        let key = config.require(API_KEY)?;
        let mut reply = self.send_once(shop, key, path, params).await?;
        if nonce_rejected(&reply) {
            reply = self.send_once(shop, key, path, params).await?;
        }
        let parsed = Json::parse(&reply.body);
        let is_error = parsed
            .as_ref()
            .and_then(|p| p.get("type"))
            .and_then(Json::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("error"));
        if !reply.is_success() || is_error {
            return Err(failure(&reply));
        }
        parsed.ok_or_else(|| util::bad("ответ Freekassa не JSON"))
    }

    async fn orders(&self, config: &ProviderConfig, filter: Param) -> PaymentResult<Vec<Json>> {
        let parsed = self.call(config, "/orders", &[filter]).await?;
        Ok(parsed
            .get("orders")
            .and_then(Json::as_array)
            .map(<[Json]>::to_vec)
            .unwrap_or_default())
    }

    async fn order_by_number(
        &self,
        config: &ProviderConfig,
        fk: &str,
    ) -> PaymentResult<Option<Json>> {
        let orders = self.orders(config, Param::number("orderId", fk)).await?;
        Ok(orders
            .into_iter()
            .find(|o| text(o, "fk_order_id").as_deref() == Some(fk)))
    }

    async fn customer_ip(&self, config: &ProviderConfig, panel: Option<&str>) -> Option<IpAddr> {
        if let Some(raw) = config.get(SERVER_IP) {
            return raw.parse().ok();
        }
        let host = panel.and_then(host_of)?;
        let found: Vec<IpAddr> = tokio::net::lookup_host((host.as_str(), 443))
            .await
            .ok()?
            .map(|addr| addr.ip())
            .filter(public)
            .collect();
        found
            .iter()
            .find(|ip| ip.is_ipv4())
            .or_else(|| found.first())
            .copied()
    }

    fn form_link(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        let shop = shop_id(config)?;
        let secret = config
            .get(SECRET1)
            .ok_or_else(|| PaymentError::NotConfigured(NO_SECRET1.into()))?;
        let amount = amount_text(request.amount);
        let order = request.payment_public_id.to_string();
        let sign = form_signature(shop, &amount, secret, "RUB", &order);
        let mut url =
            reqwest::Url::parse(FORM).map_err(|e| PaymentError::NotConfigured(e.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("m", shop)
                .append_pair("oa", &amount)
                .append_pair("currency", "RUB")
                .append_pair("o", &order)
                .append_pair("s", &sign)
                .append_pair("lang", culture());
            if let Some(email) = util::client_email(request) {
                query.append_pair("em", email);
            }
        }
        Ok(Invoice {
            pay_url: url.to_string(),
            external_id: None,
            expires_at: None,
            payload: serde_json::json!({ "order": order, "form": true }),
        })
    }

    async fn api_order(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
        channel: i64,
    ) -> PaymentResult<Invoice> {
        let ip = self
            .customer_ip(config, Some(&request.webhook_url))
            .await
            .ok_or_else(|| PaymentError::NotConfigured(NO_IP.into()))?;
        let order = request.payment_public_id.to_string();
        let params = [
            Param::text("amount", amount_text(request.amount)),
            Param::text("currency", "RUB"),
            Param::text("email", buyer_email(request)),
            Param::number("i", channel),
            Param::text("ip", ip.to_string()),
            Param::text("paymentId", order.clone()),
        ];
        let parsed = self.call(config, "/orders/create", &params).await?;
        let location = text(&parsed, "location")
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("Freekassa не вернула ссылку на оплату"))?;
        let number = text(&parsed, "orderId");
        Ok(Invoice {
            pay_url: location,
            external_id: None,
            expires_at: None,
            payload: serde_json::json!({
                "order": order,
                "orderId": number,
                "orderHash": text(&parsed, "orderHash"),
                "i": channel,
            }),
        })
    }

    fn remembered(&self, key: Uuid) -> Option<String> {
        match self.refunds.lock() {
            Ok(guard) => guard.get(&key).cloned(),
            Err(poisoned) => poisoned.into_inner().get(&key).cloned(),
        }
    }

    fn remember(&self, key: Uuid, refund: &str) {
        let mut guard = match self.refunds.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(key, refund.to_string());
    }
}

fn shop_id(config: &ProviderConfig) -> PaymentResult<&str> {
    let shop = config.require(SHOP)?;
    if shop.len() <= 12 && shop.bytes().all(|b| b.is_ascii_digit()) {
        Ok(shop)
    } else {
        Err(PaymentError::NotConfigured(
            "ID магазина Freekassa — это число из настроек магазина".into(),
        ))
    }
}

#[async_trait]
impl PaymentProvider for Freekassa {
    fn id(&self) -> &'static str {
        providers::FREEKASSA
    }

    fn title(&self) -> &'static str {
        "Freekassa"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, config: &ProviderConfig) -> ClientLabel {
        if config.get(SECRET1).is_some() {
            PAGE_LABEL
        } else {
            ClientLabel {
                title: SBP.title,
                hint: SBP.hint,
            }
        }
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
            SettingField::text(
                SHOP,
                "ID магазина",
                "Кабинет Freekassa → настройки магазина, число",
            ),
            SettingField::secret(
                API_KEY,
                "API-ключ",
                "Кабинет Freekassa → настройки магазина. Нужен для оплаты по СБП, картой и SberPay, проверки статуса и возвратов",
            ),
            SettingField::secret(
                SECRET2,
                "Секретное слово 2",
                "Настройки магазина. Им Freekassa подписывает оповещения об оплате",
            ),
            SettingField::secret(
                SECRET1,
                "Секретное слово 1",
                "Настройки магазина. Нужно для кнопки «Все способы»: она ведёт на страницу Freekassa, где клиент выбирает способ сам. Если слово задано, единая кнопка тоже ведёт туда, без него — на СБП",
            )
            .optional(),
            SettingField::text(
                SERVER_IP,
                "IP-адрес сервера",
                "Freekassa просит IP покупателя при создании заказа, панель передаёт внешний IPv4 своего сервера. Пустое поле — адрес берётся из DNS адреса панели",
            )
            .optional(),
            SettingField::flag(
                CHECK_IP,
                "Оповещения только с адресов Freekassa",
                "Панель примет оповещение об оплате только с адресов Freekassa. Включайте, если сервер панели видит настоящие адреса, а не адрес своего прокси",
            ),
            SettingField::text(
                SENDER_IPS,
                "Адреса Freekassa",
                "Через запятую, на случай если Freekassa сменит адреса. Пустое поле — 168.119.157.136, 168.119.60.227, 178.154.197.79, 51.250.54.238",
            )
            .optional(),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        shop_id(config)?;
        let key_ok = config
            .get(API_KEY)
            .is_some_and(|k| k.len() <= 256 && k.bytes().all(|b| b.is_ascii_graphic()));
        if !key_ok {
            return Err(PaymentError::NotConfigured(
                "API-ключ Freekassa вставлен с лишними символами: нужны латинские буквы и цифры без пробелов".into(),
            ));
        }
        for (key, title) in [
            (SECRET1, "Секретное слово 1"),
            (SECRET2, "Секретное слово 2"),
        ] {
            if config
                .get(key)
                .is_some_and(|v| v.chars().count() > 256 || v.chars().any(char::is_control))
            {
                return Err(PaymentError::NotConfigured(format!(
                    "{title} Freekassa вставлено с лишними символами"
                )));
            }
        }
        if let Some(raw) = config.get(SERVER_IP)
            && raw.parse::<IpAddr>().is_err()
        {
            return Err(PaymentError::NotConfigured(
                "IP-адрес сервера для Freekassa указан неверно: нужен адрес вида 203.0.113.10"
                    .into(),
            ));
        }
        senders(config).map(|_| ())
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

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        match route(config, request.method.as_deref())? {
            Route::Form => self.form_link(config, request),
            Route::Api(channel) => self.api_order(config, request, channel).await,
        }
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let secret = config.require(SECRET2)?;
        let shop = shop_id(config)?;
        let pairs = crate::form::fields(request)?;
        let (Some(merchant), Some(amount), Some(order), Some(sign)) = (
            field(&pairs, "MERCHANT_ID"),
            field(&pairs, "AMOUNT"),
            field(&pairs, "MERCHANT_ORDER_ID"),
            field(&pairs, "SIGN"),
        ) else {
            return Err(PaymentError::BadSignature);
        };
        if !signed(secret, merchant, amount, order, sign) {
            return Err(PaymentError::BadSignature);
        }
        if merchant.trim() != shop {
            return Err(util::bad(format!(
                "оповещение для магазина {}, а в настройках Freekassa магазин {shop}",
                util::clip(merchant, 16)
            )));
        }
        if config.flag(CHECK_IP) {
            let allowed = senders(config)?;
            let from = request.remote_ip.map(|ip| ip.to_canonical());
            if !from.is_some_and(|ip| allowed.contains(&ip)) {
                let shown =
                    from.map_or_else(|| "неизвестного адреса".to_string(), |ip| ip.to_string());
                return Err(util::bad(format!(
                    "оповещение пришло с {shown}, а его нет в списке адресов Freekassa. Если сервер панели стоит за прокси, выключите проверку адреса в настройках Freekassa"
                )));
            }
        }
        let intid = util::safe_id(
            field(&pairs, "intid")
                .map(str::trim)
                .ok_or_else(|| util::bad("нет intid"))?,
        )?;
        let paid = rubles(amount)?;
        Ok(WebhookOutcome {
            event_key: format!("paid:{intid}"),
            payment: util::public_ref(Some(order))
                .or_else(|| Some(PaymentRef::ProviderTxid(intid.to_string()))),
            provider_txid: Some(intid.to_string()),
            status: ProviderStatus::Paid,
            amount: Some(paid),
            saved_method: None,
            reply: WebhookReply::Text {
                status: 200,
                body: "YES".into(),
            },
            signal: None,
        })
    }

    async fn confirm_repeat(
        &self,
        config: &ProviderConfig,
        outcome: &WebhookOutcome,
    ) -> PaymentResult<bool> {
        let Some(fk) = outcome
            .provider_txid
            .as_deref()
            .and_then(|txid| digits(txid).ok())
        else {
            return Ok(false);
        };
        let Some(order) = self.order_by_number(config, fk).await? else {
            return Ok(false);
        };
        let found = order_outcome(&order)?;
        Ok(found.status == ProviderStatus::Paid
            && found.payment == outcome.payment
            && found.amount == outcome.amount)
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let raw = external_id.trim();
        if let Ok(public) = Uuid::parse_str(raw) {
            let wanted = public.to_string();
            let orders = self
                .orders(config, Param::text("paymentId", wanted.clone()))
                .await?;
            let rank = |order: &Json| match text(order, "status").as_deref() {
                Some("1") => 0,
                Some("6") => 1,
                Some("0") => 2,
                _ => 3,
            };
            let found = orders
                .iter()
                .filter(|o| {
                    text(o, "merchant_order_id").is_some_and(|m| m.eq_ignore_ascii_case(&wanted))
                })
                .min_by_key(|o| rank(o));
            return match found {
                Some(order) => order_outcome(order).map(Some),
                None => Ok(Some(util::skipped(
                    format!("order:{wanted}:none"),
                    Some(PaymentRef::PublicId(public)),
                    None,
                    WebhookReply::ok(),
                ))),
            };
        }
        let fk = digits(raw)?;
        match self.order_by_number(config, fk).await? {
            Some(order) => order_outcome(&order).map(Some),
            None => Err(PaymentError::Rejected("заказ не найден в Freekassa".into())),
        }
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        if let Some(known) = self.remembered(request.key) {
            return Ok(RefundOutcome {
                provider_refund_id: Some(known),
                completed: false,
            });
        }
        let fk = digits(&request.provider_txid)?;
        let order = self
            .order_by_number(config, fk)
            .await?
            .ok_or_else(|| PaymentError::Rejected("заказ не найден в Freekassa".into()))?;
        match text(&order, "status").as_deref() {
            Some("1") => {}
            Some("6") => {
                return Err(PaymentError::Rejected(
                    "по этому заказу Freekassa уже вернула деньги".into(),
                ));
            }
            _ => {
                return Err(PaymentError::Rejected(
                    "Freekassa не считает этот заказ оплаченным, вернуть деньги нельзя".into(),
                ));
            }
        }
        let paid = util::json_amount(order.get("amount"), order.get("currency"))?;
        let mut params = vec![Param::number("orderId", fk)];
        if paid != request.amount {
            params.push(Param::text("orderAmount", amount_text(request.amount)));
        }
        let parsed = self.call(config, "/orders/refund", &params).await?;
        let id = text(&parsed, "id")
            .filter(|id| id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
            .ok_or_else(|| util::bad("Freekassa не вернула номер возврата"))?;
        let refund = format!("{fk}:{id}");
        self.remember(request.key, &refund);
        Ok(RefundOutcome {
            provider_refund_id: Some(refund),
            completed: false,
        })
    }

    async fn fetch_refund(
        &self,
        config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let Some(fk) = provider_refund_id
            .split_once(':')
            .and_then(|(fk, _)| digits(fk).ok())
        else {
            return Ok(None);
        };
        let order = self
            .order_by_number(config, fk)
            .await?
            .ok_or_else(|| PaymentError::Rejected("заказ не найден в Freekassa".into()))?;
        let event_key = format!("refund:{}", provider_refund_id.trim());
        if text(&order, "status").as_deref() != Some("6") {
            return Ok(Some(util::skipped(
                event_key,
                None,
                None,
                WebhookReply::ok(),
            )));
        }
        Ok(Some(WebhookOutcome {
            event_key,
            payment: None,
            provider_txid: Some(fk.to_string()),
            status: ProviderStatus::Refunded,
            amount: None,
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        }))
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let shop = match shop_id(config) {
            Ok(shop) => shop.to_string(),
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let shops = match self.call(config, "/shops", &[]).await {
            Ok(parsed) => parsed,
            Err(err) if util::keys_refused(&err) => {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "Freekassa не приняла API-ключ",
                )
                .detail("API-ключ и ID магазина есть в кабинете Freekassa в настройках магазина. Ключ должен быть от того же магазина");
            }
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let listed: Vec<String> = shops
            .get("shops")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|s| text(s, "id"))
            .collect();
        let mut check = if listed.is_empty() || listed.contains(&shop) {
            ConnectionCheck::ok("API-ключ принят Freekassa")
        } else {
            ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                format!("API-ключ относится к другому магазину Freekassa, а не к {shop}"),
            )
            .detail(format!(
                "Магазины этого ключа: {}",
                listed
                    .iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        };
        if let Ok(parsed) = self.call(config, "/currencies", &[]).await {
            let all = parsed
                .get("currencies")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .to_vec();
            let enabled = |id: i64| {
                all.iter().any(|c| {
                    text(c, "id").as_deref() == Some(id.to_string().as_str())
                        && text(c, "is_enabled").as_deref() == Some("1")
                })
            };
            let off: Vec<&str> = CHANNELS
                .iter()
                .filter(|(_, id)| !enabled(*id))
                .map(|(method, _)| method.title)
                .collect();
            if !all.is_empty() && !off.is_empty() {
                check = check.detail(format!(
                    "В магазине не включены способы: {}. Не отмечайте их в способах оплаты или включите их в кабинете Freekassa",
                    off.join(", ")
                ));
            }
        }
        match self
            .customer_ip(config, context.webhook_url.as_deref())
            .await
        {
            Some(ip) => {
                check = check.detail(format!(
                    "При создании заказа по API панель передаёт Freekassa адрес {ip}"
                ));
            }
            None => check = check.detail(NO_IP),
        }
        if config.get(SECRET1).is_none() {
            check = check.detail(
                "Секретное слово 1 не задано: кнопка «Все способы» недоступна, единая кнопка ведёт на СБП",
            );
        }
        check = check.detail(
            "Секретные слова этим запросом не проверить: слово 2 проверяется на оповещении об оплате, слово 1 — на странице оплаты. Тестовый режим включается в кабинете Freekassa",
        );
        match context.webhook_url.as_deref() {
            Some(url) => {
                check = check.detail(format!(
                    "В настройках магазина Freekassa укажите URL оповещения {url} и метод POST. Freekassa не даёт прочитать его по API, поэтому сверьте его сами"
                ));
            }
            None => {
                check = check.detail(
                    "Адрес панели не задан в основных настройках: Freekassa некуда присылать оповещения",
                );
            }
        }
        if config.flag(CHECK_IP) {
            check = check.detail(
                "Включена проверка адреса: оповещения принимаются только с адресов Freekassa",
            );
        }
        check.scrub(&[config.get(API_KEY).unwrap_or_default()]);
        check
    }
}

#[cfg(test)]
mod tests;
