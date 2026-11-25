use serde_json::Value;
use std::time::Duration;

pub const PART_SIZE: u64 = 45 * 1024 * 1024;

const FILE_TIMEOUT: Duration = Duration::from_secs(600);
const FILE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const QUICK_TIMEOUT: Duration = Duration::from_secs(12);
const QUICK_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TgError {
    Api {
        code: u16,
        description: String,
        retry_after: Option<u64>,
        migrate_to: Option<i64>,
    },
    Transport(String),
}

impl TgError {
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            TgError::Api { retry_after, .. } => *retry_after,
            TgError::Transport(_) => None,
        }
    }

    pub fn thread_missing(&self) -> bool {
        matches!(self, TgError::Api { code: 400, description, .. }
            if description.to_ascii_lowercase().contains("thread not found")
                || description.to_ascii_lowercase().contains("topic_deleted")
                || description.to_ascii_lowercase().contains("topic_closed"))
    }

    pub fn readable(&self) -> String {
        match self {
            TgError::Transport(_) => "нет связи с Telegram".into(),
            TgError::Api { code: 401 | 404, .. } => "Telegram не принял токен бота".into(),
            TgError::Api { code: 403, .. } => {
                "бот не может писать в этот чат: добавьте его в чат и разрешите отправлять сообщения".into()
            }
            TgError::Api { code: 413, .. } => "файл слишком большой для Telegram".into(),
            TgError::Api {
                code: 400,
                description,
                ..
            } if description.to_ascii_lowercase().contains("chat not found") => {
                "чат не найден: проверьте номер чата и что бот в нём есть".into()
            }
            TgError::Api {
                migrate_to: Some(_),
                ..
            } => "группа стала супергруппой: укажите новый номер чата".into(),
            TgError::Api { code, .. } if *code == 429 => "Telegram просит подождать".into(),
            TgError::Api { code, .. } if *code >= 500 => format!("Telegram не ответил: ошибка {code}"),
            TgError::Api { description, .. } => format!("Telegram ответил: {description}"),
        }
    }
}

pub struct Bot {
    http: reqwest::Client,
    base: String,
}

impl Bot {
    pub fn new(
        api_base: &str,
        token: &str,
        proxy: Option<&sn_egress::Proxy>,
    ) -> Result<Self, String> {
        Self::with_timeouts(api_base, token, proxy, FILE_CONNECT_TIMEOUT, FILE_TIMEOUT)
    }

    pub fn quick(
        api_base: &str,
        token: &str,
        proxy: Option<&sn_egress::Proxy>,
    ) -> Result<Self, String> {
        Self::with_timeouts(api_base, token, proxy, QUICK_CONNECT_TIMEOUT, QUICK_TIMEOUT)
    }

    fn with_timeouts(
        api_base: &str,
        token: &str,
        proxy: Option<&sn_egress::Proxy>,
        connect: Duration,
        total: Duration,
    ) -> Result<Self, String> {
        let http = sn_egress::routed(
            reqwest::Client::builder()
                .connect_timeout(connect)
                .timeout(total)
                .redirect(reqwest::redirect::Policy::none()),
            proxy,
        )
        .build()
        .map_err(|e| e.without_url().to_string())?;
        Ok(Bot {
            http,
            base: format!("{}/bot{}", api_base.trim_end_matches('/'), token),
        })
    }

