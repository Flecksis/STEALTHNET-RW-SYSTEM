use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;

pub const DEFAULT_URL: &str = "https://api.groq.com/openai/v1";
pub const MODELS: [&str; 3] = [
    "openai/gpt-oss-120b",
    "openai/gpt-oss-20b",
    "qwen/qwen3.8-27b",
];
const TIMEOUT: Duration = Duration::from_secs(25);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DETAIL_MAX: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatMessage {
    pub role: &'static str,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "system",
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "user",
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "assistant",
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatOptions {
    pub max_tokens: u32,
    pub json: bool,
    pub reasoning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub content: String,
    pub finish_reason: Option<String>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GroqError {
    #[error("ключ Groq не подходит: проверьте его в console.groq.com → API Keys")]
    Unauthorized,
    #[error(
        "Groq отказал в доступе (403): {0}. Так бывает, если сервер панели в стране, где Groq недоступен: укажите в настройках модуля адрес своего прокси к API"
    )]
    Forbidden(String),
    #[error("модель недоступна в Groq: {0}. Выберите другую модель в настройках модуля")]
    Model(String),
    #[error("Groq ограничил частоту запросов")]
    RateLimited { retry_after: Option<u64> },
    #[error("Groq не принял ответ модели как JSON")]
    JsonFailed,
    #[error("модель не поддерживает параметры рассуждения: {0}")]
    Unsupported(String),
    #[error("Groq отклонил запрос: {0}")]
    Rejected(String),
    #[error("Groq временно недоступен (код {0})")]
    Unavailable(u16),
    #[error("Groq не ответил: {0}")]
    Network(String),
    #[error("Groq прислал непонятный ответ: {0}")]
    Malformed(String),
}

impl GroqError {
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            GroqError::RateLimited { .. } | GroqError::Unavailable(_) | GroqError::Network(_)
        )
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<ResponseMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct ModelList {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

fn cut(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= DETAIL_MAX {
        return flat;
    }
    let mut out: String = flat.chars().take(DETAIL_MAX).collect();
    out.push('…');
    out
}

fn tokens(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn status_error(status: u16, body: &str, retry_after: Option<u64>) -> GroqError {
    let detail = serde_json::from_str::<ErrorBody>(body)
        .ok()
        .and_then(|b| b.error);
    let message = detail
        .as_ref()
        .and_then(|d| d.message.clone())
        .map(|m| cut(&m))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| cut(body));
    let code = detail
        .as_ref()
        .and_then(|d| d.code.as_ref())
        .and_then(|c| c.as_str().map(str::to_string))
        .unwrap_or_default();
    let lowered = message.to_lowercase();
    match status {
        401 => GroqError::Unauthorized,
        403 => GroqError::Forbidden(message),
        404 => GroqError::Model(message),
        429 => GroqError::RateLimited { retry_after },
        400 if code == "json_validate_failed" => GroqError::JsonFailed,
        400 | 422 if code.starts_with("model_") || lowered.contains("decommissioned") => {
            GroqError::Model(message)
        }
        400 | 422 if lowered.contains("reasoning") => GroqError::Unsupported(message),
        413 => GroqError::Rejected("слишком длинный запрос".into()),
        498 | 500..=599 => GroqError::Unavailable(status),
        _ => GroqError::Rejected(message),
    }
}

fn request_body(model: &str, messages: &[ChatMessage], options: ChatOptions) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_completion_tokens": options.max_tokens,
        "temperature": 0.2,
    });
    if options.json {
        body["response_format"] = serde_json::json!({ "type": "json_object" });
    }
    if options.reasoning {
        if model.starts_with("openai/gpt-oss") {
            body["reasoning_effort"] = serde_json::json!("low");
            body["include_reasoning"] = serde_json::json!(false);
        } else if model.starts_with("qwen/") {
            body["reasoning_format"] = serde_json::json!("hidden");
        }
    }
    body
}

fn http() -> &'static reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

#[derive(Clone)]
pub struct Groq {
    http: &'static reqwest::Client,
    base: String,
    key: String,
}

