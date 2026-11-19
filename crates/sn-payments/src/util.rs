use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::{
    InvoiceRequest, PaymentRef, ProviderConfig, ProviderStatus, WebhookOutcome, WebhookReply,
};
use bytes::Bytes;
use hmac::digest::CtOutput;
use sha2::{Digest, Sha256};
use sn_domain::money::{Amount, PayCurrency, parse_units};
use sn_domain::settings::{SettingField, missing_required};
use std::collections::HashSet;
use uuid::Uuid;

pub(crate) fn skipped(
    event_key: String,
    payment: Option<PaymentRef>,
    provider_txid: Option<String>,
    reply: WebhookReply,
) -> WebhookOutcome {
    WebhookOutcome {
        event_key,
        payment,
        provider_txid,
        status: ProviderStatus::Pending,
        amount: None,
        saved_method: None,
        reply,
        signal: None,
    }
}

pub(crate) fn public_ref(value: Option<&str>) -> Option<PaymentRef> {
    value
        .and_then(|v| Uuid::parse_str(v.trim()).ok())
        .map(PaymentRef::PublicId)
}

pub(crate) fn client_email(request: &InvoiceRequest) -> Option<&str> {
    request
        .client
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| e.len() > 2 && e.contains('@') && !e.contains(char::is_whitespace))
}

pub(crate) fn require_currency(amount: Amount, currency: PayCurrency) -> PaymentResult<()> {
    if amount.currency == currency {
        Ok(())
    } else {
        Err(PaymentError::UnsupportedCurrency(
            amount.currency.code().into(),
        ))
    }
}

pub(crate) fn secret_eq(expected: &str, received: &str) -> bool {
    let digest = |text: &str| CtOutput::<Sha256>::new(Sha256::digest(text.as_bytes()));
    digest(expected) == digest(received)
}

pub(crate) fn hex_digest_eq(expected: &[u8], received: &str) -> bool {
    secret_eq(
        &hex::encode(expected),
        &received.trim().to_ascii_lowercase(),
    )
}

pub(crate) fn require_fields(
    schema: &[SettingField],
    config: &ProviderConfig,
) -> PaymentResult<()> {
    let missing = missing_required(schema, &config.0);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(PaymentError::NotConfigured(format!(
            "не заполнены поля: {}",
            missing.join(", ")
        )))
    }
}

pub(crate) fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

pub(crate) fn bad(reason: impl Into<String>) -> PaymentError {
    PaymentError::BadPayload(reason.into())
}

pub(crate) fn minor(raw: &str, currency: PayCurrency) -> PaymentResult<i64> {
    parse_units(raw, currency).ok_or_else(|| bad(format!("сумма «{}» не разобрана", clip(raw, 32))))
}

pub(crate) fn amount_text(amount: Amount) -> String {
    match amount.as_money() {
        Some(money) => sn_domain::format::money(money),
        None => amount.to_string(),
    }
}

pub(crate) fn pay_currency(code: &str) -> PaymentResult<PayCurrency> {
    PayCurrency::parse(code)
        .map_err(|_| bad(format!("валюта «{}» не поддерживается", clip(code, 8))))
}

pub(crate) fn json_amount(amount: Option<&Json>, currency: Option<&Json>) -> PaymentResult<Amount> {
    let currency = currency
        .and_then(Json::as_str)
        .ok_or_else(|| bad("нет валюты"))
        .and_then(pay_currency)?;
    let raw = amount
        .and_then(Json::scalar_text)
        .ok_or_else(|| bad("нет суммы"))?;
    Ok(Amount::new(minor(raw, currency)?, currency))
}

pub(crate) fn is_https(url: &str) -> bool {
    reqwest::Url::parse(url)
        .map(|u| u.scheme() == "https" && u.host_str().is_some())
        .unwrap_or(false)
}

pub(crate) fn origin(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let origin = parsed.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

pub(crate) fn return_url(request: &InvoiceRequest) -> Option<String> {
    request
        .return_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| origin(&request.webhook_url))
}

