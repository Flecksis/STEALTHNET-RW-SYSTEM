use crate::error::{PaymentError, PaymentResult};
use crate::provider::*;
use async_trait::async_trait;
use base64::Engine as _;
use chrono::Utc;
use serde::Deserialize;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const NANO_PER_GRAM: u128 = 1_000_000_000;
const STEP_NANO: u128 = 10_000_000;
const MICRO_PER_UNIT: u128 = 1_000_000;
const MICRO_PER_MINOR: u128 = 10_000;
const PRICE_MAX_MICRO: u128 = 1_000_000_000_000_000;
const RATES_TTL: Duration = Duration::from_secs(300);
const TONCENTER: &str = "https://toncenter.com/api/v2";
const TONCENTER_TESTNET: &str = "https://testnet.toncenter.com/api/v2";
const TONAPI: &str = "https://tonapi.io";
const COINGECKO: &str = "https://api.coingecko.com";
const TONKEEPER: &str = "https://app.tonkeeper.com/transfer";
const COMMENT_PREFIX: &str = "SN-";
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const TRANSACTIONS_LIMIT: u32 = 100;
const TRANSACTIONS_RETRY_LIMIT: u32 = 30;
const CHAIN_TTL: Duration = Duration::from_secs(5);
const REJECTED_KEY_PAUSE: Duration = Duration::from_secs(300);
const LOG_EVERY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GramInvoice {
    pub comment: String,
    pub nano: u128,
    pub address: String,
    pub amount: Amount,
}

impl GramInvoice {
    pub fn external_id(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.comment,
            self.nano,
            self.address,
            self.amount.minor,
            self.amount.currency.code()
        )
    }

    pub fn parse(raw: &str) -> Option<GramInvoice> {
        let mut parts = raw.split('|');
        let comment = parts.next()?.to_string();
        let nano = parts.next()?.parse::<u128>().ok()?;
        let address = parts.next()?.to_string();
        let minor = parts.next()?.parse::<i64>().ok()?;
        let currency = PayCurrency::parse(parts.next()?).ok()?;
        if parts.next().is_some()
            || !comment.starts_with(COMMENT_PREFIX)
            || nano == 0
            || !valid_address(&address)
        {
            return None;
        }
        Some(GramInvoice {
            comment,
            nano,
            address,
            amount: Amount::new(minor, currency),
        })
    }

    pub fn grams(&self) -> String {
        format_grams(self.nano)
    }

    pub fn ton_link(&self) -> String {
        format!(
            "ton://transfer/{}?amount={}&text={}",
            self.address,
            self.nano,
            encode(&self.comment)
        )
    }

    pub fn tonkeeper_link(&self) -> String {
        format!(
            "{TONKEEPER}/{}?amount={}&text={}",
            self.address,
            self.nano,
            encode(&self.comment)
        )
    }

    pub fn wallet_transaction(&self, valid_until: i64) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "valid_until": valid_until,
            "messages": [{
                "address": friendly_address(&self.address)?,
                "amount": self.nano.to_string(),
                "payload": comment_payload(&self.comment)?,
            }],
        }))
    }
}

const COMMENT_BYTES_MAX: usize = 123;

pub fn comment_payload(text: &str) -> Option<String> {
    if text.len() > COMMENT_BYTES_MAX {
        return None;
    }
    let mut data = vec![0u8; 4];
    data.extend_from_slice(text.as_bytes());
    cell_boc(&data)
}

fn cell_boc(data: &[u8]) -> Option<String> {
    let doubled = u8::try_from(data.len() * 2).ok()?;
    let mut cell = vec![0u8, doubled];
    cell.extend_from_slice(data);
    let mut boc = vec![0xb5, 0xee, 0x9c, 0x72, 0x41, 1, 1, 1, 0];
    boc.push(u8::try_from(cell.len()).ok()?);
    boc.push(0);
    boc.extend_from_slice(&cell);
    let crc = crc32c(&boc);
    boc.extend_from_slice(&crc.to_le_bytes());
    Some(base64::engine::general_purpose::STANDARD.encode(boc))
}

pub fn checksum_ok(address: &str) -> bool {
    let address = address.trim();
    if address.contains(':') {
        return true;
    }
    let normalized: String = address
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    match base64::engine::general_purpose::URL_SAFE.decode(normalized) {
        Ok(bytes) if bytes.len() == 36 => crc16(&bytes[..34]).to_be_bytes() == bytes[34..],
        _ => false,
    }
}

pub fn friendly_address(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !valid_address(raw) {
        return None;
    }
    let Some((workchain, hex)) = raw.split_once(':') else {
        return Some(raw.to_string());
    };
    let workchain = workchain.parse::<i8>().ok()?;
    let mut bytes = Vec::with_capacity(36);
    bytes.push(0x51);
    bytes.push(workchain.to_be_bytes()[0]);
    for pair in hex.as_bytes().chunks(2) {
        bytes.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
    }
    let crc = crc16(&bytes);
    bytes.extend_from_slice(&crc.to_be_bytes());
    Some(base64::engine::general_purpose::URL_SAFE.encode(bytes))
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x1021
            };
        }
    }
    crc
}

