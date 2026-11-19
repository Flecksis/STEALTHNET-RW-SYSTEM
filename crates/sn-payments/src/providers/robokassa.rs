use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util::{self, field};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha384, Sha512};
use sn_domain::client::Locale;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use uuid::Uuid;

const AUTH: &str = "https://auth.robokassa.ru";
const SERVICES: &str = "https://services.robokassa.ru";
const PAGE_PATH: &str = "/Merchant/Index.aspx";
const SERVICE_PATH: &str = "/Merchant/WebService/Service.asmx";
const REFUND_PATH: &str = "/RefundService/Refund";
const MERCHANT: &str = "merchant_login";
const PASSWORD1: &str = "password1";
const PASSWORD2: &str = "password2";
const PASSWORD3: &str = "password3";
const HASH: &str = "hash";
const TEST_MODE: &str = "test_mode";
const TEST_PASSWORD1: &str = "test_password1";
const TEST_PASSWORD2: &str = "test_password2";
const CHECK_IP: &str = "check_ip";
const HASHES: &[&str] = &["md5", "sha1", "sha256", "sha384", "sha512"];
const HASH_LABELS: &[&str] = &["MD5", "SHA-1", "SHA-256", "SHA-384", "SHA-512"];
const SENDERS: [Ipv4Addr; 2] = [
    Ipv4Addr::new(185, 59, 216, 65),
    Ipv4Addr::new(185, 59, 217, 65),
];
const DESCRIPTION_MAX: usize = 100;
const FIELD_MAX: usize = 100;
const INVOICE_MASK: u64 = (1 << 53) - 1;
const CARD: PayMethod = PayMethod::new("card", "Карта", "Карты российских банков");
const SBP: PayMethod = PayMethod::new("sbp", "СБП", "Через приложение банка");
const SBERPAY: PayMethod = PayMethod::new("sberpay", "SberPay", "Через СберБанк Онлайн");
const TPAY: PayMethod = PayMethod::new("tpay", "T-Pay", "Через приложение Т-Банка");
const PAGE: PayMethod = PayMethod::new(
    "page",
    "Все способы",
    "Способ выбирается на странице оплаты",
);
const CHANNELS: [(PayMethod, &str); 4] = [
    (CARD, "BankCard"),
    (SBP, "SBP"),
    (SBERPAY, "SberPay"),
    (TPAY, "TinkoffPay"),
];
const NOT_FOUND: &str = "3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Algorithm {
    const ALL: [Algorithm; 5] = [
        Algorithm::Md5,
        Algorithm::Sha1,
        Algorithm::Sha256,
        Algorithm::Sha384,
        Algorithm::Sha512,
    ];

    fn code(self) -> &'static str {
        match self {
            Algorithm::Md5 => "md5",
            Algorithm::Sha1 => "sha1",
            Algorithm::Sha256 => "sha256",
            Algorithm::Sha384 => "sha384",
            Algorithm::Sha512 => "sha512",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Algorithm::Md5 => "MD5",
            Algorithm::Sha1 => "SHA-1",
            Algorithm::Sha256 => "SHA-256",
            Algorithm::Sha384 => "SHA-384",
            Algorithm::Sha512 => "SHA-512",
        }
    }

    fn of(config: &ProviderConfig) -> PaymentResult<Algorithm> {
        match config.get(HASH) {
            None => Ok(Algorithm::Md5),
            Some(code) => Algorithm::ALL
                .into_iter()
                .find(|a| a.code() == code.to_ascii_lowercase())
                .ok_or_else(|| {
                    PaymentError::NotConfigured(
                        "алгоритм хеша Robokassa: MD5, SHA-1, SHA-256, SHA-384 или SHA-512".into(),
                    )
                }),
        }
    }

    fn digest(self, text: &str) -> Vec<u8> {
        let bytes = text.as_bytes();
        match self {
            Algorithm::Md5 => crate::md5::digest(bytes).to_vec(),
            Algorithm::Sha1 => sha1::Sha1::digest(bytes).to_vec(),
            Algorithm::Sha256 => Sha256::digest(bytes).to_vec(),
            Algorithm::Sha384 => Sha384::digest(bytes).to_vec(),
            Algorithm::Sha512 => Sha512::digest(bytes).to_vec(),
        }
    }

    fn hex(self, text: &str) -> String {
        hex::encode(self.digest(text))
    }
}

