use serde_json::Value;
use std::borrow::Cow;

const HIDDEN: &str = "•••";
const NESTED_DEPTH: u8 = 2;
const TEXT_LIMIT: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    Secret,
    Email,
    Phone,
    Person,
    Place,
    Ip,
    Card,
    CardStart,
    Expiry,
    Wallet,
    Text,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MaskedBody {
    Json(Value),
    Form(Vec<(String, String)>),
    Text(String),
    Binary(usize),
}

fn normalized(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

const SECRET_PARTS: [&str; 9] = [
    "signature",
    "hash",
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "privatekey",
    "authorization",
];
const SECRET_KEYS: [&str; 10] = [
    "sign",
    "cvv",
    "cvv2",
    "cvc",
    "cvc2",
    "pin",
    "otp",
    "authcode",
    "paymentmethodid",
    "codeword",
];
const METHOD_PARENTS: [&str; 4] = ["paymentmethod", "paymentmethoddata", "savedmethod", "card"];
const PERSON_KEYS: [&str; 18] = [
    "name",
    "firstname",
    "lastname",
    "middlename",
    "fathersname",
    "patronymic",
    "surname",
    "fullname",
    "cardholder",
    "cardholdername",
    "holder",
    "holdername",
    "payername",
    "customername",
    "buyername",
    "clientname",
    "username",
    "login",
];
const NAMED_THINGS: [&str; 14] = [
    "product",
    "offer",
    "tier",
    "items",
    "item",
    "receipt",
    "shop",
    "merchant",
    "project",
    "method",
    "paymentmethod",
    "bank",
    "issuer",
    "asset",
];
const PLACE_KEYS: [&str; 18] = [
    "address",
    "street",
    "building",
    "house",
    "suite",
    "flat",
    "apartment",
    "zip",
    "zipcode",
    "postcode",
    "postalcode",
    "city",
    "district",
    "ipcity",
    "ipregion",
    "ipdistrict",
    "iplatitude",
    "iplongitude",
];
const IP_KEYS: [&str; 10] = [
    "ip",
    "ipaddress",
    "clientip",
    "clientipaddress",
    "payerip",
    "userip",
    "useripaddress",
    "customerip",
    "buyerip",
    "remoteip",
];
const CARD_KEYS: [&str; 7] = [
    "pan",
    "cardnumber",
    "cardmask",
    "maskedpan",
    "cardpan",
    "accountnumber",
    "cardnum",
];
const CARD_START_KEYS: [&str; 5] = ["cardfirstsix", "first6", "firstsix", "bin", "cardbin"];
const EXPIRY_KEYS: [&str; 9] = [
    "cardexpdate",
    "expdate",
    "expiry",
    "expirydate",
    "expirymonth",
    "expiryyear",
    "expmonth",
    "expyear",
    "exp",
];
const WALLET_KEYS: [&str; 6] = [
    "sender",
    "from",
    "payeraccount",
    "payerwallet",
    "senderwallet",
    "fromaddress",
];
const TEXT_KEYS: [&str; 3] = ["comment", "payercomment", "usercomment"];

fn rule(parent: Option<&str>, key: &str) -> Option<Rule> {
    let k = normalized(key);
    let p = parent.map(normalized).unwrap_or_default();
    if k.is_empty() {
        return None;
    }
    if SECRET_KEYS.contains(&k.as_str())
        || k.ends_with("sign")
        || SECRET_PARTS.iter().any(|part| k.contains(part))
        || (k == "id" && METHOD_PARENTS.contains(&p.as_str()))
    {
        return Some(Rule::Secret);
    }
    if k.contains("email") || k == "mail" {
        return Some(Rule::Email);
    }
    if k.contains("phone") || k == "msisdn" || k == "tel" {
        return Some(Rule::Phone);
    }
    if PERSON_KEYS.contains(&k.as_str()) {
        if k == "name" && NAMED_THINGS.contains(&p.as_str()) {
            return None;
        }
        return Some(Rule::Person);
    }
    if PLACE_KEYS.contains(&k.as_str()) || k.contains("latitude") || k.contains("longitude") {
        return Some(Rule::Place);
    }
    if IP_KEYS.contains(&k.as_str()) {
        return Some(Rule::Ip);
    }
    if CARD_KEYS.contains(&k.as_str()) || (k == "number" && p == "card") {
        return Some(Rule::Card);
    }
    if CARD_START_KEYS.contains(&k.as_str()) {
        return Some(Rule::CardStart);
    }
    if EXPIRY_KEYS.contains(&k.as_str()) {
        return Some(Rule::Expiry);
    }
    if WALLET_KEYS.contains(&k.as_str()) {
        return Some(Rule::Wallet);
    }
    if TEXT_KEYS.contains(&k.as_str()) {
        return Some(Rule::Text);
    }
    None
}

fn first_char(text: &str) -> String {
    text.trim()
        .chars()
        .next()
        .map(String::from)
        .unwrap_or_default()
}

fn head_tail(text: &str) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    if chars.len() <= 10 {
        return HIDDEN.to_string();
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{head}…{tail}")
}

fn email(text: &str) -> String {
    match text.trim().rsplit_once('@') {
        Some((local, domain)) if !local.is_empty() && !domain.is_empty() => {
            format!("{}{HIDDEN}@{domain}", first_char(local))
        }
        _ => HIDDEN.to_string(),
    }
}

fn phone(text: &str) -> String {
    let digits = text.chars().filter(char::is_ascii_digit).count();
    if digits < 5 {
        return HIDDEN.to_string();
    }
    let mut seen = 0;
    text.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_digit() {
                seen += 1;
                if seen > digits - 2 { c } else { '•' }
            } else {
                c
            }
        })
        .collect()
}