fn crc32c(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0x82F6_3B78
            };
        }
    }
    !crc
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn format_grams(nano: u128) -> String {
    let whole = nano / NANO_PER_GRAM;
    let fraction = nano % NANO_PER_GRAM;
    if fraction == 0 {
        return whole.to_string();
    }
    let digits = format!("{fraction:09}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

pub fn valid_address(raw: &str) -> bool {
    let raw = raw.trim();
    if let Some((workchain, hex)) = raw.split_once(':') {
        let workchain = workchain.strip_prefix('-').unwrap_or(workchain);
        return !workchain.is_empty()
            && workchain.len() <= 3
            && workchain.bytes().all(|b| b.is_ascii_digit())
            && hex.len() == 64
            && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    raw.len() == 48
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'+' | b'/'))
}

pub fn comment_for(payment: Uuid) -> String {
    let bytes = payment.as_bytes();
    let mut bits: u64 = 0;
    for byte in &bytes[..5] {
        bits = (bits << 8) | u64::from(*byte);
    }
    let mut out = String::from(COMMENT_PREFIX);
    for index in (0..8).rev() {
        let value = ((bits >> (index * 5)) & 0x1F) as usize;
        out.push(BASE32[value] as char);
    }
    out
}

pub fn parse_price(raw: &str) -> Option<u128> {
    let raw = raw.trim().replace(',', ".");
    let (whole, fraction) = match raw.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (raw.as_str(), ""),
    };
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
        || whole.len() > 12
    {
        return None;
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut fraction_micro: u128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().ok()?
    };
    for _ in fraction.len()..6 {
        fraction_micro *= 10;
    }
    let micro = whole * MICRO_PER_UNIT + fraction_micro;
    (micro > 0 && micro < PRICE_MAX_MICRO).then_some(micro)
}

fn price_from_float(value: f64) -> Option<u128> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let micro = (value * MICRO_PER_UNIT as f64).round();
    if micro < 1.0 || micro >= PRICE_MAX_MICRO as f64 {
        return None;
    }
    Some(micro as u128)
}

pub fn to_nano(minor: i64, price_micro: u128) -> Option<u128> {
    if minor <= 0 || price_micro == 0 {
        return None;
    }
    let amount_micro = (minor as u128) * MICRO_PER_MINOR;
    let nano = (amount_micro * NANO_PER_GRAM).div_ceil(price_micro);
    Some(nano.div_ceil(STEP_NANO) * STEP_NANO)
}