pub fn signature(algorithm: &str, base: &str) -> String {
    Algorithm::ALL
        .into_iter()
        .find(|a| a.code() == algorithm)
        .map(|a| a.hex(base))
        .unwrap_or_default()
}

pub fn invoice_number(public_id: Uuid) -> u64 {
    let bytes = public_id.as_bytes();
    let mut tail = [0u8; 8];
    tail.copy_from_slice(&bytes[8..16]);
    match u64::from_be_bytes(tail) & INVOICE_MASK {
        0 => 1,
        number => number,
    }
}

fn invoice_id(raw: &str) -> PaymentResult<&str> {
    let raw = raw.trim();
    let valid = !raw.is_empty()
        && raw.len() <= 19
        && raw.bytes().all(|b| b.is_ascii_digit())
        && !raw.starts_with('0');
    if valid {
        Ok(raw)
    } else {
        Err(util::bad("номер счёта Robokassa не число"))
    }
}

fn sum(raw: &str) -> PaymentResult<Amount> {
    let raw = raw.trim();
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    let extra = fraction.get(2..).unwrap_or_default();
    if !extra.bytes().all(|b| b == b'0') {
        return Err(util::bad(format!(
            "сумма «{}» точнее копейки",
            util::clip(raw, 32)
        )));
    }
    let cents = fraction.get(..2).unwrap_or(fraction);
    let normalized = if cents.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{cents}")
    };
    Ok(Amount::new(
        util::minor(&normalized, PayCurrency::Rub)?,
        PayCurrency::Rub,
    ))
}

fn plain_description(text: &str) -> String {
    let mapped: String = text
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '.' | ',' | '-' | '(' | ')' | '№' | '+') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let compact = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    let clipped = util::clip(&compact, DESCRIPTION_MAX).trim().to_string();
    if clipped.is_empty() {
        "Оплата заказа".to_string()
    } else {
        clipped
    }
}

fn culture() -> &'static str {
    match sn_domain::lang::current() {
        Locale::En => "en",
        Locale::Ru => "ru",
    }
}

fn label(method: Option<&str>) -> PaymentResult<Option<&'static str>> {
    match method {
        None => Ok(None),
        Some(code) if code == PAGE.code => Ok(None),
        Some(code) => CHANNELS
            .iter()
            .find(|(m, _)| m.code == code)
            .map(|(_, label)| Some(*label))
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

fn custom_fields(pairs: &[(String, String)]) -> String {
    let mut custom: Vec<&(String, String)> = pairs
        .iter()
        .filter(|(k, _)| k.len() > 4 && k.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("shp_")))
        .collect();
    custom.sort_by(|a, b| a.0.cmp(&b.0));
    custom
        .iter()
        .map(|(k, v)| format!(":{k}={v}"))
        .collect::<String>()
}

