use crate::Services;
use sn_modules::catalog::antibot::captcha::Endpoints;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

pub(crate) type Form = HashMap<String, String>;

pub(crate) struct Siteverify {
    _server: MockServer,
    seen: Arc<Mutex<Vec<Form>>>,
}

impl Siteverify {
    pub fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    pub fn last(&self) -> Form {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }
}

fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'%'
            && i + 3 <= bytes.len()
            && let Ok(value) = u8::from_str_radix(&raw[i + 1..i + 3], 16)
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(if byte == b'+' { b' ' } else { byte });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn form(body: &[u8]) -> Form {
    String::from_utf8_lossy(body)
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (decode(key), decode(value)))
        .collect()
}

pub(crate) const SLOW: Duration = Duration::from_secs(4);

fn turnstile(form: &Form) -> ResponseTemplate {
    let token = form.get("response").cloned().unwrap_or_default();
    if let Some(rest) = token.strip_prefix("slow:") {
        let mut form = form.clone();
        form.insert("response".into(), rest.into());
        return turnstile(&form).set_delay(SLOW);
    }
    if token.starts_with("down") {
        return ResponseTemplate::new(500).set_body_string("upstream failed");
    }
    if token.starts_with("secret") {
        return ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "error-codes": ["invalid-input-secret"],
        }));
    }
    if token.starts_with("bare") {
        return ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "challenge_ts": "2026-11-24T10:00:00.000Z",
            "hostname": "example.com",
            "error-codes": [],
        }));
    }
    if let Some(rest) = token.strip_prefix("ok:") {
        let (action, cdata) = rest.split_once(':').unwrap_or((rest, ""));
        let mut body = serde_json::json!({
            "success": true,
            "challenge_ts": "2026-11-24T10:00:00.000Z",
            "hostname": "panel.example.com",
            "error-codes": [],
        });
        if !action.is_empty() {
            body["action"] = serde_json::json!(action);
        }
        if !cdata.is_empty() {
            body["cdata"] = serde_json::json!(cdata);
        }
        return ResponseTemplate::new(200).set_body_json(body);
    }
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "success": false,
        "error-codes": ["invalid-input-response"],
    }))
}

fn yandex(form: &Form) -> ResponseTemplate {
    let token = form.get("token").cloned().unwrap_or_default();
    if token.starts_with("down") {
        return ResponseTemplate::new(503).set_body_string("unavailable");
    }
    let body = if token.starts_with("nohost") {
        serde_json::json!({ "status": "ok", "message": "", "host": "" })
    } else if token.starts_with("ok:port") {
        serde_json::json!({ "status": "ok", "message": "", "host": "panel.example.com:8443" })
    } else if token.starts_with("ok") {
        serde_json::json!({ "status": "ok", "message": "", "host": "panel.example.com" })
    } else if token.starts_with("secret") {
        return ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "status": "failed",
            "message": "Authentication failed. Invalid secret.",
        }));
    } else {
        serde_json::json!({ "status": "failed", "message": "Invalid or expired Token." })
    };
    ResponseTemplate::new(200).set_body_json(body)
}

pub(crate) async fn siteverify(services: &Services) -> Siteverify {
    let server = MockServer::start().await;
    let seen: Arc<Mutex<Vec<Form>>> = Arc::default();
    let log = seen.clone();
    Mock::given(method("POST"))
        .and(path("/turnstile"))
        .respond_with(move |request: &Request| {
            let form = form(&request.body);
            log.lock().unwrap().push(form.clone());
            turnstile(&form)
        })
        .mount(&server)
        .await;
    let log = seen.clone();
    Mock::given(method("POST"))
        .and(path("/yandex"))
        .respond_with(move |request: &Request| {
            let form = form(&request.body);
            log.lock().unwrap().push(form.clone());
            yandex(&form)
        })
        .mount(&server)
        .await;
    services.set_captcha_endpoints(Endpoints {
        turnstile: format!("{}/turnstile", server.uri()),
        yandex: format!("{}/yandex", server.uri()),
    });
    Siteverify {
        _server: server,
        seen,
    }
}

#[test]
fn форма_разбирается_с_кодированием() {
    let parsed = form(b"secret=a%2Bb&remoteip=2001%3Adb8%3A%3A1&response=ok%3Atrial%3Aabc&x=1+2");
    assert_eq!(parsed["secret"], "a+b");
    assert_eq!(parsed["remoteip"], "2001:db8::1");
    assert_eq!(parsed["response"], "ok:trial:abc");
    assert_eq!(parsed["x"], "1 2");
}