fn ip(text: &str) -> String {
    let text = text.trim();
    if let Ok(std::net::IpAddr::V4(v4)) = text.parse::<std::net::IpAddr>() {
        let [a, b, c, _] = v4.octets();
        return format!("{a}.{b}.{c}.•");
    }
    if let Ok(std::net::IpAddr::V6(v6)) = text.parse::<std::net::IpAddr>() {
        let segments = v6.segments();
        return format!("{:x}:{:x}:{HIDDEN}", segments[0], segments[1]);
    }
    HIDDEN.to_string()
}

fn card(text: &str) -> String {
    let digits: Vec<char> = text.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return HIDDEN.to_string();
    }
    let tail: String = digits.iter().skip(digits.len() - 4).collect();
    format!("•••• {tail}")
}

fn place(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() >= 16 && !trimmed.contains(char::is_whitespace) {
        head_tail(trimmed)
    } else {
        HIDDEN.to_string()
    }
}

fn masked_text(rule: Rule, text: &str) -> String {
    if text.trim().is_empty() || text.contains('•') || text.contains('…') {
        return text.to_string();
    }
    match rule {
        Rule::Secret | Rule::Text => HIDDEN.to_string(),
        Rule::Email => email(text),
        Rule::Phone => phone(text),
        Rule::Person => format!("{}{HIDDEN}", first_char(text)),
        Rule::Place => place(text),
        Rule::Ip => ip(text),
        Rule::Card => card(text),
        Rule::CardStart => "••••••".to_string(),
        Rule::Expiry => "••".to_string(),
        Rule::Wallet => head_tail(text),
    }
}

fn masked_value(rule: Rule, value: &Value) -> Option<Value> {
    match value {
        Value::String(text) => {
            let masked = masked_text(rule, text);
            (masked != *text).then_some(Value::String(masked))
        }
        Value::Number(number) => Some(Value::String(masked_text(rule, &number.to_string()))),
        _ => None,
    }
}

fn nested(text: &str, depth: u8) -> Option<String> {
    let trimmed = text.trim_start();
    if depth >= NESTED_DEPTH || !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let mut inner: Value = serde_json::from_str(text).ok()?;
    walk(&mut inner, None, depth + 1)
        .then(|| serde_json::to_string(&inner).ok())
        .flatten()
}

