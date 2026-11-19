use serde_json::{Map, Value};

pub const HIDDEN: &str = "скрыто";

const SECRET_PARTS: [&str; 15] = [
    "password",
    "passwd",
    "secret",
    "token",
    "apikey",
    "privatekey",
    "accesskey",
    "authorization",
    "cookie",
    "signature",
    "credential",
    "totp",
    "seed",
    "mnemonic",
    "hash",
];
const SECRET_KEYS: [&str; 9] = [
    "sign", "cvv", "cvv2", "cvc", "cvc2", "pin", "otp", "pan", "key",
];
const NAME_LISTS: [&str; 6] = ["secrets", "fields", "keys", "changed", "cleared", "missing"];

fn normalized(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

pub fn is_secret_key(key: &str) -> bool {
    let k = normalized(key);
    if k.is_empty() || NAME_LISTS.contains(&k.as_str()) {
        return false;
    }
    SECRET_KEYS.contains(&k.as_str()) || SECRET_PARTS.iter().any(|part| k.contains(part))
}

fn is_uuid(word: &str) -> bool {
    let parts: Vec<&str> = word.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(part, len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

fn is_bot_token(word: &str) -> bool {
    let Some((id, rest)) = word.split_once(':') else {
        return false;
    };
    (6..=12).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_digit())
        && rest.len() >= 30
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_jwt(word: &str) -> bool {
    word.starts_with("eyJ")
        && word.split('.').count() == 3
        && word.len() >= 30
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn is_opaque(word: &str) -> bool {
    word.len() >= 32
        && !is_uuid(word)
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-'))
        && word.chars().any(|c| c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_alphabetic())
}

pub fn looks_secret(word: &str) -> bool {
    let word =
        word.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | '(' | ')' | '<' | '>'));
    word.starts_with("snk_") && word.len() > 12
        || is_bot_token(word)
        || is_jwt(word)
        || is_opaque(word)
}

fn url_password(word: &str) -> Option<String> {
    let (scheme, rest) = word.split_once("://")?;
    let (auth, host) = rest.split_once('@')?;
    let (user, password) = auth.split_once(':')?;
    if password.is_empty() || auth.contains('/') {
        return None;
    }
    Some(format!("{scheme}://{user}:{HIDDEN}@{host}"))
}

fn scrub_word(word: &str, after_scheme: bool) -> String {
    if after_scheme {
        return HIDDEN.into();
    }
    if let Some(clean) = url_password(word) {
        return clean;
    }
    if looks_secret(word) {
        return HIDDEN.into();
    }
    let mut out = String::with_capacity(word.len());
    let mut piece = String::new();
    for c in word.chars() {
        if matches!(c, ':' | '=' | ',' | ';' | '"' | '\'' | '(' | ')') {
            out.push_str(if looks_secret(&piece) { HIDDEN } else { &piece });
            piece.clear();
            out.push(c);
        } else {
            piece.push(c);
        }
    }
    out.push_str(if looks_secret(&piece) { HIDDEN } else { &piece });
    out
}

pub fn scrub_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let mut scheme = false;
    let flush = |word: &mut String, out: &mut String, scheme: &mut bool| {
        if word.is_empty() {
            return;
        }
        out.push_str(&scrub_word(word, *scheme));
        *scheme = word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic");
        word.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut out, &mut scheme);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out, &mut scheme);
    out
}

fn scrub_in(value: &Value, secret: bool) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (key, inner) in map {
                out.insert(key.clone(), scrub_in(inner, secret || is_secret_key(key)));
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| scrub_in(item, secret)).collect())
        }
        Value::String(text) if secret && !text.is_empty() => Value::String(HIDDEN.into()),
        Value::String(text) => Value::String(scrub_text(text)),
        other => other.clone(),
    }
}

pub fn scrub(value: &Value) -> Value {
    scrub_in(value, false)
}