fn page_url(webhook_url: &str, payment: Uuid) -> Option<String> {
    let (base, _) = webhook_url.split_once("/api/webhooks/pay/")?;
    (base.starts_with("https://") || base.starts_with("http://"))
        .then(|| format!("{}/pay/{payment}", base.trim_end_matches('/')))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rates {
    usd: u128,
    rub: u128,
}

impl Rates {
    fn get(self, currency: Currency) -> u128 {
        match currency {
            Currency::Usd => self.usd,
            Currency::Rub => self.rub,
        }
    }
}

#[derive(Deserialize)]
struct TonapiRates {
    rates: HashMap<String, TonapiToken>,
}

#[derive(Deserialize)]
struct TonapiToken {
    prices: HashMap<String, f64>,
}

#[derive(Deserialize)]
struct ToncenterResponse {
    ok: bool,
    #[serde(default)]
    result: Vec<Transaction>,
}

#[derive(Clone, Deserialize)]
struct Transaction {
    transaction_id: TransactionId,
    #[serde(default)]
    in_msg: Option<Message>,
}

#[derive(Clone, Deserialize)]
struct TransactionId {
    hash: String,
}

#[derive(Clone, Deserialize)]
struct Message {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

type Chain = HashMap<String, (Instant, Vec<Transaction>)>;

pub struct Gram {
    http: reqwest::Client,
    toncenter: Option<String>,
    tonapi: String,
    coingecko: String,
    rates: Mutex<Option<(Instant, Rates)>>,
    chain_ttl: Duration,
    chain: Mutex<Chain>,
    rejected_key: Mutex<Option<(String, Instant, String)>>,
    issue: Mutex<Option<ProviderIssue>>,
    logged: Mutex<HashMap<String, Instant>>,
}

fn guard<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn toncenter_error(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("error")
        .or_else(|| value.get("result"))
        .and_then(serde_json::Value::as_str)
        .map(|text| text.chars().take(200).collect())
}

fn failure_text(err: &PaymentError, detail: &str) -> String {
    match err {
        PaymentError::Http { status: 429 } => "toncenter ограничил частоту запросов (ответ 429). Без ключа он отвечает не чаще раза в секунду — добавьте ключ API".to_string(),
        PaymentError::Http { status } if *status >= 500 => format!(
            "toncenter не отдал историю кошелька (ответ {status}: {detail}). Проверка повторится сама"
        ),
        PaymentError::Transport(_) => format!("toncenter недоступен: {detail}"),
        _ if detail.contains("address") => format!(
            "toncenter не понял адрес кошелька ({detail}). Проверьте адрес в настройках GRAM: его лучше скопировать из кошелька целиком"
        ),
        _ => format!("toncenter ответил ошибкой: {detail}"),
    }
}

impl Default for Gram {
    fn default() -> Self {
        Self::new()
    }
}

impl Gram {
    pub fn new() -> Self {
        Gram {
            http: http_client(),
            toncenter: None,
            tonapi: TONAPI.into(),
            coingecko: COINGECKO.into(),
            rates: Mutex::new(None),
            chain_ttl: CHAIN_TTL,
            chain: Mutex::new(HashMap::new()),
            rejected_key: Mutex::new(None),
            issue: Mutex::new(None),
            logged: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_endpoints(
        toncenter: impl Into<String>,
        tonapi: impl Into<String>,
        coingecko: impl Into<String>,
    ) -> Self {
        Gram {
            http: http_client(),
            toncenter: Some(toncenter.into()),
            tonapi: tonapi.into(),
            coingecko: coingecko.into(),
            rates: Mutex::new(None),
            chain_ttl: Duration::ZERO,
            chain: Mutex::new(HashMap::new()),
            rejected_key: Mutex::new(None),
            issue: Mutex::new(None),
            logged: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_chain_cache(mut self, ttl: Duration) -> Self {
        self.chain_ttl = ttl;
        self
    }

    fn note(&self, message: String) {
        *guard(&self.issue) = Some(ProviderIssue {
            at: Utc::now(),
            message,
        });
    }

    fn key_resting(&self, key: &str) -> bool {
        guard(&self.rejected_key)
            .as_ref()
            .is_some_and(|(rejected, at, _)| rejected == key && at.elapsed() < REJECTED_KEY_PAUSE)
    }

    fn key_message(&self) -> Option<String> {
        guard(&self.rejected_key)
            .as_ref()
            .map(|(_, _, message)| message.clone())
    }

    fn toncenter(&self, config: &ProviderConfig) -> String {
        match &self.toncenter {
            Some(base) => base.trim_end_matches('/').to_string(),
            None if config.flag("testnet") => TONCENTER_TESTNET.to_string(),
            None => TONCENTER.to_string(),
        }
    }

    fn cached(&self) -> Option<Rates> {
        let guard = match self.rates.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .filter(|(at, _)| at.elapsed() < RATES_TTL)
            .map(|(_, rates)| rates)
    }

    fn remember(&self, rates: Rates) {
        let mut guard = match self.rates.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = Some((Instant::now(), rates));
    }

    async fn rates_tonapi(&self) -> PaymentResult<Rates> {
        let response = self
            .http
            .get(format!(
                "{}/v2/rates?tokens=ton&currencies=usd,rub",
                self.tonapi.trim_end_matches('/')
            ))
            .send()
            .await
            .map_err(PaymentError::transport)?;
        if !response.status().is_success() {
            return Err(PaymentError::Http {
                status: response.status().as_u16(),
            });
        }
        let parsed: TonapiRates = response
            .json()
            .await
            .map_err(|e| PaymentError::BadPayload(e.without_url().to_string()))?;
        let token = parsed
            .rates
            .into_iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("ton"))
            .map(|(_, token)| token)
            .ok_or_else(|| PaymentError::BadPayload("в ответе нет курса TON".into()))?;
        let price = |code: &str| {
            token
                .prices
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(code))
                .and_then(|(_, value)| price_from_float(*value))
        };
        match (price("usd"), price("rub")) {
            (Some(usd), Some(rub)) => Ok(Rates { usd, rub }),
            _ => Err(PaymentError::BadPayload("курс TON без USD или RUB".into())),
        }
    }

    async fn rates_coingecko(&self) -> PaymentResult<Rates> {
        let response = self
            .http
            .get(format!(
                "{}/api/v3/simple/price?ids=the-open-network&vs_currencies=usd,rub",
                self.coingecko.trim_end_matches('/')
            ))
            .send()
            .await
            .map_err(PaymentError::transport)?;
        if !response.status().is_success() {
            return Err(PaymentError::Http {
                status: response.status().as_u16(),
            });
        }
        let parsed: HashMap<String, HashMap<String, f64>> = response
            .json()
            .await
            .map_err(|e| PaymentError::BadPayload(e.without_url().to_string()))?;
        let prices = parsed
            .get("the-open-network")
            .ok_or_else(|| PaymentError::BadPayload("в ответе нет курса TON".into()))?;
        match (
            prices.get("usd").copied().and_then(price_from_float),
            prices.get("rub").copied().and_then(price_from_float),
        ) {
            (Some(usd), Some(rub)) => Ok(Rates { usd, rub }),
            _ => Err(PaymentError::BadPayload("курс TON без USD или RUB".into())),
        }
    }

    async fn market_price(&self, currency: Currency) -> PaymentResult<u128> {
        if let Some(rates) = self.cached() {
            return Ok(rates.get(currency));
        }
        let rates = match self.rates_tonapi().await {
            Ok(rates) => rates,
            Err(first) => {
                tracing::warn!(error = %first, "курс GRAM от tonapi не получен, пробуем CoinGecko");
                self.rates_coingecko().await.map_err(|_| {
                    PaymentError::Transport("курс GRAM сейчас недоступен, попробуйте позже".into())
                })?
            }
        };
        self.remember(rates);
        Ok(rates.get(currency))
    }

    async fn price(&self, config: &ProviderConfig, currency: Currency) -> PaymentResult<u128> {
        match manual_rate(config) {
            Some(Ok(price)) => Ok(price),
            Some(Err(())) => Err(PaymentError::NotConfigured(
                "курс вручную записан неверно: нужно число больше нуля".into(),
            )),
            None => self.market_price(currency).await,
        }
    }

    fn log_due(&self, invoice: &str) -> bool {
        let mut logged = guard(&self.logged);
        logged.retain(|_, at| at.elapsed() < LOG_EVERY);
        if logged.contains_key(invoice) {
            return false;
        }
        logged.insert(invoice.to_string(), Instant::now());
        true
    }

    async fn request_chain(
        &self,
        base: &str,
        address: &str,
        key: Option<&str>,
    ) -> Result<Vec<Transaction>, (PaymentError, String)> {
        match self
            .request_page(base, address, key, TRANSACTIONS_LIMIT)
            .await
        {
            Err((PaymentError::Http { status: 500..=599 } | PaymentError::BadPayload(_), _)) => {
                self.request_page(base, address, key, TRANSACTIONS_RETRY_LIMIT)
                    .await
            }
            other => other,
        }
    }

    async fn request_page(
        &self,
        base: &str,
        address: &str,
        key: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Transaction>, (PaymentError, String)> {
        let mut request = self.http.get(format!("{base}/getTransactions")).query(&[
            ("address", address.to_string()),
            ("limit", limit.to_string()),
            ("archival", "true".to_string()),
        ]);
        if let Some(key) = key {
            request = request.header("X-API-Key", key);
        }
        let response = request.send().await.map_err(|e| {
            let err = PaymentError::transport(e);
            let text = err.to_string();
            (err, text)
        })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail =
                toncenter_error(&body).unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
            return Err((
                PaymentError::Http {
                    status: status.as_u16(),
                },
                detail,
            ));
        }
        let parsed: ToncenterResponse = response.json().await.map_err(|e| {
            let text = format!("ответ не разобран: {}", e.without_url());
            (PaymentError::BadPayload(text.clone()), text)
        })?;
        if !parsed.ok {
            let text = "toncenter ответил ok=false".to_string();
            return Err((PaymentError::BadPayload(text.clone()), text));
        }
        Ok(parsed.result)
    }

    async fn transactions(
        &self,
        config: &ProviderConfig,
        address: &str,
    ) -> PaymentResult<Vec<Transaction>> {
        let base = self.toncenter(config);
        let slot = format!("{base}|{address}");
        if !self.chain_ttl.is_zero()
            && let Some((at, list)) = guard(&self.chain).get(&slot)
            && at.elapsed() < self.chain_ttl
        {
            return Ok(list.clone());
        }
        let key = config
            .get("toncenter_key")
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string);
        let resting = key.as_deref().is_some_and(|k| self.key_resting(k));
        let result = match key.as_deref().filter(|_| !resting) {
            Some(key) => match self.request_chain(&base, address, Some(key)).await {
                Err((
                    PaymentError::Http {
                        status: status @ (401 | 403),
                    },
                    detail,
                )) => {
                    let message = format!(
                        "toncenter не принял ключ API (ответ {status}: {detail}). Проверяем оплаты без ключа. Новый ключ начинает работать через 5–10 минут; если ошибка не уходит, откройте бота @toncenter → Manage API Keys и проверьте ключ: сеть Mainnet, статус Active"
                    );
                    *guard(&self.rejected_key) =
                        Some((key.to_string(), Instant::now(), message.clone()));
                    tracing::warn!("{message}");
                    match self.request_chain(&base, address, None).await {
                        Ok(list) => {
                            self.note(message);
                            Ok(list)
                        }
                        Err((err, detail)) => {
                            self.note(format!("{message}. Без ключа тоже ошибка: {detail}"));
                            Err(err)
                        }
                    }
                }
                Ok(list) => {
                    *guard(&self.rejected_key) = None;
                    *guard(&self.issue) = None;
                    Ok(list)
                }
                Err((err, detail)) => {
                    self.note(failure_text(&err, &detail));
                    Err(err)
                }
            },
            None => match self.request_chain(&base, address, None).await {
                Ok(list) => {
                    match self.key_message().filter(|_| resting) {
                        Some(message) => self.note(message),
                        None => *guard(&self.issue) = None,
                    }
                    Ok(list)
                }
                Err((err, detail)) => {
                    let failure = failure_text(&err, &detail);
                    match self.key_message().filter(|_| resting) {
                        Some(message) => self.note(format!(
                            "{message}. Последняя проверка без ключа тоже не прошла: {failure}"
                        )),
                        None => self.note(failure),
                    }
                    Err(err)
                }
            },
        };
        if let Ok(list) = &result
            && !self.chain_ttl.is_zero()
        {
            let mut chain = guard(&self.chain);
            chain.retain(|_, (at, _)| at.elapsed() < self.chain_ttl);
            chain.insert(slot, (Instant::now(), list.clone()));
        }
        result
    }
}

fn manual_rate(config: &ProviderConfig) -> Option<Result<u128, ()>> {
    let raw = match config.0.get("rate") {
        None | Some(serde_json::Value::Null) => return None,
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::String(text)) if text.trim().is_empty() => return None,
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(_) => return Some(Err(())),
    };
    Some(parse_price(&raw).ok_or(()))
}

fn pending(invoice: &GramInvoice) -> WebhookOutcome {
    WebhookOutcome {
        event_key: format!("gram:wait:{}", invoice.comment),
        payment: None,
        provider_txid: None,
        status: ProviderStatus::Pending,
        amount: None,
        saved_method: None,
        reply: WebhookReply::ok(),
    }
}

#[async_trait]
impl PaymentProvider for Gram {
    fn id(&self) -> &'static str {
        providers::GRAM
    }

    fn title(&self) -> &'static str {
        "GRAM"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Usd, Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "GRAM",
            hint: "криптовалюта TON",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                "wallet",
                "Адрес кошелька TON",
                "Куда приходят платежи. Лучше некастодиальный кошелёк: Tonkeeper, TON Space в Telegram Wallet",
            ),
            SettingField::secret(
                "toncenter_key",
                "Ключ API toncenter",
                "Необязательно: без ключа toncenter отвечает раз в секунду, для проверки оплат этого хватает. Ключ выдаёт бот @toncenter («toncenter.com bot» → Manage API Keys) или @tonapibot, нужен ключ для сети Mainnet. Новый ключ начинает работать через 5–10 минут; если toncenter его не примет, оплаты проверяются без ключа, а ошибка видна здесь",
            )
            .optional(),
            SettingField::number(
                "rate",
                "Курс вручную",
                "Цена 1 GRAM в валюте сервиса. Пусто — курс берётся с tonapi.io и CoinGecko и обновляется раз в 5 минут",
            ),
            SettingField::flag(
                "testnet",
                "Тестовая сеть",
                "Только для проверки: платежи тестовыми GRAM в сети testnet",
            ),
        ]
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        let wallet = config.require("wallet")?;
        if !valid_address(wallet) {
            return Err(PaymentError::NotConfigured(
                "адрес кошелька TON записан неверно: нужен адрес из 48 символов, например UQ…"
                    .into(),
            ));
        }
        if !checksum_ok(wallet) {
            return Err(PaymentError::NotConfigured(
                "в адресе кошелька TON опечатка: не сходится контрольная сумма. Скопируйте адрес из кошелька ещё раз"
                    .into(),
            ));
        }
        if let Some(Err(())) = manual_rate(config) {
            return Err(PaymentError::NotConfigured(
                "курс вручную записан неверно: нужно число больше нуля".into(),
            ));
        }
        Ok(())
    }

    fn polls_status(&self) -> bool {
        true
    }

    fn issue(&self) -> Option<ProviderIssue> {
        guard(&self.issue).clone()
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        let address = config.require("wallet")?.to_string();
        if !valid_address(&address) {
            return Err(PaymentError::NotConfigured(
                "адрес кошелька TON записан неверно".into(),
            ));
        }
        let currency = request.amount.currency.as_currency().ok_or_else(|| {
            PaymentError::UnsupportedCurrency(request.amount.currency.code().into())
        })?;
        let price = self.price(config, currency).await?;
        let nano = to_nano(request.amount.minor, price).ok_or_else(|| {
            PaymentError::Rejected("сумма к оплате должна быть больше нуля".into())
        })?;
        let invoice = GramInvoice {
            comment: comment_for(request.payment_public_id),
            nano,
            address,
            amount: request.amount,
        };
        let pay_url = page_url(&request.webhook_url, request.payment_public_id)
            .unwrap_or_else(|| invoice.tonkeeper_link());
        Ok(Invoice {
            pay_url,
            external_id: Some(invoice.external_id()),
            expires_at: Some(Utc::now() + request.expires_in),
            payload: serde_json::json!({
                "address": invoice.address,
                "nano": invoice.nano.to_string(),
                "comment": invoice.comment,
                "rate_micro": price.to_string(),
                "currency": currency.code(),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        _config: &ProviderConfig,
        _request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        Err(PaymentError::Unsupported(
            "уведомления: оплата GRAM проверяется в блокчейне",
        ))
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let Some(invoice) = GramInvoice::parse(external_id) else {
            return Ok(None);
        };
        let transactions = match self.transactions(config, &invoice.address).await {
            Ok(list) => list,
            Err(err) => {
                if self.log_due(&invoice.comment) {
                    let reason = self
                        .issue()
                        .map(|issue| issue.message)
                        .unwrap_or_else(|| err.to_string());
                    tracing::warn!(invoice = %invoice.comment, "проверка оплаты GRAM не прошла: {reason}");
                }
                if err.is_retryable() || matches!(err, PaymentError::Http { status: 429 }) {
                    return Ok(Some(pending(&invoice)));
                }
                return Err(err);
            }
        };
        let paid = transactions.into_iter().find(|tx| {
            tx.in_msg.as_ref().is_some_and(|msg| {
                msg.source.as_deref().is_some_and(|s| !s.trim().is_empty())
                    && msg
                        .message
                        .as_deref()
                        .is_some_and(|m| m.trim().eq_ignore_ascii_case(&invoice.comment))
                    && msg
                        .value
                        .as_deref()
                        .and_then(|v| v.trim().parse::<u128>().ok())
                        .is_some_and(|value| value >= invoice.nano)
            })
        });
        Ok(Some(match paid {
            Some(tx) => WebhookOutcome {
                event_key: format!("gram:{}", tx.transaction_id.hash),
                payment: None,
                provider_txid: None,
                status: ProviderStatus::Paid,
                amount: Some(invoice.amount),
                saved_method: None,
                reply: WebhookReply::ok(),
            },
            None => pending(&invoice),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{PAYMENT, config, invoice, rub, usd};
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const WALLET: &str = "UQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqEBI";

    fn settings(extra: &[(&str, serde_json::Value)]) -> ProviderConfig {
        let mut pairs = vec![("wallet", json!(WALLET))];
        pairs.extend(extra.iter().cloned());
        config(&pairs)
    }

    async fn rates(server: &MockServer, usd: f64, rub: f64) {
        Mock::given(method("GET"))
            .and(path("/v2/rates"))
            .and(query_param("tokens", "ton"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "rates": {"TON": {"prices": {"USD": usd, "RUB": rub}, "diff_24h": {"USD": "+1%"}}}
            })))
            .mount(server)
            .await;
    }

    fn tx(hash: &str, source: &str, value: &str, comment: &str) -> serde_json::Value {
        json!({
            "@type": "raw.transaction",
            "utime": 1_763_300_000,
            "transaction_id": {"@type": "internal.transactionId", "lt": "51000000000001", "hash": hash},
            "in_msg": {"@type": "raw.message", "source": source, "destination": WALLET, "value": value, "message": comment},
            "out_msgs": []
        })
    }

    #[test]
    fn перевод_для_кошелька_telegram_с_комментарием_в_payload() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc16(b"123456789"), 0x31C3);
        assert_eq!(cell_boc(&[]).as_deref(), Some("te6cckEBAQEAAgAAAEysuc0="));
        assert_eq!(
            comment_payload("SN-ABCDEFGH").as_deref(),
            Some("te6cckEBAQEAEQAAHgAAAABTTi1BQkNERUZHSIkhE3A=")
        );
        assert_eq!(comment_payload(&"x".repeat(124)), None);
        assert_eq!(
            friendly_address("0:83dfd552e63729b472fcbcc8c45ebcc6691702558b68ec7527e1ba403a0f31a8")
                .as_deref(),
            Some("UQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqEBI")
        );
        assert_eq!(friendly_address(WALLET).as_deref(), Some(WALLET));
        assert_eq!(friendly_address("0:zz"), None);
        assert!(checksum_ok(WALLET));
        assert!(checksum_ok(
            "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N"
        ));
        assert!(checksum_ok(
            "0:83dfd552e63729b472fcbcc8c45ebcc6691702558b68ec7527e1ba403a0f31a8"
        ));
        let typo = "UQBvW8Z5huBkMJYdnfAEM5JqTNkuWX3diqYENkWsIL0XggGG";
        assert!(valid_address(typo));
        assert!(!checksum_ok(typo));
        let refused = Gram::new()
            .validate_settings(&settings(&[("wallet", json!(typo))]))
            .unwrap_err();
        assert!(
            refused.to_string().contains("контрольная сумма"),
            "{refused}"
        );
        let invoice = GramInvoice {
            comment: "SN-ABCDEFGH".into(),
            nano: 1_340_000_000,
            address: WALLET.into(),
            amount: Amount::new(699, PayCurrency::Usd),
        };
        assert_eq!(
            invoice.wallet_transaction(1_800_000_000),
            Some(json!({
                "valid_until": 1_800_000_000,
                "messages": [{
                    "address": WALLET,
                    "amount": "1340000000",
                    "payload": "te6cckEBAQEAEQAAHgAAAABTTi1BQkNERUZHSIkhE3A="
                }]
            }))
        );
    }

    #[test]
    fn граммы_адреса_и_комментарий() {
        assert_eq!(format_grams(12_470_000_000), "12.47");
        assert_eq!(format_grams(5_000_000_000), "5");
        assert_eq!(format_grams(10_000_000), "0.01");
        assert!(valid_address(WALLET));
        assert!(valid_address(
            "0:83dfd552e63729b472fcbcc8c45ebcc6691702558b68ec7527e1ba403a0f31a8"
        ));
        assert!(!valid_address("UQ123"));
        assert!(!valid_address("0:zz"));
        let comment = comment_for(PAYMENT);
        assert_eq!(comment.len(), 11);
        assert!(comment.starts_with("SN-"));
        assert_eq!(comment, comment_for(PAYMENT));
        assert_ne!(comment, comment_for(Uuid::from_u128(1 << 100)));
        assert_eq!(parse_price("5.25"), Some(5_250_000));
        assert_eq!(parse_price("410,5"), Some(410_500_000));
        assert_eq!(parse_price("0"), None);
        assert_eq!(parse_price("abc"), None);
        assert_eq!(parse_price("1.1234567"), None);
        assert_eq!(to_nano(699, 5_250_000), Some(1_340_000_000));
        assert_eq!(to_nano(500, 5_000_000), Some(1_000_000_000));
        assert_eq!(to_nano(0, 5_000_000), None);
        let invoice = GramInvoice {
            comment: "SN-ABCDEFGH".into(),
            nano: 1_340_000_000,
            address: WALLET.into(),
            amount: usd(699),
        };
        assert_eq!(
            GramInvoice::parse(&invoice.external_id()),
            Some(invoice.clone())
        );
        assert_eq!(GramInvoice::parse("SN-A|1|bad|1|USD"), None);
        assert_eq!(
            invoice.ton_link(),
            format!("ton://transfer/{WALLET}?amount=1340000000&text=SN-ABCDEFGH")
        );
        assert!(
            invoice
                .tonkeeper_link()
                .starts_with("https://app.tonkeeper.com/transfer/")
        );
    }

    #[tokio::test]
    async fn счёт_по_курсу_с_округлением_и_своей_страницей() {
        let server = MockServer::start().await;
        rates(&server, 5.25, 420.0).await;
        let provider = Gram::with_endpoints(server.uri(), server.uri(), server.uri());
        let created = provider
            .create_invoice(&settings(&[]), &invoice(usd(699), None))
            .await
            .unwrap();
        assert_eq!(
            created.pay_url,
            format!("https://panel.example.com/pay/{PAYMENT}")
        );
        let parsed = GramInvoice::parse(created.external_id.as_deref().unwrap()).unwrap();
        assert_eq!(parsed.nano, 1_340_000_000);
        assert_eq!(parsed.amount, usd(699));
        assert_eq!(parsed.address, WALLET);
        assert_eq!(parsed.comment, comment_for(PAYMENT));
        assert_eq!(created.payload["rate_micro"], "5250000");
        let rubles = provider
            .create_invoice(&settings(&[]), &invoice(rub(49_900), None))
            .await
            .unwrap();
        let parsed = GramInvoice::parse(rubles.external_id.as_deref().unwrap()).unwrap();
        assert_eq!(parsed.nano, 1_190_000_000);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let mut offline = invoice(usd(699), None);
        offline.webhook_url = String::new();
        let created = provider
            .create_invoice(&settings(&[]), &offline)
            .await
            .unwrap();
        assert!(
            created
                .pay_url
                .starts_with("https://app.tonkeeper.com/transfer/")
        );
    }

    #[tokio::test]
    async fn ручной_курс_и_запасной_источник() {
        let server = MockServer::start().await;
        let provider = Gram::with_endpoints(server.uri(), server.uri(), server.uri());
        let manual = provider
            .create_invoice(&settings(&[("rate", json!("5"))]), &invoice(usd(500), None))
            .await
            .unwrap();
        assert_eq!(
            GramInvoice::parse(manual.external_id.as_deref().unwrap())
                .unwrap()
                .nano,
            1_000_000_000
        );
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(matches!(
            provider
                .create_invoice(
                    &settings(&[("rate", json!("-1"))]),
                    &invoice(usd(500), None)
                )
                .await,
            Err(PaymentError::NotConfigured(_))
        ));
        Mock::given(method("GET"))
            .and(path("/v2/rates"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/simple/price"))
            .and(query_param("ids", "the-open-network"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"the-open-network": {"usd": 4.0, "rub": 320.0}})),
            )
            .mount(&server)
            .await;
        let fallback = provider
            .create_invoice(&settings(&[]), &invoice(usd(500), None))
            .await
            .unwrap();
        assert_eq!(
            GramInvoice::parse(fallback.external_id.as_deref().unwrap())
                .unwrap()
                .nano,
            1_250_000_000
        );
        assert!(
            Gram::new()
                .validate_settings(&config(&[("wallet", json!("кошелёк"))]))
                .is_err()
        );
        assert!(Gram::new().validate_settings(&settings(&[])).is_ok());
        assert!(
            provider
                .create_invoice(
                    &settings(&[("rate", json!(5))]),
                    &invoice(sn_domain::money::Amount::new(10, PayCurrency::Xtr), None)
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn оплата_находится_по_комментарию_а_недоплата_ждёт() {
        let server = MockServer::start().await;
        let provider = Gram::with_endpoints(server.uri(), server.uri(), server.uri());
        let invoice = GramInvoice {
            comment: comment_for(PAYMENT),
            nano: 1_340_000_000,
            address: WALLET.into(),
            amount: usd(699),
        };
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .and(query_param("address", WALLET))
            .and(header("X-API-Key", "secret-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": [
                    tx("aGFzaDE=", "EQSender", "1340000000", "другой платёж"),
                    tx("aGFzaDI=", "", "1340000000", &invoice.comment),
                    tx("aGFzaDM=", "EQSender", "1000000000", &invoice.comment),
                ]
            })))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        let keyed = settings(&[("toncenter_key", json!("secret-key"))]);
        let waiting = provider
            .fetch_status(&keyed, &invoice.external_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.status, ProviderStatus::Pending);
        assert_eq!(waiting.amount, None);
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": [
                    tx("aGFzaDQ=", "EQSender", "1340000000", &invoice.comment.to_lowercase()),
                ]
            })))
            .with_priority(2)
            .mount(&server)
            .await;
        let paid = provider
            .fetch_status(&keyed, &invoice.external_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(paid.status, ProviderStatus::Paid);
        assert_eq!(paid.amount, Some(usd(699)));
        assert_eq!(paid.event_key, "gram:aGFzaDQ=");
        assert_eq!(provider.fetch_status(&keyed, "чужой").await.unwrap(), None);
    }

    #[tokio::test]
    async fn лимит_toncenter_не_ломает_проверку_и_тестовая_сеть() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let provider = Gram::with_endpoints(server.uri(), server.uri(), server.uri());
        let invoice = GramInvoice {
            comment: comment_for(PAYMENT),
            nano: 1_000_000_000,
            address: WALLET.into(),
            amount: rub(10_000),
        };
        let outcome = provider
            .fetch_status(&settings(&[]), &invoice.external_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        let real = Gram::new();
        assert_eq!(real.toncenter(&settings(&[])), TONCENTER);
        assert_eq!(
            real.toncenter(&settings(&[("testnet", json!(true))])),
            TONCENTER_TESTNET
        );
        assert!(real.polls_status());
        assert!(!real.accepts_http_webhooks());
    }

    fn paid_list(comment: &str) -> serde_json::Value {
        json!({
            "ok": true,
            "result": [{
                "transaction_id": {"lt": "9", "hash": "a2V5bGVzcw=="},
                "in_msg": {"source": "EQDGrR9gCY0r", "value": "1160000000", "message": comment}
            }]
        })
    }

    #[tokio::test]
    async fn непринятый_ключ_toncenter_не_мешает_найти_оплату() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .and(header("X-API-Key", "fresh-key"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "ok": false, "error": "API key does not exist", "code": 401
            })))
            .with_priority(1)
            .mount(&server)
            .await;
        let invoice = GramInvoice {
            comment: "SN-WLPDQ6GT".into(),
            nano: 1_160_000_000,
            address: WALLET.into(),
            amount: rub(10_000),
        };
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(paid_list(&invoice.comment)))
            .with_priority(2)
            .mount(&server)
            .await;
        let provider = Gram::with_endpoints(server.uri(), server.uri(), server.uri());
        let keyed = settings(&[("toncenter_key", json!("fresh-key"))]);
        assert_eq!(provider.issue(), None);
        let outcome = provider
            .fetch_status(&keyed, &invoice.external_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(rub(10_000)));
        let issue = provider.issue().unwrap();
        assert!(
            issue.message.contains("не принял ключ API"),
            "{}",
            issue.message
        );
        assert!(
            issue.message.contains("API key does not exist"),
            "{}",
            issue.message
        );
        assert!(!issue.message.contains("fresh-key"));
        let keyed_requests = |requests: &[wiremock::Request]| {
            requests
                .iter()
                .filter(|r| r.headers.get("X-API-Key").is_some())
                .count()
        };
        assert_eq!(
            keyed_requests(&server.received_requests().await.unwrap()),
            1
        );
        provider
            .fetch_status(&keyed, &invoice.external_id())
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(keyed_requests(&requests), 1);
        assert_eq!(requests.len(), 3);
        assert!(provider.issue().is_some());

        let other = Gram::with_endpoints(server.uri(), server.uri(), server.uri());
        let fixed = settings(&[("toncenter_key", json!("working-key"))]);
        other
            .fetch_status(&fixed, &invoice.external_id())
            .await
            .unwrap();
        assert_eq!(other.issue(), None);
    }

    #[tokio::test]
    async fn ответ_toncenter_живёт_в_кеше_несколько_секунд() {
        let server = MockServer::start().await;
        let invoice = GramInvoice {
            comment: "SN-WLPDQ6GT".into(),
            nano: 1_160_000_000,
            address: WALLET.into(),
            amount: rub(10_000),
        };
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(paid_list(&invoice.comment)))
            .mount(&server)
            .await;
        let provider = Gram::with_endpoints(server.uri(), server.uri(), server.uri())
            .with_chain_cache(Duration::from_secs(60));
        for _ in 0..3 {
            let outcome = provider
                .fetch_status(&settings(&[]), &invoice.external_id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(outcome.status, ProviderStatus::Paid);
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .respond_with(ResponseTemplate::new(503))
            .with_priority(1)
            .mount(&server)
            .await;
        let fresh = Gram::with_endpoints(server.uri(), server.uri(), server.uri());
        let outcome = fresh
            .fetch_status(&settings(&[]), &invoice.external_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        assert!(
            fresh
                .issue()
                .unwrap()
                .message
                .contains("не отдал историю кошелька")
        );
    }

    #[tokio::test]
    async fn история_кошелька_со_старыми_переводами_читается_целиком() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .and(query_param("archival", "false"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "ok": false,
                "error": "LITE_SERVER_UNKNOWN: cannot compute block with specified transaction: lt not in db",
                "code": 500
            })))
            .with_priority(1)
            .mount(&server)
            .await;
        let invoice = GramInvoice {
            comment: "SN-WLPDQ6GT".into(),
            nano: 1_160_000_000,
            address: WALLET.into(),
            amount: rub(10_000),
        };
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(paid_list(&invoice.comment)))
            .with_priority(2)
            .mount(&server)
            .await;
        let provider = Gram::with_endpoints(server.uri(), server.uri(), server.uri());
        let outcome = provider
            .fetch_status(&settings(&[]), &invoice.external_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(provider.issue(), None);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .url
                .query_pairs()
                .any(|(name, value)| name == "archival" && value == "true")
        );
    }

    #[tokio::test]
    async fn если_длинная_страница_не_отдалась_берём_короткую() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .and(query_param("limit", "100"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "ok": false,
                "error": "LITE_SERVER_UNKNOWN: cannot locate transaction in block with specified logical time",
                "code": 500
            })))
            .with_priority(1)
            .mount(&server)
            .await;
        let invoice = GramInvoice {
            comment: "SN-WLPDQ6GT".into(),
            nano: 1_160_000_000,
            address: WALLET.into(),
            amount: rub(10_000),
        };
        Mock::given(method("GET"))
            .and(path("/getTransactions"))
            .and(query_param("limit", "30"))
            .respond_with(ResponseTemplate::new(200).set_body_json(paid_list(&invoice.comment)))
            .with_priority(2)
            .mount(&server)
            .await;
        let provider = Gram::with_endpoints(server.uri(), server.uri(), server.uri());
        let outcome = provider
            .fetch_status(&settings(&[]), &invoice.external_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        assert_eq!(provider.issue(), None);
    }

    #[test]
    fn предупреждение_в_журнал_не_чаще_раза_в_минуту_на_счёт() {
        let provider = Gram::new();
        assert!(provider.log_due("SN-AAAAAAAA"));
        assert!(!provider.log_due("SN-AAAAAAAA"));
        assert!(provider.log_due("SN-BBBBBBBB"));
    }
}
