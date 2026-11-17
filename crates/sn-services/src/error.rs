use sn_domain::DomainError;
use sn_modules::ModuleError;
use sn_payments::PaymentError;
use sn_remnawave::RemnaError;
use sn_store::StoreError;

pub type ServiceResult<T> = Result<T, ServiceError>;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("не найдено: {0}")]
    NotFound(&'static str),
    #[error("требуется вход")]
    Unauthorized,
    #[error("недостаточно прав")]
    Forbidden,
    #[error("{message}")]
    Rejected { code: &'static str, message: String },
    #[error("на балансе не хватает средств")]
    InsufficientFunds,
    #[error("Remnawave не подключена: {0}")]
    RemnaNotConfigured(String),
    #[error("не настроен ключ шифрования SN_SECRET_KEY")]
    NoSecretKey,
    #[error("сценарий ещё не реализован: {0}")]
    NotImplemented(&'static str),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Remna(#[from] RemnaError),
    #[error(transparent)]
    Payment(#[from] PaymentError),
    #[error(transparent)]
    Module(#[from] ModuleError),
    #[error("внутренняя ошибка: {0}")]
    Internal(String),
}

impl ServiceError {
    pub fn rejected(code: &'static str, message: impl Into<String>) -> Self {
        ServiceError::Rejected {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            ServiceError::NotFound(_) => "not_found",
            ServiceError::Unauthorized => "auth.required",
            ServiceError::Forbidden => "auth.forbidden",
            ServiceError::Rejected { code, .. } => code,
            ServiceError::InsufficientFunds => "balance.insufficient",
            ServiceError::RemnaNotConfigured(_) => "remnawave.not_configured",
            ServiceError::NoSecretKey => "config.no_secret_key",
            ServiceError::NotImplemented(_) => "not_implemented",
            ServiceError::Domain(e) => e.code(),
            ServiceError::Store(StoreError::NotFound(_)) => "not_found",
            ServiceError::Store(StoreError::Conflict { .. }) => "conflict",
            ServiceError::Store(StoreError::InsufficientFunds) => "balance.insufficient",
            ServiceError::Store(StoreError::Invalid(_)) => "validation.invalid_value",
            ServiceError::Store(StoreError::NotImplemented(_)) => "not_implemented",
            ServiceError::Store(_) => "internal",
            ServiceError::Remna(RemnaError::NotFound { .. }) => "remnawave.not_found",
            ServiceError::Remna(e) if e.outcome_unknown() => "remnawave.unavailable",
            ServiceError::Remna(_) => "remnawave.error",
            ServiceError::Payment(PaymentError::BadSignature) => "payment.bad_signature",
            ServiceError::Payment(PaymentError::NotImplemented(_)) => "not_implemented",
            ServiceError::Payment(PaymentError::EmailRequired) => "payment.email_required",
            ServiceError::Payment(_) => "payment.error",
            ServiceError::Module(ModuleError::Disabled(_)) => "module.disabled",
            ServiceError::Module(ModuleError::Unavailable { .. }) => "module.unavailable",
            ServiceError::Module(ModuleError::NotImplemented(_)) => "not_implemented",
            ServiceError::Module(ModuleError::Invalid(_)) => "module.invalid",
            ServiceError::Module(_) => "module.error",
            ServiceError::Internal(_) => "internal",
        }
    }

    pub fn is_not_implemented(&self) -> bool {
        matches!(
            self,
            ServiceError::NotImplemented(_)
                | ServiceError::Store(StoreError::NotImplemented(_))
                | ServiceError::Payment(PaymentError::NotImplemented(_))
                | ServiceError::Module(ModuleError::NotImplemented(_))
        )
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            ServiceError::Remna(e) => e.is_retryable(),
            ServiceError::Payment(e) => e.is_retryable(),
            ServiceError::Store(StoreError::Db(_)) => true,
            _ => false,
        }
    }
}

impl From<sqlx::Error> for ServiceError {
    fn from(err: sqlx::Error) -> Self {
        ServiceError::Store(StoreError::from(err))
    }
}