fn walk(value: &mut Value, parent: Option<&str>, depth: u8) -> bool {
    match value {
        Value::Object(map) => {
            let mut changed = false;
            for (key, item) in map.iter_mut() {
                if let Some(rule) = rule(parent, key)
                    && !matches!(item, Value::Object(_) | Value::Array(_))
                {
                    if let Some(masked) = masked_value(rule, item) {
                        *item = masked;
                        changed = true;
                    }
                    continue;
                }
                changed |= walk(item, Some(key), depth);
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            for item in items.iter_mut() {
                changed |= walk(item, parent, depth);
            }
            changed
        }
        Value::String(text) => match nested(text, depth) {
            Some(masked) => {
                *text = masked;
                true
            }
            None => false,
        },
        _ => false,
    }
}

fn percent_decode(raw: &str) -> Option<String> {
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
                let hex = bytes.get(pos + 1..pos + 3)?;
                let text = std::str::from_utf8(hex).ok()?;
                out.push(u8::from_str_radix(text, 16).ok()?);
                pos += 3;
            }
            other => {
                out.push(other);
                pos += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn form_pairs(text: &str) -> Option<Vec<(String, String)>> {
    if !text.contains('=') || text.contains(char::is_whitespace) {
        return None;
    }
    text.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Some((percent_decode(key)?, percent_decode(value)?))
        })
        .collect()
}

fn mask_pairs(pairs: &mut [(String, String)]) -> bool {
    let mut changed = false;
    for (key, value) in pairs.iter_mut() {
        let masked = match rule(None, key) {
            Some(rule) => masked_text(rule, value),
            None => match nested(value, 0) {
                Some(inner) => inner,
                None => continue,
            },
        };
        if masked != *value {
            *value = masked;
            changed = true;
        }
    }
    changed
}

fn clip(text: &str) -> String {
    text.chars().take(TEXT_LIMIT).collect()
}

pub fn mask_body(body: &[u8]) -> (MaskedBody, bool) {
    let Ok(text) = std::str::from_utf8(body) else {
        return (MaskedBody::Binary(body.len()), false);
    };
    let trimmed = text.trim_start();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(mut value) = serde_json::from_str::<Value>(text)
    {
        let changed = walk(&mut value, None, 0);
        return (MaskedBody::Json(value), changed);
    }
    if let Some(mut pairs) = sn_payments::form::multipart(body) {
        mask_pairs(&mut pairs);
        return (MaskedBody::Form(pairs), true);
    }
    if let Some(mut pairs) = form_pairs(text.trim()) {
        let changed = mask_pairs(&mut pairs);
        return (MaskedBody::Form(pairs), changed);
    }
    (MaskedBody::Text(clip(text)), false)
}

const EVENT_KEYS: [&str; 10] = [
    "event",
    "eventType",
    "event_type",
    "update_type",
    "notification_type",
    "type",
    "OperationType",
    "status",
    "Status",
    "payment_status",
];
const EVENT_LIMIT: usize = 64;

fn event_name(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.chars().count() <= EVENT_LIMIT && !v.contains('•'))
        .map(str::to_string)
}

pub fn body_event(body: &MaskedBody) -> Option<String> {
    match body {
        MaskedBody::Json(value) => EVENT_KEYS
            .iter()
            .find_map(|key| event_name(value.get(*key).and_then(Value::as_str))),
        MaskedBody::Form(pairs) => EVENT_KEYS.iter().find_map(|key| {
            event_name(
                pairs
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.as_str()),
            )
        }),
        MaskedBody::Text(_) | MaskedBody::Binary(_) => None,
    }
}

pub fn route_event(route: Option<&str>) -> Option<String> {
    route
        .map(str::trim)
        .filter(|r| {
            (1..=16).contains(&r.len())
                && r.bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_' || b == b'-')
        })
        .map(str::to_string)
}

pub struct Prepared {
    pub body: Vec<u8>,
    pub event: Option<String>,
}

pub fn prepare(body: &[u8], route: Option<&str>) -> Prepared {
    let (masked, changed) = mask_body(body);
    let event = route_event(route).or_else(|| body_event(&masked));
    Prepared {
        body: stored_from(body, masked, changed).into_owned(),
        event,
    }
}