pub(crate) fn safe_id(id: &str) -> PaymentResult<&str> {
    if !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(id)
    } else {
        Err(bad("идентификатор платежа в недопустимом формате"))
    }
}

fn percent_decode(raw: &str) -> PaymentResult<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut pos = 0;
    while let Some(&byte) = bytes.get(pos) {
        match byte {
            b'+' => {
                out.push(b' ');
                pos += 1;
            }
            b'%' => {
                let hex = bytes
                    .get(pos + 1..pos + 3)
                    .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                    .ok_or_else(|| bad("испорченное URL-кодирование"))?;
                let text =
                    std::str::from_utf8(hex).map_err(|_| bad("испорченное URL-кодирование"))?;
                out.push(
                    u8::from_str_radix(text, 16).map_err(|_| bad("испорченное URL-кодирование"))?,
                );
                pos += 3;
            }
            other => {
                out.push(other);
                pos += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| bad("поле формы не в UTF-8"))
}

pub(crate) fn form_pairs(body: &[u8]) -> PaymentResult<Vec<(String, String)>> {
    let text = std::str::from_utf8(body).map_err(|_| bad("тело формы не в UTF-8"))?;
    let mut pairs = Vec::new();
    let mut seen = HashSet::new();
    for part in text.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        let key = percent_decode(key)?;
        if !seen.insert(key.clone()) {
            return Err(bad(format!("поле «{}» повторяется", clip(&key, 32))));
        }
        pairs.push((key, percent_decode(value)?));
    }
    Ok(pairs)
}

pub(crate) fn field<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

pub(crate) fn rfc3986(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(crate) const KEYS_REFUSED: &str =
    "платёжная система не приняла ключи доступа, проверьте настройки";

pub(crate) fn keys_refused(err: &PaymentError) -> bool {
    match err {
        PaymentError::Rejected(reason) => reason == KEYS_REFUSED,
        PaymentError::Http { status } => matches!(status, 401 | 403),
        PaymentError::BadSignature => true,
        _ => false,
    }
}

pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Bytes,
}

pub(crate) async fn send(request: reqwest::RequestBuilder) -> PaymentResult<Reply> {
    let response = request.send().await.map_err(PaymentError::transport)?;
    let status = response.status().as_u16();
    let body = response.bytes().await.map_err(PaymentError::transport)?;
    Ok(Reply { status, body })
}

impl Reply {
    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub(crate) fn json(&self) -> PaymentResult<Json> {
        Json::parse(&self.body).ok_or_else(|| self.unreadable())
    }

    pub(crate) fn typed<T: serde::de::DeserializeOwned>(&self) -> PaymentResult<T> {
        serde_json::from_slice(&self.body).map_err(|_| self.unreadable())
    }

    fn unreadable(&self) -> PaymentError {
        if self.is_success() {
            PaymentError::Transport(format!("ответ не разобран (HTTP {})", self.status))
        } else {
            self.failure(None)
        }
    }

