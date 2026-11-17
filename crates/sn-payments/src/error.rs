pub type PaymentResult<T> = Result<T, PaymentError>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PaymentError {
    #[error("платёжная система не настроена: {0}")]
    NotConfigured(String),
    #[error("платёжная система не поддерживает валюту {0}")]
    UnsupportedCurrency(String),
    #[error("платёжная система этого не умеет: {0}")]
    Unsupported(&'static str),
    #[error("подключение платёжной системы ещё не реализовано: {0}")]
    NotImplemented(&'static str),
    #[error("подпись уведомления не совпала")]
    BadSignature,
    #[error("уведомление не разобрано: {0}")]
    BadPayload(String),
    #[error("платёжная система отклонила запрос: {0}")]
    Rejected(String),
    #[error("платёжная система ответила ошибкой HTTP {status}")]
    Http { status: u16 },
    #[error("платёжная система недоступна: {0}")]
    Transport(String),
    #[error("для оплаты этим способом нужна почта покупателя")]
    EmailRequired,
}

impl PaymentError {
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            PaymentError::Transport(_) | PaymentError::Http { status: 500..=599 }
        )
    }

    pub const fn is_authentication(&self) -> bool {
        matches!(self, PaymentError::BadSignature)
    }

    pub fn transport(err: reqwest::Error) -> Self {
        PaymentError::Transport(err.without_url().to_string())
    }
}