impl Groq {
    pub fn new(base: &str, key: &str) -> Self {
        Groq {
            http: http(),
            base: base.trim().trim_end_matches('/').to_string(),
            key: key.trim().to_string(),
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<String, GroqError> {
        let response = request
            .bearer_auth(&self.key)
            .send()
            .await
            .map_err(|e| GroqError::Network(cut(&e.without_url().to_string())))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|s| s.ceil().max(0.0) as u64);
        let body = response
            .text()
            .await
            .map_err(|e| GroqError::Network(cut(&e.without_url().to_string())))?;
        if (200..300).contains(&status) {
            Ok(body)
        } else {
            Err(status_error(status, &body, retry_after))
        }
    }

    pub async fn chat(
        &self,
        model: &str,
        messages: &[ChatMessage],
        options: ChatOptions,
    ) -> Result<Completion, GroqError> {
        let body = request_body(model, messages, options);
        let text = self
            .send(
                self.http
                    .post(format!("{}/chat/completions", self.base))
                    .json(&body),
            )
            .await?;
        let parsed: ChatResponse =
            serde_json::from_str(&text).map_err(|e| GroqError::Malformed(cut(&e.to_string())))?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| GroqError::Malformed("в ответе нет вариантов".into()))?;
        let usage = parsed.usage.unwrap_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
        });
        Ok(Completion {
            content: choice.message.and_then(|m| m.content).unwrap_or_default(),
            finish_reason: choice.finish_reason,
            prompt_tokens: tokens(usage.prompt_tokens),
            completion_tokens: tokens(usage.completion_tokens),
        })
    }

    pub async fn models(&self) -> Result<Vec<String>, GroqError> {
        let text = self
            .send(self.http.get(format!("{}/models", self.base)))
            .await?;
        let parsed: ModelList =
            serde_json::from_str(&text).map_err(|e| GroqError::Malformed(cut(&e.to_string())))?;
        Ok(parsed.data.into_iter().map(|m| m.id).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ошибки_groq_понятны_оператору() {
        assert_eq!(status_error(401, "{}", None), GroqError::Unauthorized);
        assert!(matches!(
            status_error(403, "{\"error\":{\"message\":\"Forbidden\"}}", None),
            GroqError::Forbidden(m) if m == "Forbidden"
        ));
        assert_eq!(
            status_error(429, "{}", Some(3)),
            GroqError::RateLimited {
                retry_after: Some(3)
            }
        );
        assert_eq!(
            status_error(
                400,
                "{\"error\":{\"message\":\"Failed to generate JSON\",\"code\":\"json_validate_failed\"}}",
                None
            ),
            GroqError::JsonFailed
        );
        assert!(matches!(
            status_error(
                400,
                "{\"error\":{\"message\":\"The model `x` has been decommissioned\",\"code\":\"model_decommissioned\"}}",
                None
            ),
            GroqError::Model(_)
        ));
        assert!(matches!(
            status_error(
                400,
                "{\"error\":{\"message\":\"reasoning_effort is not supported\"}}",
                None
            ),
            GroqError::Unsupported(_)
        ));
        assert_eq!(status_error(503, "", None), GroqError::Unavailable(503));
        assert!(status_error(503, "", None).retryable());
        assert!(!GroqError::Unauthorized.retryable());
    }

    #[test]
    fn параметры_рассуждения_зависят_от_модели() {
        let messages = [ChatMessage::user("привет")];
        let options = ChatOptions {
            max_tokens: 500,
            json: true,
            reasoning: true,
        };
        let oss = request_body("openai/gpt-oss-120b", &messages, options);
        assert_eq!(oss["reasoning_effort"], "low");
        assert_eq!(oss["include_reasoning"], false);
        assert_eq!(oss["response_format"]["type"], "json_object");
        assert_eq!(oss["max_completion_tokens"], 500);
        let qwen = request_body("qwen/qwen3.8-27b", &messages, options);
        assert_eq!(qwen["reasoning_format"], "hidden");
        assert!(qwen.get("reasoning_effort").is_none());
        let plain = request_body(
            "openai/gpt-oss-20b",
            &messages,
            ChatOptions {
                max_tokens: 500,
                json: false,
                reasoning: false,
            },
        );
        assert!(plain.get("response_format").is_none());
        assert!(plain.get("reasoning_effort").is_none());
    }
}
