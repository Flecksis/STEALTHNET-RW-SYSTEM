use crate::error::PaymentResult;
use crate::json::Json;
use crate::provider::WebhookRequest;
use crate::util;

fn boundary(body: &[u8]) -> Option<&[u8]> {
    let line_end = body.windows(2).position(|w| w == b"\r\n")?;
    let marker = body.get(..line_end)?.strip_prefix(b"--")?;
    (!marker.is_empty() && marker.len() <= 200 && marker.iter().all(u8::is_ascii_graphic))
        .then_some(marker)
}

fn part_name(head: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (key, params) = line.split_once(':')?;
        if !key.trim().eq_ignore_ascii_case("content-disposition") {
            return None;
        }
        params.split(';').find_map(|part| {
            let (k, v) = part.trim().split_once('=')?;
            (k.trim() == "name").then(|| v.trim().trim_matches('"').to_string())
        })
    })
}

pub fn multipart(body: &[u8]) -> Option<Vec<(String, String)>> {
    let marker = boundary(body)?;
    let mut delimiter = b"\r\n--".to_vec();
    delimiter.extend_from_slice(marker);
    let mut rest = body.get(2 + marker.len()..)?;
    let mut pairs: Vec<(String, String)> = Vec::new();
    loop {
        if rest.starts_with(b"--") {
            return Some(pairs);
        }
        rest = rest.strip_prefix(b"\r\n")?;
        let head_end = rest.windows(4).position(|w| w == b"\r\n\r\n")?;
        let head = std::str::from_utf8(rest.get(..head_end)?).ok()?;
        let value_start = head_end + 4;
        let value_len = rest
            .get(value_start..)?
            .windows(delimiter.len())
            .position(|w| w == delimiter.as_slice())?;
        let value = std::str::from_utf8(rest.get(value_start..value_start + value_len)?).ok()?;
        let name = part_name(head)?;
        if pairs.iter().any(|(k, _)| *k == name) {
            return None;
        }
        pairs.push((name, value.to_string()));
        rest = rest.get(value_start + value_len + delimiter.len()..)?;
    }
}

fn json_pairs(body: &[u8]) -> Option<Vec<(String, String)>> {
    let Json::Object(members) = Json::parse(body)? else {
        return None;
    };
    members
        .into_iter()
        .map(|(key, value)| match value {
            Json::String(text) | Json::Number(text) => Some((key, text)),
            Json::Null => Some((key, String::new())),
            _ => None,
        })
        .collect()
}

pub(crate) fn fields(request: &WebhookRequest) -> PaymentResult<Vec<(String, String)>> {
    if let Some(pairs) = multipart(&request.body) {
        return Ok(pairs);
    }
    if request.body.trim_ascii_start().starts_with(b"{") {
        return json_pairs(&request.body)
            .ok_or_else(|| util::bad("тело уведомления не плоский JSON"));
    }
    let body: &[u8] = if request.body.iter().all(u8::is_ascii_whitespace) {
        request.query.as_deref().unwrap_or_default().as_bytes()
    } else {
        &request.body
    };
    util::form_pairs(body.trim_ascii())
}

pub(crate) fn encode(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", util::rfc3986(key), util::rfc3986(value)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::hook;

    #[test]
    fn поля_формы_из_тела_multipart_json_и_строки_запроса() {
        let urlencoded = fields(&hook(&[], "a=1&b=%D0%9F+x")).unwrap();
        assert_eq!(util::field(&urlencoded, "b"), Some("П x"));
        let body = "--B1\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--B1\r\nContent-Disposition: form-data; name=\"b\"\r\n\r\nП x\r\n--B1--\r\n";
        let parts = fields(&hook(&[], body)).unwrap();
        assert_eq!(parts, urlencoded);
        let json = fields(&hook(&[], r#"{"a":1,"b":"П x","c":null}"#)).unwrap();
        assert_eq!(util::field(&json, "a"), Some("1"));
        assert_eq!(util::field(&json, "c"), Some(""));
        let mut get = hook(&[], "");
        get.query = Some("a=1&b=%D0%9F+x".into());
        assert_eq!(fields(&get).unwrap(), urlencoded);
        assert!(
            multipart(b"--B1\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1").is_none()
        );
        let twice = "--B1\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--B1\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n2\r\n--B1--\r\n";
        assert!(multipart(twice.as_bytes()).is_none());
        assert!(fields(&hook(&[], r#"{"a":{"b":1}}"#)).is_err());
    }

    #[test]
    fn форма_кодируется_для_php() {
        assert_eq!(
            encode(&[
                ("amount", "1500.00".into()),
                ("payer_data[email]", "a+b@x.ru".into())
            ]),
            "amount=1500.00&payer_data%5Bemail%5D=a%2Bb%40x.ru"
        );
    }
}
