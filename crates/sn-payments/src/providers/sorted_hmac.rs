use crate::json::Json;
use hmac::{Hmac, Mac};
use sha2::Sha256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    Raw,
    Php,
}

fn php_number(raw: &str) -> String {
    if raw.contains(['e', 'E']) || !raw.contains('.') {
        return raw.to_string();
    }
    let trimmed = raw.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn value_text(value: &Json, style: Style) -> String {
    match (value, style) {
        (Json::Null, _) => String::new(),
        (Json::String(text), _) => text.clone(),
        (Json::Number(raw), Style::Raw) => raw.clone(),
        (Json::Number(raw), Style::Php) => php_number(raw),
        (Json::Bool(flag), Style::Php) => if *flag { "1" } else { "" }.to_string(),
        (Json::Array(_) | Json::Object(_), Style::Php) => "Array".to_string(),
        (other, Style::Raw) => String::from_utf8(other.to_bytes()).unwrap_or_default(),
    }
}

pub(crate) fn signed_text(body: &Json, style: Style) -> Option<String> {
    let Json::Object(members) = body else {
        return None;
    };
    let mut sorted: Vec<&(String, Json)> = members.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Some(
        sorted
            .iter()
            .map(|(_, value)| value_text(value, style))
            .collect(),
    )
}

pub(crate) fn signature(secret: &str, body: &[u8], style: Style) -> String {
    Json::parse(body)
        .as_ref()
        .and_then(|body| signed_text(body, style))
        .and_then(|text| {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
            mac.update(text.as_bytes());
            Some(hex::encode(mac.finalize().into_bytes()))
        })
        .unwrap_or_default()
}

pub(crate) fn verify(secret: &str, body: &Json, header: &str, style: Style) -> bool {
    let Ok(expected) = hex::decode(header.trim()) else {
        return false;
    };
    let Some(text) = signed_text(body, style) else {
        return false;
    };
    Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map(|mut mac| {
            mac.update(text.as_bytes());
            mac.verify_slice(&expected).is_ok()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn числа_и_флаги_как_у_php() {
        for (raw, php) in [
            ("1250", "1250"),
            ("1250.0", "1250"),
            ("1209.10", "1209.1"),
            ("1209.01", "1209.01"),
            ("0.50", "0.5"),
            ("0.0", "0"),
            ("1e3", "1e3"),
        ] {
            assert_eq!(php_number(raw), php, "{raw}");
        }
        let body = Json::parse(br#"{"b":true,"a":false,"c":null,"d":{"x":1},"e":19.0}"#).unwrap();
        assert_eq!(signed_text(&body, Style::Php).as_deref(), Some("1Array19"));
        assert_eq!(
            signed_text(&body, Style::Raw).as_deref(),
            Some("falsetrue{\"x\":1}19.0")
        );
        assert_eq!(signed_text(&Json::parse(b"[1]").unwrap(), Style::Php), None);
    }
}