fn stored_from(body: &[u8], masked: MaskedBody, changed: bool) -> Cow<'_, [u8]> {
    match (masked, changed) {
        (MaskedBody::Json(value), true) => match serde_json::to_vec(&value) {
            Ok(bytes) => Cow::Owned(bytes),
            Err(_) => Cow::Borrowed(body),
        },
        (MaskedBody::Form(pairs), true) => Cow::Owned(
            pairs
                .iter()
                .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
                .collect::<Vec<_>>()
                .join("&")
                .into_bytes(),
        ),
        _ => Cow::Borrowed(body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn json_of(body: &[u8]) -> Value {
        match mask_body(body).0 {
            MaskedBody::Json(value) => value,
            other => panic!("ожидался JSON, получено {other:?}"),
        }
    }

    #[test]
    fn подписи_токены_и_ключи_скрываются_целиком() {
        let body = json!({
            "sign": "a1b2c3d4e5f6",
            "sha1_hash": "ac13833bd6ba9eff1fa9e4bed76f3d6ebb57f6c0",
            "Token": "tk_live_0123456789",
            "api_key": "secret-key",
            "object": { "payment_method": { "id": "2d9e2f42-000f-5000-9000-1b2e0b2f3c11", "saved": true, "title": "Bank card *4444" } },
            "amount": { "value": "499.00", "currency": "RUB" },
            "status": "succeeded"
        })
        .to_string();
        let masked = json_of(body.as_bytes());
        assert_eq!(masked["sign"], "•••");
        assert_eq!(masked["sha1_hash"], "•••");
        assert_eq!(masked["Token"], "•••");
        assert_eq!(masked["api_key"], "•••");
        assert_eq!(masked["object"]["payment_method"]["id"], "•••");
        assert_eq!(masked["object"]["payment_method"]["saved"], true);
        assert_eq!(
            masked["object"]["payment_method"]["title"],
            "Bank card *4444"
        );
        assert_eq!(masked["amount"]["value"], "499.00");
        assert_eq!(masked["status"], "succeeded");
        let stored = String::from_utf8(prepare(body.as_bytes(), None).body).unwrap();
        for secret in [
            "a1b2c3d4e5f6",
            "tk_live_0123456789",
            "secret-key",
            "2d9e2f42-000f",
        ] {
            assert!(
                !stored.contains(secret),
                "{secret} остался в сохранённом теле"
            );
        }
    }

    #[test]
    fn личные_данные_маскируются_с_безопасной_частью() {
        let body = json!({
            "Email": "ivan.petrov@example.com",
            "Phone": "+79161234567",
            "Name": "IVAN PETROV",
            "IpAddress": "203.0.113.45",
            "IpCity": "Москва",
            "CardFirstSix": "220220",
            "CardLastFour": "4444",
            "CardExpDate": "12/29",
            "buyer": { "email": "a@b.ru", "cardMask": "220220******4444" },
            "from": "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE",
            "comment": "оплата за маму",
            "product": { "name": "VPN на месяц" },
            "AccountId": "5b8a3c2e-0000-4000-8000-000000000001"
        })
        .to_string();
        let masked = json_of(body.as_bytes());
        assert_eq!(masked["Email"], "i•••@example.com");
        assert_eq!(masked["Phone"], "+•••••••••67");
        assert_eq!(masked["Name"], "I•••");
        assert_eq!(masked["IpAddress"], "203.0.113.•");
        assert_eq!(masked["IpCity"], "•••");
        assert_eq!(masked["CardFirstSix"], "••••••");
        assert_eq!(masked["CardLastFour"], "4444");
        assert_eq!(masked["CardExpDate"], "••");
        assert_eq!(masked["buyer"]["email"], "a•••@b.ru");
        assert_eq!(masked["buyer"]["cardMask"], "•••• 4444");
        assert_eq!(masked["from"], "TQn9…bLSE");
        assert_eq!(masked["comment"], "•••");
        assert_eq!(masked["product"]["name"], "VPN на месяц");
        assert_eq!(masked["AccountId"], "5b8a3c2e-0000-4000-8000-000000000001");
    }

    #[test]
    fn форма_маскируется_по_полям_и_сохраняется_формой() {
        let body = b"notification_type=p2p-incoming&amount=485.03&sender=410011234567890&label=6f1c&sha1_hash=ac13833bd6ba9eff1fa9e4bed76f3d6ebb57f6c0&email=ivan%40example.com&lastname=%D0%9F%D0%B5%D1%82%D1%80%D0%BE%D0%B2";
        let (masked, changed) = mask_body(body);
        assert!(changed);
        let MaskedBody::Form(pairs) = masked else {
            panic!("ожидалась форма");
        };
        let field = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(field("notification_type"), "p2p-incoming");
        assert_eq!(field("amount"), "485.03");
        assert_eq!(field("sender"), "4100…7890");
        assert_eq!(field("sha1_hash"), "•••");
        assert_eq!(field("email"), "i•••@example.com");
        assert_eq!(field("lastname"), "П•••");
        let stored = String::from_utf8(prepare(body, None).body).unwrap();
        assert!(stored.starts_with("notification_type=p2p-incoming&amount=485.03&"));
        assert!(!stored.contains("ac13833bd6ba"));
        assert!(!stored.contains("ivan%40example.com"));
    }

    #[test]
    fn json_внутри_строки_тоже_маскируется() {
        let body = b"TransactionId=42&Data=%7B%22email%22%3A%22ivan%40example.com%22%2C%22plan%22%3A%22m%22%7D";
        let (masked, changed) = mask_body(body);
        assert!(changed);
        let MaskedBody::Form(pairs) = masked else {
            panic!("ожидалась форма");
        };
        assert_eq!(pairs[1].1, r#"{"email":"i•••@example.com","plan":"m"}"#);
    }

    #[test]
    fn без_личных_данных_тело_хранится_как_пришло() {
        let body = br#"{"status": "paid",  "amount": 100.00, "payment": "abc"}"#;
        assert_eq!(prepare(body, None).body, body.to_vec());
        let text = b"not json and not form";
        assert_eq!(
            mask_body(text).0,
            MaskedBody::Text("not json and not form".into())
        );
        assert_eq!(mask_body(&[0xff, 0xfe]).0, MaskedBody::Binary(2));
        assert_eq!(prepare(&[0xff, 0xfe], None).body, vec![0xff, 0xfe]);
    }

    #[test]
    fn событие_берётся_из_адреса_или_тела() {
        let json = br#"{"update_type":"invoice_paid","payload":{"invoice_id":7}}"#;
        assert_eq!(prepare(json, None).event.as_deref(), Some("invoice_paid"));
        let form = b"notification_type=p2p-incoming&operation_id=1";
        assert_eq!(prepare(form, None).event.as_deref(), Some("p2p-incoming"));
        assert_eq!(prepare(form, Some("check")).event.as_deref(), Some("check"));
        let token = "0f3a9c2b4d5e6f708192a3b4c5d6e7f8";
        assert_eq!(prepare(b"{}", Some(token)).event, None);
        assert_eq!(prepare(br#"{"status":"VERY_LONG_STATUS_NAME_THAT_GOES_ON_AND_ON_AND_ON_AND_ON_FOREVER_AND_EVER"}"#, None).event, None);
        assert_eq!(prepare(b"\xff", None).event, None);
    }

    #[test]
    fn повторная_маска_ничего_не_меняет() {
        let body = json!({
            "email": "ivan@example.com",
            "ip": "203.0.113.45",
            "phone": "+79161234567",
            "pan": "4111111111111111",
            "from": "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE",
            "sign": "abc"
        })
        .to_string();
        let once = prepare(body.as_bytes(), None).body;
        assert_eq!(prepare(&once, None).body, once);
        let (again, changed) = mask_body(&once);
        assert!(!changed);
        let MaskedBody::Json(value) = again else {
            panic!("ожидался JSON");
        };
        assert_eq!(value["ip"], "203.0.113.•");
        assert_eq!(value["email"], "i•••@example.com");
    }

    #[test]
    fn короткие_и_странные_значения_не_ломают_маску() {
        let body = json!({
            "email": "без собаки",
            "phone": "12",
            "ip": "не адрес",
            "pan": "12",
            "sender": "short",
            "name": "",
            "token": 12345,
            "deep": [[[[{"password": "x"}]]]]
        })
        .to_string();
        let masked = json_of(body.as_bytes());
        assert_eq!(masked["email"], "•••");
        assert_eq!(masked["phone"], "•••");
        assert_eq!(masked["ip"], "•••");
        assert_eq!(masked["pan"], "•••");
        assert_eq!(masked["sender"], "•••");
        assert_eq!(masked["name"], "");
        assert_eq!(masked["token"], "•••");
        assert_eq!(masked["deep"][0][0][0][0]["password"], "•••");
    }

    fn form_of(body: &[u8]) -> Vec<(String, String)> {
        match mask_body(body).0 {
            MaskedBody::Form(pairs) => pairs,
            other => panic!("ожидалась форма, получено {other:?}"),
        }
    }

    fn value<'a>(pairs: &'a [(String, String)], key: &str) -> &'a str {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or_default()
    }

    #[test]
    fn уведомления_kassaai_и_paypalych_маскируются_в_форме_и_multipart() {
        let kassa = "MERCHANT_ID=777&AMOUNT=1500&intid=652367&MERCHANT_ORDER_ID=6a1c0c8e-3f4b-4d2a-9b7e-51c02d3e4f50&P_EMAIL=ivan.petrov%40example.com&P_PHONE=79161234567&CUR_ID=44&payer_account=2200123456781234&SIGN=30c76991785c43f10af2079878acd3d6";
        let pairs = form_of(kassa.as_bytes());
        assert_eq!(value(&pairs, "SIGN"), "•••");
        assert_eq!(value(&pairs, "AMOUNT"), "1500");
        assert_ne!(value(&pairs, "P_EMAIL"), "ivan.petrov@example.com");
        assert_ne!(value(&pairs, "P_PHONE"), "79161234567");
        assert_ne!(value(&pairs, "payer_account"), "2200123456781234");
        let mut multipart = String::new();
        for (key, val) in kassa.split('&').filter_map(|p| p.split_once('=')) {
            let val = val.replace("%40", "@");
            multipart.push_str(&format!(
                "--Zb7\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{val}\r\n"
            ));
        }
        multipart.push_str("--Zb7--\r\n");
        let stored = String::from_utf8(prepare(multipart.as_bytes(), None).body).unwrap();
        for secret in [
            "30c76991785c43f10af2079878acd3d6",
            "ivan.petrov@",
            "79161234567",
            "2200123456781234",
        ] {
            assert!(!stored.contains(secret), "{secret} остался в {stored}");
        }
        assert_eq!(value(&form_of(stored.as_bytes()), "AMOUNT"), "1500");
        let palych = "Status=SUCCESS&InvId=6a1c0c8e-3f4b-4d2a-9b7e-51c02d3e4f50&OutSum=1500.00&TrsId=GkLWvKx3&AccountNumber=220220%2A%2A%2A%2A%2A%2A7046&PayerEmail=ivan.petrov%40example.com&SignatureValue=DC32D403CE6BD2E54D4D213278DB286C";
        let prepared = prepare(palych.as_bytes(), None);
        assert_eq!(prepared.event.as_deref(), Some("SUCCESS"));
        let stored = String::from_utf8(prepared.body).unwrap();
        assert!(
            !stored.contains("DC32D403CE6BD2E54D4D213278DB286C"),
            "{stored}"
        );
        assert!(!stored.contains("ivan.petrov@"), "{stored}");
        let mulen = json!({"id": 777, "amount": 1500.5, "currency": "RUB", "uuid": "6a1c0c8e-3f4b-4d2a-9b7e-51c02d3e4f50", "payment_status": "success"}).to_string();
        assert_eq!(
            prepare(mulen.as_bytes(), None).event.as_deref(),
            Some("success")
        );
    }
}