    async fn parse(response: reqwest::Response) -> Result<Value, TgError> {
        let status = response.status().as_u16();
        let body: Value = response.json().await.map_err(|_| TgError::Api {
            code: status,
            description: "ответ не JSON".into(),
            retry_after: None,
            migrate_to: None,
        })?;
        if body.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(body.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(TgError::Api {
            code: body
                .get("error_code")
                .and_then(Value::as_u64)
                .and_then(|c| u16::try_from(c).ok())
                .unwrap_or(status),
            description: body
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            retry_after: body
                .pointer("/parameters/retry_after")
                .and_then(Value::as_u64),
            migrate_to: body
                .pointer("/parameters/migrate_to_chat_id")
                .and_then(Value::as_i64),
        })
    }

    async fn call(&self, method: &str, body: &Value) -> Result<Value, TgError> {
        let response = self
            .http
            .post(format!("{}/{method}", self.base))
            .json(body)
            .send()
            .await
            .map_err(|e| TgError::Transport(e.without_url().to_string()))?;
        Self::parse(response).await
    }

    pub async fn me(&self) -> Result<Value, TgError> {
        self.call("getMe", &serde_json::json!({})).await
    }

    pub async fn menu_button(&self) -> Result<Value, TgError> {
        self.call("getChatMenuButton", &serde_json::json!({})).await
    }

    pub async fn set_menu_button(&self, button: &Value) -> Result<(), TgError> {
        self.call(
            "setChatMenuButton",
            &serde_json::json!({ "menu_button": button }),
        )
        .await
        .map(|_| ())
    }

    pub async fn send_message(
        &self,
        chat_id: i64,
        thread: Option<i64>,
        html: &str,
    ) -> Result<(), TgError> {
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": html,
            "parse_mode": "HTML",
            "link_preview_options": { "is_disabled": true },
        });
        if let Some(thread) = thread {
            body["message_thread_id"] = thread.into();
        }
        self.call("sendMessage", &body).await.map(|_| ())
    }

    pub async fn create_topic(&self, chat_id: i64, name: &str) -> Result<i64, TgError> {
        let result = self
            .call(
                "createForumTopic",
                &serde_json::json!({ "chat_id": chat_id, "name": name }),
            )
            .await?;
        result
            .get("message_thread_id")
            .and_then(Value::as_i64)
            .ok_or(TgError::Api {
                code: 200,
                description: "тема не создана".into(),
                retry_after: None,
                migrate_to: None,
            })
    }

    pub async fn send_document(
        &self,
        chat_id: i64,
        thread: Option<i64>,
        name: &str,
        bytes: Vec<u8>,
        caption: &str,
    ) -> Result<(), TgError> {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(name.to_string())
            .mime_str("application/octet-stream")
            .map_err(|e| TgError::Transport(e.to_string()))?;
        let mut form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .text("caption", caption.to_string())
            .text("parse_mode", "HTML")
            .text("disable_content_type_detection", "true")
            .part("document", part);
        if let Some(thread) = thread {
            form = form.text("message_thread_id", thread.to_string());
        }
        let response = self
            .http
            .post(format!("{}/sendDocument", self.base))
            .multipart(form)
            .send()
            .await
            .map_err(|e| TgError::Transport(e.without_url().to_string()))?;
        Self::parse(response).await.map(|_| ())
    }
}

pub fn parts(size: u64, part: u64) -> u64 {
    size.div_ceil(part.max(1)).max(1)
}

pub fn part_name(file: &str, index: u64, total: u64) -> String {
    if total <= 1 {
        file.to_string()
    } else {
        format!("{file}.part{:03}", index + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn копия_в_telegram_не_уходит_по_редиректу() {
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let elsewhere = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": true, "result": {}})),
            )
            .mount(&elsewhere)
            .await;
        let api = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/elsewhere", elsewhere.uri())),
            )
            .mount(&api)
            .await;
        let bot = Bot::new(&api.uri(), "123:abc", None).unwrap();
        assert!(bot.send_message(-100, None, "проверка").await.is_err());
        assert!(elsewhere.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn большая_копия_делится_на_части_с_именами_по_порядку() {
        assert_eq!(parts(0, PART_SIZE), 1);
        assert_eq!(parts(PART_SIZE, PART_SIZE), 1);
        assert_eq!(parts(PART_SIZE + 1, PART_SIZE), 2);
        assert_eq!(part_name("a.tar.gz", 0, 1), "a.tar.gz");
        assert_eq!(part_name("a.tar.gz", 0, 12), "a.tar.gz.part001");
        assert_eq!(part_name("a.tar.gz", 11, 12), "a.tar.gz.part012");
    }

    #[test]
    fn ошибки_telegram_понятны_оператору() {
        let gone = TgError::Api {
            code: 400,
            description: "Bad Request: chat not found".into(),
            retry_after: None,
            migrate_to: None,
        };
        assert!(gone.readable().contains("чат не найден"));
        let thread = TgError::Api {
            code: 400,
            description: "Bad Request: message thread not found".into(),
            retry_after: None,
            migrate_to: None,
        };
        assert!(thread.thread_missing());
        assert!(
            TgError::Transport("x".into())
                .readable()
                .contains("нет связи")
        );
    }
}
