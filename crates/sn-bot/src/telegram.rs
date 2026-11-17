use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_API: &str = "https://api.telegram.org";
pub const CALLBACK_DATA_MAX: usize = 64;
pub const MESSAGE_MAX: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TelegramError {
    #[error("Telegram недоступен: {0}")]
    Transport(String),
    #[error("Telegram отказал ({code}): {description}")]
    Api {
        code: i64,
        description: String,
        retry_after: Option<u64>,
    },
    #[error("ответ Telegram не разобран: {0}")]
    Decode(String),
}

impl TelegramError {
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            TelegramError::Api {
                code: 429,
                retry_after,
                ..
            } => Some(retry_after.unwrap_or(5)),
            _ => None,
        }
    }

    pub fn is_forbidden(&self) -> bool {
        matches!(self, TelegramError::Api { code: 403, .. })
    }

    pub fn is_not_modified(&self) -> bool {
        matches!(self, TelegramError::Api { code: 400, description, .. } if description.contains("message is not modified"))
    }

    pub fn is_parse_error(&self) -> bool {
        matches!(self, TelegramError::Api { code: 400, description, .. } if description.contains("can't parse entities"))
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            TelegramError::Transport(_) | TelegramError::Decode(_) => true,
            TelegramError::Api { code, .. } => *code == 429 || *code >= 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotUser {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub language_code: Option<String>,
    #[serde(default)]
    pub is_premium: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type", default)]
    pub kind: String,
}

impl Chat {
    pub fn is_private(&self) -> bool {
        self.kind == "private"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessfulPayment {
    pub currency: String,
    pub total_amount: i64,
    pub invoice_payload: String,
    pub telegram_payment_charge_id: String,
    #[serde(default)]
    pub provider_payment_charge_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundedPayment {
    pub currency: String,
    pub total_amount: i64,
    pub invoice_payload: String,
    pub telegram_payment_charge_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub message_id: i64,
    #[serde(default)]
    pub from: Option<BotUser>,
    pub chat: Chat,
    #[serde(default)]
    pub date: i64,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub successful_payment: Option<SuccessfulPayment>,
    #[serde(default)]
    pub refunded_payment: Option<RefundedPayment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: BotUser,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub data: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreCheckoutQuery {
    pub id: String,
    pub from: BotUser,
    pub currency: String,
    pub total_amount: i64,
    pub invoice_payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
    #[serde(default)]
    pub pre_checkout_query: Option<PreCheckoutQuery>,
}

impl Update {
    pub fn sender(&self) -> Option<&BotUser> {
        if let Some(message) = &self.message {
            return message.from.as_ref();
        }
        if let Some(callback) = &self.callback_query {
            return Some(&callback.from);
        }
        self.pre_checkout_query.as_ref().map(|q| &q.from)
    }

    pub fn chat_key(&self) -> i64 {
        if let Some(message) = &self.message {
            return message.chat.id;
        }
        if let Some(message) = self
            .callback_query
            .as_ref()
            .and_then(|c| c.message.as_ref())
        {
            return message.chat.id;
        }
        self.sender().map_or(0, |u| u.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Button {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Button {
    pub fn callback(text: impl Into<String>, data: impl Into<String>) -> Self {
        Button {
            text: text.into(),
            callback_data: Some(data.into()),
            url: None,
        }
    }

    pub fn url(text: impl Into<String>, url: impl Into<String>) -> Self {
        Button {
            text: text.into(),
            callback_data: None,
            url: Some(url.into()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Keyboard {
    pub inline_keyboard: Vec<Vec<Button>>,
}

impl Keyboard {
    pub fn new() -> Self {
        Keyboard::default()
    }

    pub fn row(mut self, buttons: Vec<Button>) -> Self {
        let buttons: Vec<Button> = buttons
            .into_iter()
            .filter(|b| {
                b.callback_data
                    .as_ref()
                    .is_none_or(|d| !d.is_empty() && d.len() <= CALLBACK_DATA_MAX)
            })
            .collect();
        if !buttons.is_empty() {
            self.inline_keyboard.push(buttons);
        }
        self
    }

    pub fn button(self, text: impl Into<String>, data: impl Into<String>) -> Self {
        self.row(vec![Button::callback(text, data)])
    }

    pub fn link(self, text: impl Into<String>, url: impl Into<String>) -> Self {
        self.row(vec![Button::url(text, url)])
    }

    pub fn is_empty(&self) -> bool {
        self.inline_keyboard.is_empty()
    }
}

#[derive(Deserialize)]
struct ResponseParameters {
    #[serde(default)]
    retry_after: Option<u64>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
    #[serde(default)]
    error_code: i64,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parameters: Option<ResponseParameters>,
}

pub struct TelegramApi {
    http: reqwest::Client,
    base: String,
}

impl TelegramApi {
    pub fn new(api_base: &str, token: &str) -> Self {
        TelegramApi {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(65))
                .connect_timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            base: format!("{}/bot{}", api_base.trim_end_matches('/'), token),
        }
    }

    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &serde_json::Value,
    ) -> Result<T, TelegramError> {
        let response = self
            .http
            .post(format!("{}/{method}", self.base))
            .json(body)
            .send()
            .await
            .map_err(|e| TelegramError::Transport(e.without_url().to_string()))?;
        let status = response.status();
        let envelope: Envelope<T> = response.json().await.map_err(|e| {
            if status.is_server_error() {
                TelegramError::Api {
                    code: i64::from(status.as_u16()),
                    description: status.to_string(),
                    retry_after: None,
                }
            } else {
                TelegramError::Decode(e.without_url().to_string())
            }
        })?;
        match (envelope.ok, envelope.result) {
            (true, Some(result)) => Ok(result),
            _ => Err(TelegramError::Api {
                code: if envelope.error_code == 0 {
                    i64::from(status.as_u16())
                } else {
                    envelope.error_code
                },
                description: envelope.description,
                retry_after: envelope.parameters.and_then(|p| p.retry_after),
            }),
        }
    }

    pub async fn get_me(&self) -> Result<BotUser, TelegramError> {
        self.call("getMe", &serde_json::json!({})).await
    }

    pub async fn delete_webhook(&self) -> Result<bool, TelegramError> {
        self.call(
            "deleteWebhook",
            &serde_json::json!({ "drop_pending_updates": false }),
        )
        .await
    }

    pub async fn set_commands(&self, commands: &[(&str, &str)]) -> Result<bool, TelegramError> {
        let list: Vec<serde_json::Value> = commands
            .iter()
            .map(|(command, description)| {
                serde_json::json!({ "command": command, "description": description })
            })
            .collect();
        self.call("setMyCommands", &serde_json::json!({ "commands": list }))
            .await
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout: Duration,
    ) -> Result<Vec<Update>, TelegramError> {
        let raw: Vec<serde_json::Value> = self
            .call(
                "getUpdates",
                &serde_json::json!({
                    "offset": offset,
                    "timeout": timeout.as_secs(),
                    "allowed_updates": ["message", "callback_query", "pre_checkout_query"],
                }),
            )
            .await?;
        Ok(raw
            .into_iter()
            .filter_map(|value| {
                let id = value.get("update_id").and_then(|v| v.as_i64());
                match serde_json::from_value::<Update>(value) {
                    Ok(update) => Some(update),
                    Err(err) => {
                        tracing::warn!(update_id = ?id, error = %err, "апдейт Telegram не разобран");
                        id.map(|update_id| Update {
                            update_id,
                            message: None,
                            callback_query: None,
                            pre_checkout_query: None,
                        })
                    }
                }
            })
            .collect())
    }

    fn message_body(
        chat_id: i64,
        text: &str,
        keyboard: Option<&Keyboard>,
        html: bool,
    ) -> serde_json::Value {
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": truncate(text, MESSAGE_MAX),
            "link_preview_options": { "is_disabled": true },
        });
        if html {
            body["parse_mode"] = serde_json::json!("HTML");
        }
        if let Some(keyboard) = keyboard.filter(|k| !k.is_empty()) {
            body["reply_markup"] = serde_json::json!(keyboard);
        }
        body
    }

    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<Message, TelegramError> {
        match self
            .call(
                "sendMessage",
                &Self::message_body(chat_id, text, keyboard, true),
            )
            .await
        {
            Err(err) if err.is_parse_error() => {
                self.call(
                    "sendMessage",
                    &Self::message_body(chat_id, &strip_tags(text), keyboard, false),
                )
                .await
            }
            other => other,
        }
    }

    pub async fn edit_message(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<(), TelegramError> {
        let mut body = Self::message_body(chat_id, text, keyboard, true);
        body["message_id"] = serde_json::json!(message_id);
        match self
            .call::<serde_json::Value>("editMessageText", &body)
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.is_not_modified() => Ok(()),
            Err(err) => Err(err),
        }
    }

    pub async fn answer_callback(
        &self,
        callback_id: &str,
        text: Option<&str>,
    ) -> Result<(), TelegramError> {
        let mut body = serde_json::json!({ "callback_query_id": callback_id });
        if let Some(text) = text {
            body["text"] = serde_json::json!(truncate(text, 200));
        }
        self.call::<bool>("answerCallbackQuery", &body)
            .await
            .map(|_| ())
    }

    pub async fn answer_pre_checkout(
        &self,
        query_id: &str,
        error: Option<&str>,
    ) -> Result<(), TelegramError> {
        let body = match error {
            None => serde_json::json!({ "pre_checkout_query_id": query_id, "ok": true }),
            Some(message) => serde_json::json!({
                "pre_checkout_query_id": query_id,
                "ok": false,
                "error_message": truncate(message, 200),
            }),
        };
        self.call::<bool>("answerPreCheckoutQuery", &body)
            .await
            .map(|_| ())
    }
}

pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

pub fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "123456:SECRET-token";

    #[tokio::test]
    async fn ошибка_сети_не_раскрывает_токен() {
        let api = TelegramApi::new("http://127.0.0.1:9", TOKEN);
        let err = api.get_me().await.unwrap_err();
        assert!(matches!(err, TelegramError::Transport(_)));
        assert!(!err.to_string().contains("SECRET"));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn отказ_телеграма_разбирается_с_паузой() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/sendMessage")))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "ok": false, "error_code": 429, "description": "Too Many Requests: retry after 7",
                "parameters": {"retry_after": 7}
            })))
            .mount(&server)
            .await;
        let api = TelegramApi::new(&server.uri(), TOKEN);
        let err = api.send_message(1, "текст", None).await.unwrap_err();
        assert_eq!(err.retry_after(), Some(7));
        assert!(err.is_retryable());
        assert!(!err.is_forbidden());
    }

    #[tokio::test]
    async fn неразобранная_разметка_отправляется_простым_текстом() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/sendMessage")))
            .and(body_partial_json(serde_json::json!({"parse_mode": "HTML"})))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "ok": false, "error_code": 400, "description": "Bad Request: can't parse entities"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/sendMessage")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"message_id": 5, "chat": {"id": 1, "type": "private"}, "date": 0}
            })))
            .mount(&server)
            .await;
        let api = TelegramApi::new(&server.uri(), TOKEN);
        let sent = api
            .send_message(1, "<b>Привет</b> &amp; пока", None)
            .await
            .unwrap();
        assert_eq!(sent.message_id, 5);
        let requests = server.received_requests().await.unwrap();
        let plain: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(plain["text"], "Привет & пока");
        assert!(plain.get("parse_mode").is_none());
    }

    #[test]
    fn апдейты_разбираются_в_типы() {
        let update: Update = serde_json::from_value(serde_json::json!({
            "update_id": 10,
            "callback_query": {
                "id": "cb1",
                "from": {"id": 42, "is_bot": false, "first_name": "Алексей", "username": "morozov_a"},
                "message": {"message_id": 7, "chat": {"id": 42, "type": "private"}, "date": 1},
                "data": "m"
            }
        }))
        .unwrap();
        assert_eq!(update.chat_key(), 42);
        assert_eq!(update.sender().map(|u| u.id), Some(42));
        let pre: Update = serde_json::from_value(serde_json::json!({
            "update_id": 11,
            "pre_checkout_query": {
                "id": "q1", "from": {"id": 43, "first_name": "Ксения"}, "currency": "XTR",
                "total_amount": 350, "invoice_payload": "00000000-0000-0000-0000-000000000000"
            }
        }))
        .unwrap();
        assert_eq!(pre.chat_key(), 43);
    }

    #[test]
    fn кнопка_с_длинными_данными_не_попадает_в_клавиатуру() {
        let keyboard = Keyboard::new()
            .button("Ок", "m")
            .button("Длинно", "x".repeat(65))
            .link("Оплатить", "https://example.com/pay");
        assert_eq!(keyboard.inline_keyboard.len(), 2);
        assert_eq!(truncate("абвгд", 3), "аб…");
    }
}