    pub(crate) fn failure(&self, message: Option<&str>) -> PaymentError {
        let message = message
            .map(|m| clip(m.trim(), 200))
            .filter(|m| !m.is_empty());
        match (self.status, message) {
            (403, Some(message)) => PaymentError::Rejected(message),
            (401 | 403, _) => PaymentError::Rejected(KEYS_REFUSED.into()),
            (429 | 500..=599, _) => PaymentError::Http {
                status: self.status,
            },
            (_, Some(message)) => PaymentError::Rejected(message),
            (200..=299, None) => PaymentError::Rejected("запрос отклонён без пояснения".into()),
            (status, None) => PaymentError::Http { status },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn сравнение_секретов() {
        assert!(secret_eq("abc", "abc"));
        assert!(!secret_eq("abc", "abd"));
        assert!(!secret_eq("abc", ""));
        assert!(!secret_eq("abc", "abcd"));
        assert!(!secret_eq("abc", "abc\0"));
        assert!(!secret_eq("abc\0\0", "abc"));
        assert!(secret_eq("", ""));
        let long = "k".repeat(200);
        assert!(secret_eq(&long, &long.clone()));
        assert!(!secret_eq(&long, &format!("{long}\0")));
        assert!(hex_digest_eq(&[0xab, 0x01], "AB01"));
        assert!(!hex_digest_eq(&[0xab, 0x01], "ab02"));
    }

    #[test]
    fn форма_разбирается_строго() {
        let pairs = form_pairs(b"a=1&b=%D0%9F%2B+x&c=&d").unwrap();
        assert_eq!(field(&pairs, "b"), Some("П+ x"));
        assert_eq!(field(&pairs, "c"), Some(""));
        assert_eq!(field(&pairs, "d"), Some(""));
        assert!(form_pairs(b"a=1&a=2").is_err());
        assert!(form_pairs(b"a=%zz").is_err());
        assert!(form_pairs(b"a=%+1").is_err());
        assert!(form_pairs(b"a=%D0").is_err());
    }

    #[test]
    fn кодирование_по_rfc_3986() {
        assert_eq!(rfc3986("2013-12-26T08:28:34Z"), "2013-12-26T08%3A28%3A34Z");
        assert_eq!(
            rfc3986(" !'()* Привет +& "),
            "%20%21%27%28%29%2A%20%D0%9F%D1%80%D0%B8%D0%B2%D0%B5%D1%82%20%2B%26%20"
        );
        assert_eq!(rfc3986("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn суммы_из_json_без_плавающей_точки() {
        let value = Json::parse(
            br#"{"a":19.99,"b":"19.9","c":1200,"d":19.999,"e":1e3,"f":-1,"cur":"RUB"}"#,
        )
        .unwrap();
        let amount = |key: &str| json_amount(value.get(key), value.get("cur"));
        assert_eq!(amount("a"), Ok(Amount::new(1999, PayCurrency::Rub)));
        assert_eq!(amount("b"), Ok(Amount::new(1990, PayCurrency::Rub)));
        assert_eq!(amount("c"), Ok(Amount::new(120_000, PayCurrency::Rub)));
        for key in ["d", "e", "f", "missing"] {
            assert!(amount(key).is_err(), "{key}");
        }
        assert!(json_amount(value.get("a"), None).is_err());
        let euro = Json::parse(br#"{"a":"1.00","cur":"EUR"}"#).unwrap();
        assert!(json_amount(euro.get("a"), euro.get("cur")).is_err());
    }

    #[test]
    fn при_403_виден_текст_платёжки_а_при_401_просьба_проверить_ключи() {
        let reply = |status: u16| Reply {
            status,
            body: Bytes::new(),
        };
        let keys = PaymentError::Rejected(
            "платёжная система не приняла ключи доступа, проверьте настройки".into(),
        );
        assert_eq!(reply(401).failure(Some("Unauthorized")), keys);
        assert_eq!(reply(401).failure(None), keys);
        assert_eq!(reply(403).failure(None), keys);
        assert_eq!(reply(403).failure(Some("  ")), keys);
        assert_eq!(
            reply(403).failure(Some(" Recurring payments are not allowed for this shop ")),
            PaymentError::Rejected("Recurring payments are not allowed for this shop".into())
        );
        assert_eq!(
            reply(400).failure(Some("Receipt is missing or illegal")),
            PaymentError::Rejected("Receipt is missing or illegal".into())
        );
        assert_eq!(
            reply(429).failure(Some("Too many requests")),
            PaymentError::Http { status: 429 }
        );
    }

    #[test]
    fn адрес_возврата_по_умолчанию_корень_панели() {
        assert_eq!(
            origin("https://panel.example.com/api/webhooks/pay/platega"),
            Some("https://panel.example.com".into())
        );
        assert_eq!(origin("not a url"), None);
        assert!(is_https("https://pay.example.com/x"));
        assert!(!is_https("http://pay.example.com/x"));
        assert!(safe_id("2a8f-00_1").is_ok());
        assert!(safe_id("../x").is_err());
        assert!(safe_id("").is_err());
    }
}