fn sender_allowed(ip: Option<IpAddr>) -> bool {
    match ip.map(|ip| ip.to_canonical()) {
        Some(IpAddr::V4(v4)) => SENDERS.contains(&v4),
        _ => false,
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let Some(end) = tail.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn element<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut from = 0;
    while let Some(found) = xml.get(from..).and_then(|s| s.find(&open)) {
        let start = from + found;
        let after = start + open.len();
        match xml.as_bytes().get(after) {
            Some(b'>') => {
                let body = after + 1;
                let end = xml.get(body..)?.find(&close)? + body;
                return xml.get(body..end);
            }
            Some(b' ' | b'\t' | b'\r' | b'\n' | b'/') => {
                let tag_end = xml.get(after..)?.find('>')? + after;
                if xml.as_bytes().get(tag_end.checked_sub(1)?) == Some(&b'/') {
                    return Some("");
                }
                let body = tag_end + 1;
                let end = xml.get(body..)?.find(&close)? + body;
                return xml.get(body..end);
            }
            _ => from = after,
        }
    }
    None
}

fn xml_text(xml: &str, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(xml, |inner, name| element(inner, name))
        .map(|t| unescape(t.trim()))
        .filter(|t| !t.is_empty())
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!(" {name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let end = tag.get(start..)?.find('"')? + start;
    Some(unescape(tag.get(start..end)?))
}

struct Offer {
    alias: Option<String>,
    name: Option<String>,
}

fn offers(xml: &str) -> Vec<Offer> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Currency ") {
        let tail = &rest[start..];
        let Some(end) = tail.find('>') else {
            break;
        };
        let tag = &tail[..end];
        found.push(Offer {
            alias: attribute(tag, "Alias"),
            name: attribute(tag, "Name"),
        });
        rest = &tail[end..];
    }
    found
}

fn result_code(xml: &str) -> Option<String> {
    xml_text(xml, &["Result", "Code"])
}

fn result_failure(code: &str, xml: &str) -> PaymentError {
    let described = xml_text(xml, &["Result", "Description"]);
    match code {
        "1" => PaymentError::Rejected(
            "Robokassa не приняла подпись запроса: проверьте пароль №2 и алгоритм хеша".into(),
        ),
        "2" => PaymentError::Rejected(
            "Robokassa не нашла магазин с этим идентификатором или он не активирован".into(),
        ),
        "3" => PaymentError::Rejected("операция не найдена в Robokassa".into()),
        "4" => PaymentError::Rejected(
            "в Robokassa две операции с этим номером счёта, проверьте их в кабинете".into(),
        ),
        "1000" => PaymentError::Http { status: 503 },
        other => PaymentError::Rejected(match described {
            Some(text) => format!(
                "Robokassa ответила кодом {}: {}",
                util::clip(other, 8),
                util::clip(&text, 160)
            ),
            None => format!("Robokassa ответила кодом {}", util::clip(other, 8)),
        }),
    }
}

fn refund_message(message: Option<&str>) -> String {
    match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some("NotEnoughOperationFunds") => {
            "по этой операции в Robokassa не осталось денег для возврата".into()
        }
        Some(text) if text.to_ascii_lowercase().contains("opkey") => {
            "Robokassa не нашла операцию для возврата".into()
        }
        Some(text) => format!("Robokassa отказала в возврате: {}", util::clip(text, 160)),
        None => "Robokassa отказала в возврате без пояснения".into(),
    }
}

