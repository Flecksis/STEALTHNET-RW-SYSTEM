use serde::Deserialize;

pub type RemnaResult<T> = Result<T, RemnaError>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemnaError {
    #[error("Remnawave не настроена: {0}")]
    NotConfigured(String),
    #[error("в Remnawave не найдено: {message}")]
    NotFound {
        code: Option<String>,
        message: String,
    },
    #[error("Remnawave отклонила запрос: {message}")]
    BadRequest {
        code: Option<String>,
        message: String,
    },
    #[error("токен Remnawave не принят (HTTP {0})")]
    Unauthorized(u16),
    #[error("Remnawave ограничила частоту запросов")]
    RateLimited,
    #[error("ошибка Remnawave HTTP {status}: {message}")]
    Server { status: u16, message: String },
    #[error("Remnawave не ответила вовремя")]
    Timeout,
    #[error("Remnawave недоступна: {0}")]
    Unavailable(String),
    #[error("не удалось разобрать ответ Remnawave: {0}")]
    Decode(String),
    #[error("метод Remnawave пока не поддержан: {0}")]
    Unsupported(&'static str),
}

impl RemnaError {
    pub const fn is_not_found(&self) -> bool {
        matches!(self, RemnaError::NotFound { .. })
    }

    pub const fn outcome_unknown(&self) -> bool {
        matches!(
            self,
            RemnaError::Timeout | RemnaError::Server { .. } | RemnaError::Unavailable(_)
        )
    }

    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            RemnaError::Timeout
                | RemnaError::Server { .. }
                | RemnaError::Unavailable(_)
                | RemnaError::RateLimited
        )
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            RemnaError::NotFound { code, .. } | RemnaError::BadRequest { code, .. } => {
                code.as_deref()
            }
            _ => None,
        }
    }

    pub fn from_status(status: u16, body: &str) -> Self {
        let parsed: Option<ErrorBody> = serde_json::from_str(body).ok();
        let code = parsed.as_ref().and_then(|b| b.error_code.clone());
        let message = parsed
            .and_then(|b| b.message)
            .unwrap_or_else(|| body.chars().take(300).collect());
        match status {
            404 => RemnaError::NotFound { code, message },
            400 | 409 | 422 => RemnaError::BadRequest { code, message },
            401 | 403 => RemnaError::Unauthorized(status),
            429 => RemnaError::RateLimited,
            _ => RemnaError::Server { status, message },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    message: Option<String>,
    error_code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn код_ошибки_remnawave_сохраняется() {
        let err = RemnaError::from_status(
            400,
            r#"{"timestamp":"2026-11-16T00:00:00Z","path":"/api/users","message":"LIMITED and EXPIRED statuses are not allowed to be set manually.","errorCode":"A089"}"#,
        );
        assert_eq!(err.code(), Some("A089"));
        assert!(!err.outcome_unknown());
        let missing =
            RemnaError::from_status(404, r#"{"message":"User not found","errorCode":"A063"}"#);
        assert!(missing.is_not_found());
        assert!(RemnaError::from_status(502, "bad gateway").outcome_unknown());
        assert_eq!(
            RemnaError::from_status(401, ""),
            RemnaError::Unauthorized(401)
        );
    }
}