fn jwt(secret: &str, payload: &Json) -> PaymentResult<String> {
    let header = Json::object([("alg", Json::from("HS256")), ("typ", Json::from("JWT"))]);
    let signed = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_bytes()),
        URL_SAFE_NO_PAD.encode(payload.to_bytes())
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| PaymentError::NotConfigured("пароль №3 Robokassa не подходит".into()))?;
    mac.update(signed.as_bytes());
    Ok(format!(
        "{signed}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}

fn passable(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= FIELD_MAX
        && value.bytes().all(|b| b.is_ascii_graphic() && b != b':')
}

struct Operation {
    state: Option<String>,
    paid: Option<Amount>,
    op_key: Option<String>,
}

fn operation(xml: &str) -> PaymentResult<Operation> {
    let paid = match xml_text(xml, &["Info", "OutSum"]) {
        Some(raw) => Some(sum(&raw)?),
        None => None,
    };
    Ok(Operation {
        state: xml_text(xml, &["State", "Code"]),
        paid,
        op_key: xml_text(xml, &["Info", "OpKey"]),
    })
}

fn state_outcome(inv: &str, op: &Operation) -> WebhookOutcome {
    let state = op.state.as_deref().unwrap_or("5");
    let status = match state {
        "100" => ProviderStatus::Paid,
        "10" => ProviderStatus::Canceled,
        "60" => ProviderStatus::Failed,
        _ => ProviderStatus::Pending,
    };
    WebhookOutcome {
        event_key: format!("state:{inv}:{}", util::clip(state, 8)),
        payment: Some(PaymentRef::ProviderTxid(inv.to_string())),
        provider_txid: Some(inv.to_string()),
        amount: if status == ProviderStatus::Paid {
            op.paid
        } else {
            None
        },
        status,
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    }
}

pub struct Robokassa {
    http: reqwest::Client,
    auth: String,
    services: String,
    refunds: Mutex<HashMap<Uuid, String>>,
}

impl Default for Robokassa {
    fn default() -> Self {
        Self::new()
    }
}

impl Robokassa {
    pub fn new() -> Self {
        Robokassa {
            http: http_client(),
            auth: AUTH.to_string(),
            services: SERVICES.to_string(),
            refunds: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        let base = api_base.into().trim_end_matches('/').to_string();
        Robokassa {
            http: http_client(),
            auth: base.clone(),
            services: base,
            refunds: Mutex::new(HashMap::new()),
        }
    }

    async fn service(&self, method: &str, query: &[(&str, &str)]) -> PaymentResult<String> {
        let reply = util::send(
            self.http
                .get(format!("{}{SERVICE_PATH}/{method}", self.auth))
                .query(query),
        )
        .await?;
        if !reply.is_success() {
            return Err(reply.failure(None));
        }
        String::from_utf8(reply.body.to_vec()).map_err(|_| util::bad("ответ Robokassa не в UTF-8"))
    }

    async fn op_state_with(
        &self,
        config: &ProviderConfig,
        inv: &str,
        algorithm: Algorithm,
    ) -> PaymentResult<String> {
        let login = config.require(MERCHANT)?;
        let password = config.require(PASSWORD2)?;
        let signature = algorithm.hex(&format!("{login}:{inv}:{password}"));
        self.service(
            "OpStateExt",
            &[
                ("MerchantLogin", login),
                ("InvoiceID", inv),
                ("Signature", signature.as_str()),
            ],
        )
        .await
    }

    async fn op_state(&self, config: &ProviderConfig, inv: &str) -> PaymentResult<String> {
        self.op_state_with(config, inv, Algorithm::of(config)?)
            .await
    }

    fn remembered(&self, key: Uuid) -> Option<String> {
        match self.refunds.lock() {
            Ok(guard) => guard.get(&key).cloned(),
            Err(poisoned) => poisoned.into_inner().get(&key).cloned(),
        }
    }

    fn remember(&self, key: Uuid, request: &str) {
        let mut guard = match self.refunds.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(key, request.to_string());
    }

    async fn probe(&self, config: &ProviderConfig) -> PaymentResult<Option<Algorithm>> {
        let configured = Algorithm::of(config)?;
        let probe = invoice_number(Uuid::new_v4()).to_string();
        let xml = self.op_state_with(config, &probe, configured).await?;
        match result_code(&xml).as_deref() {
            Some("0" | NOT_FOUND | "4") => return Ok(Some(configured)),
            Some("1") => {}
            Some(code) => return Err(result_failure(code, &xml)),
            None => return Err(util::bad("в ответе Robokassa нет кода результата")),
        }
        for other in Algorithm::ALL.into_iter().filter(|a| *a != configured) {
            let xml = self.op_state_with(config, &probe, other).await?;
            if matches!(result_code(&xml).as_deref(), Some("0" | NOT_FOUND | "4")) {
                return Ok(Some(other));
            }
        }
        Ok(None)
    }

    fn usable(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        Algorithm::of(config)?;
        let fields = [
            (MERCHANT, "Идентификатор магазина"),
            (PASSWORD1, "Пароль №1"),
            (PASSWORD2, "Пароль №2"),
            (PASSWORD3, "Пароль №3"),
            (TEST_PASSWORD1, "Тестовый пароль №1"),
            (TEST_PASSWORD2, "Тестовый пароль №2"),
        ];
        for (key, title) in fields {
            if config.get(key).is_some_and(|v| !passable(v)) {
                return Err(PaymentError::NotConfigured(format!(
                    "{title} Robokassa вставлен с лишними символами: нужны латинские буквы, цифры и знаки без пробелов и двоеточий"
                )));
            }
        }
        if config.flag(TEST_MODE)
            && (config.get(TEST_PASSWORD1).is_none() || config.get(TEST_PASSWORD2).is_none())
        {
            return Err(PaymentError::NotConfigured(
                "для тестового режима Robokassa заполните тестовые пароли №1 и №2".into(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl PaymentProvider for Robokassa {
    fn id(&self) -> &'static str {
        providers::ROBOKASSA
    }

    fn title(&self) -> &'static str {
        "Robokassa"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: CARD.title,
            hint: "Карты российских банков и СБП",
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
                MERCHANT,
                "Идентификатор магазина",
                "Кабинет Robokassa → Мои магазины → Технические настройки",
            ),
            SettingField::secret(
                PASSWORD1,
                "Пароль №1",
                "Технические настройки магазина. Им подписывается ссылка на оплату",
            ),
            SettingField::secret(
                PASSWORD2,
                "Пароль №2",
                "Технические настройки магазина. Им Robokassa подписывает уведомления об оплате",
            ),
            SettingField::select(
                HASH,
                "Алгоритм хеша",
                "Как в поле «Алгоритм расчёта хеша» в технических настройках магазина. RIPEMD-160 панель не поддерживает",
                HASHES,
            )
            .labels(HASH_LABELS)
            .with_default("md5"),
            SettingField::secret(
                PASSWORD3,
                "Пароль №3",
                "Нужен для возвратов из панели, создаётся в технических настройках магазина. Без него деньги возвращают в кабинете Robokassa",
            )
            .optional(),
            SettingField::flag(
                TEST_MODE,
                "Тестовый режим",
                "Ссылки на оплату создаются тестовыми и подписываются тестовыми паролями, деньги не списываются. Тестовые оплаты засчитываются только в этом режиме",
            ),
            SettingField::secret(
                TEST_PASSWORD1,
                "Тестовый пароль №1",
                "Технические настройки магазина → Параметры проведения тестовых платежей",
            )
            .optional(),
            SettingField::secret(
                TEST_PASSWORD2,
                "Тестовый пароль №2",
                "Там же, где тестовый пароль №1",
            )
            .optional(),
            SettingField::flag(
                CHECK_IP,
                "Уведомления только с адресов Robokassa",
                "Панель примет уведомление об оплате только с 185.59.216.65 и 185.59.217.65. Включайте, если сервер панели видит настоящие адреса, а не адрес своего прокси",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.usable(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        self.usable(config)?;
        if config.get(TEST_PASSWORD2).is_some()
            && config.get(TEST_PASSWORD2) == config.get(PASSWORD2)
        {
            return Err(PaymentError::NotConfigured(
                "тестовый пароль №2 Robokassa совпадает с боевым паролем №2: задайте в Robokassa разные пароли".into(),
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

    fn supports_refund_polling(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        self.usable(config)?;
        let chosen = label(request.method.as_deref())?;
        let login = config.require(MERCHANT)?;
        let test = config.flag(TEST_MODE);
        let password = if test {
            config.require(TEST_PASSWORD1)?
        } else {
            config.require(PASSWORD1)?
        };
        let algorithm = Algorithm::of(config)?;
        let number = invoice_number(request.payment_public_id).to_string();
        let out_sum = request.amount.units();
        let signature = algorithm.hex(&format!("{login}:{out_sum}:{number}:{password}"));
        let mut url = reqwest::Url::parse(&format!("{}{PAGE_PATH}", self.auth))
            .map_err(|e| PaymentError::NotConfigured(e.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("MerchantLogin", login)
                .append_pair("OutSum", &out_sum)
                .append_pair("InvId", &number)
                .append_pair("Description", &plain_description(&request.description))
                .append_pair("SignatureValue", &signature)
                .append_pair("Culture", culture())
                .append_pair("Encoding", "utf-8");
            if let Some(email) = util::client_email(request) {
                query.append_pair("Email", email);
            }
            if let Some(code) = chosen {
                query.append_pair("IncCurrLabel", code);
            }
            if test {
                query.append_pair("IsTest", "1");
            }
        }
        Ok(Invoice {
            pay_url: url.to_string(),
            external_id: Some(number.clone()),
            expires_at: None,
            payload: serde_json::json!({
                "inv_id": number,
                "test": test,
                "method": chosen,
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let main = config.require(PASSWORD2)?;
        let algorithm = Algorithm::of(config)?;
        let pairs = crate::form::fields(request)?;
        let (Some(out_sum), Some(inv), Some(signed)) = (
            field(&pairs, "OutSum"),
            field(&pairs, "InvId"),
            field(&pairs, "SignatureValue"),
        ) else {
            return Err(PaymentError::BadSignature);
        };
        let tail = custom_fields(&pairs);
        let signed_with = |password: &str| {
            let expected = algorithm.hex(&format!("{out_sum}:{inv}:{password}{tail}"));
            util::secret_eq(&expected, &signed.trim().to_ascii_lowercase())
        };
        let genuine = signed_with(main);
        let test = !genuine
            && config.flag(TEST_MODE)
            && config.get(TEST_PASSWORD2).is_some_and(signed_with);
        if !genuine && !test {
            return Err(PaymentError::BadSignature);
        }
        if config.flag(CHECK_IP) && !sender_allowed(request.remote_ip) {
            let shown = request
                .remote_ip
                .map_or_else(|| "неизвестного адреса".to_string(), |ip| ip.to_string());
            return Err(util::bad(format!(
                "уведомление пришло с {shown}, а Robokassa присылает их с 185.59.216.65 и 185.59.217.65. Если сервер панели стоит за прокси, выключите проверку адреса в настройках Robokassa"
            )));
        }
        let inv = invoice_id(inv)?;
        let reply = WebhookReply::Text {
            status: 200,
            body: format!("OK{inv}"),
        };
        let flagged = field(&pairs, "IsTest")
            .map(str::trim)
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        if flagged && !config.flag(TEST_MODE) {
            return Ok(util::skipped(format!("test:{inv}"), None, None, reply));
        }
        let amount = sum(out_sum)?;
        Ok(WebhookOutcome {
            event_key: format!("paid:{inv}"),
            payment: Some(PaymentRef::ProviderTxid(inv.to_string())),
            provider_txid: Some(inv.to_string()),
            status: ProviderStatus::Paid,
            amount: Some(amount),
            saved_method: None,
            reply,
            signal: None,
        })
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let inv = invoice_id(external_id)?;
        let xml = self.op_state(config, inv).await?;
        match result_code(&xml).as_deref() {
            Some("0") => Ok(Some(state_outcome(inv, &operation(&xml)?))),
            Some(NOT_FOUND) => Ok(Some(util::skipped(
                format!("state:{inv}:none"),
                Some(PaymentRef::ProviderTxid(inv.to_string())),
                Some(inv.to_string()),
                WebhookReply::ok(),
            ))),
            Some(code) => Err(result_failure(code, &xml)),
            None => Err(util::bad("в ответе Robokassa нет кода результата")),
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
        let password = config.get(PASSWORD3).ok_or_else(|| {
            PaymentError::NotConfigured(
                "для возвратов из панели нужен пароль №3 Robokassa: создайте его в технических настройках магазина или верните деньги в кабинете Robokassa".into(),
            )
        })?;
        let inv = invoice_id(&request.provider_txid)?;
        let xml = self.op_state(config, inv).await?;
        match result_code(&xml).as_deref() {
            Some("0") => {}
            Some(code) => return Err(result_failure(code, &xml)),
            None => return Err(util::bad("в ответе Robokassa нет кода результата")),
        }
        let op = operation(&xml)?;
        if op.state.as_deref() != Some("100") {
            return Err(PaymentError::Rejected(
                "Robokassa ещё не зачислила эту оплату, вернуть её пока нельзя".into(),
            ));
        }
        let op_key = op
            .op_key
            .clone()
            .ok_or_else(|| util::bad("Robokassa не вернула ключ операции для возврата"))?;
        let mut payload = Json::object([("OpKey", Json::from(op_key))]);
        if op.paid != Some(request.amount) {
            payload.push("RefundSum", Json::number(request.amount.units()));
        }
        let token = jwt(password, &payload)?;
        let reply = util::send(
            self.http
                .post(format!("{}{REFUND_PATH}/Create", self.services))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(Json::from(token).to_bytes()),
        )
        .await?;
        let parsed = Json::parse(&reply.body);
        let message = refund_message(
            parsed
                .as_ref()
                .and_then(|p| p.get("message"))
                .and_then(Json::as_str),
        );
        if !reply.is_success() {
            return Err(reply.failure(Some(&message)));
        }
        let parsed = parsed.ok_or_else(|| util::bad("ответ Robokassa на возврат не JSON"))?;
        if parsed.get("success").and_then(Json::as_bool) != Some(true) {
            return Err(PaymentError::Rejected(message));
        }
        let id = parsed
            .get("requestId")
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|id| Uuid::parse_str(id).is_ok())
            .ok_or_else(|| util::bad("Robokassa не вернула номер заявки на возврат"))?
            .to_string();
        self.remember(request.key, &id);
        Ok(RefundOutcome {
            provider_refund_id: Some(id),
            completed: false,
        })
    }

    async fn fetch_refund(
        &self,
        _config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let Ok(id) = Uuid::parse_str(provider_refund_id.trim()) else {
            return Ok(None);
        };
        let id = id.to_string();
        let reply = util::send(
            self.http
                .get(format!("{}{REFUND_PATH}/GetState", self.services))
                .query(&[("id", id.as_str())]),
        )
        .await?;
        let parsed = Json::parse(&reply.body);
        if !reply.is_success() || parsed.as_ref().is_some_and(|p| p.get("label").is_none()) {
            let message = parsed
                .as_ref()
                .and_then(|p| p.get("message"))
                .and_then(Json::as_str);
            return Err(match message {
                Some(text) if text.to_ascii_lowercase().contains("does not exist") => {
                    PaymentError::Rejected("заявка на возврат не найдена в Robokassa".into())
                }
                other => reply.failure(other),
            });
        }
        let parsed = parsed.ok_or_else(|| util::bad("ответ Robokassa о возврате не JSON"))?;
        let event_key = format!("refund:{id}");
        let label = parsed
            .get("label")
            .and_then(Json::as_str)
            .map(|l| l.trim().to_ascii_lowercase());
        let (status, amount) = match label.as_deref() {
            Some("finished") => {
                let refunded = parsed
                    .get("amount")
                    .and_then(Json::scalar_text)
                    .ok_or_else(|| util::bad("Robokassa не сообщила сумму возврата"))
                    .and_then(sum)?;
                (
                    ProviderStatus::PartiallyRefunded { refunded },
                    Some(refunded),
                )
            }
            Some("canceled" | "cancelled") => (ProviderStatus::RefundDeclined, None),
            _ => {
                return Ok(Some(util::skipped(
                    event_key,
                    None,
                    None,
                    WebhookReply::ok(),
                )));
            }
        };
        Ok(Some(WebhookOutcome {
            event_key,
            payment: None,
            provider_txid: None,
            status,
            amount,
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
        let Some(login) = config.get(MERCHANT) else {
            return ConnectionCheck::failed(&PaymentError::NotConfigured(
                "не задан идентификатор магазина".into(),
            ));
        };
        let xml = match self
            .service(
                "GetCurrencies",
                &[("MerchantLogin", login), ("Language", "ru")],
            )
            .await
        {
            Ok(xml) => xml,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        match result_code(&xml).as_deref() {
            Some("0") => {}
            Some("2") => {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    format!(
                        "Robokassa не нашла магазин «{}» или он ещё не активирован",
                        util::clip(login, 64)
                    ),
                )
                .detail("Идентификатор магазина есть в кабинете Robokassa: Мои магазины → Технические настройки");
            }
            Some(code) => return ConnectionCheck::failed(&result_failure(code, &xml)),
            None => {
                return ConnectionCheck::failed(&util::bad(
                    "в ответе Robokassa нет кода результата",
                ));
            }
        }
        let methods: Vec<String> = offers(&xml)
            .into_iter()
            .filter_map(|c| c.name.or(c.alias))
            .collect();
        let configured = match Algorithm::of(config) {
            Ok(algorithm) => algorithm,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let mut check = match self.probe(config).await {
            Ok(Some(found)) if found == configured => {
                ConnectionCheck::ok("Магазин найден, пароль №2 и алгоритм хеша приняты Robokassa")
            }
            Ok(Some(found)) => ConnectionCheck::new(
                CheckStatus::SettingsRejected,
                format!(
                    "В Robokassa выбран алгоритм хеша {}, а в панели {}",
                    found.title(),
                    configured.title()
                ),
            )
            .detail("Выберите в панели тот же алгоритм, что в технических настройках магазина, иначе ссылки на оплату и уведомления не пройдут проверку подписи"),
            Ok(None) => ConnectionCheck::new(
                CheckStatus::KeysRejected,
                "Магазин найден, но Robokassa не приняла пароль №2",
            )
            .detail("Скопируйте пароль №2 из технических настроек магазина заново. Если в Robokassa выбран RIPEMD-160, смените его на MD5 или SHA-256: этот алгоритм панель не поддерживает"),
            Err(err) => return ConnectionCheck::failed(&err),
        };
        if !methods.is_empty() {
            let shown: Vec<String> = methods.iter().take(12).cloned().collect();
            check = check.detail(format!("Способы оплаты магазина: {}", shown.join(", ")));
        }
        check = check.detail(
            "Пароль №1 этим запросом не проверить: он проверяется на странице оплаты. Если после перехода на оплату Robokassa пишет об ошибке 29, пароль №1 или алгоритм хеша не совпадают",
        );
        if config.get(PASSWORD3).is_none() {
            check = check.detail(
                "Пароль №3 не задан: возвраты из панели недоступны, деньги можно вернуть в кабинете Robokassa",
            );
        }
        if config.flag(TEST_MODE) {
            check = check.detail(
                "Включён тестовый режим: ссылки создаются тестовыми, деньги не списываются. Тестовые пароли проверяются только тестовой оплатой. Перед приёмом настоящих оплат выключите режим",
            );
        }
        match context.webhook_url.as_deref() {
            Some(url) => {
                check = check.detail(format!(
                    "В технических настройках магазина укажите Result URL {url}, метод POST. Robokassa не даёт прочитать этот адрес по API, поэтому сверьте его сами"
                ));
            }
            None => {
                check = check.detail(
                    "Адрес панели не задан в основных настройках: Robokassa некуда присылать уведомления",
                );
            }
        }
        if config.flag(CHECK_IP) {
            check = check.detail(
                "Включена проверка адреса: уведомления принимаются только с 185.59.216.65 и 185.59.217.65",
            );
        }
        check
    }
}

#[cfg(test)]
mod tests;
