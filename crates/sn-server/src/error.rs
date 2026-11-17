use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sn_modules::ModuleError;
use sn_remnawave::RemnaError;
use sn_services::ServiceError;
use sn_services::auth::{TOTP_INVALID, TOTP_REQUIRED};
use sn_services::limits::RATE_LIMITED;
use sn_store::StoreError;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "auth.required", "требуется вход")
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "auth.forbidden", "недостаточно прав")
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "не найдено")
    }

    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn not_implemented(what: &str) -> Self {
        Self::new(
            StatusCode::NOT_IMPLEMENTED,
            "not_implemented",
            format!("ещё не реализовано: {what}"),
        )
    }
}

impl From<ServiceError> for ApiError {
    fn from(err: ServiceError) -> Self {
        let code = err.code();
        let status = match &err {
            err if err.is_not_implemented() => StatusCode::NOT_IMPLEMENTED,
            ServiceError::Rejected { code, .. } if *code == RATE_LIMITED => {
                StatusCode::TOO_MANY_REQUESTS
            }
            ServiceError::Rejected { code, .. }
                if *code == TOTP_REQUIRED || *code == TOTP_INVALID =>
            {
                StatusCode::UNAUTHORIZED
            }
            ServiceError::NotFound(_)
            | ServiceError::Store(StoreError::NotFound(_))
            | ServiceError::Remna(RemnaError::NotFound { .. }) => StatusCode::NOT_FOUND,
            ServiceError::Unauthorized => StatusCode::UNAUTHORIZED,
            ServiceError::Forbidden => StatusCode::FORBIDDEN,
            ServiceError::Rejected { .. }
            | ServiceError::Domain(_)
            | ServiceError::Store(StoreError::Invalid(_))
            | ServiceError::Payment(sn_payments::PaymentError::EmailRequired) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            ServiceError::InsufficientFunds
            | ServiceError::Store(StoreError::InsufficientFunds)
            | ServiceError::Store(StoreError::Conflict { .. }) => StatusCode::CONFLICT,
            ServiceError::RemnaNotConfigured(_) | ServiceError::NoSecretKey => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            ServiceError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            ServiceError::Remna(e) if e.outcome_unknown() => StatusCode::SERVICE_UNAVAILABLE,
            ServiceError::Remna(_) | ServiceError::Payment(_) => StatusCode::BAD_GATEWAY,
            ServiceError::Module(ModuleError::Disabled(_)) => StatusCode::NOT_FOUND,
            ServiceError::Module(ModuleError::Unavailable { .. })
            | ServiceError::Module(ModuleError::Invalid(_)) => StatusCode::UNPROCESSABLE_ENTITY,
            ServiceError::Store(_) | ServiceError::Module(_) | ServiceError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let message = match &err {
            _ if status == StatusCode::INTERNAL_SERVER_ERROR => {
                tracing::error!(error = %err, "внутренняя ошибка запроса");
                "внутренняя ошибка".to_string()
            }
            ServiceError::Module(ModuleError::Invalid(message)) => message.clone(),
            _ => err.to_string(),
        };
        ApiError {
            status,
            code,
            message,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": { "code": self.code, "message": self.message }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_payments::PaymentError;

    fn status(err: ServiceError) -> (StatusCode, &'static str) {
        let api = ApiError::from(err);
        (api.status, api.code)
    }

    #[test]
    fn ошибки_сценариев_переводятся_в_коды_http() {
        assert_eq!(
            status(ServiceError::Store(StoreError::NotImplemented("x"))),
            (StatusCode::NOT_IMPLEMENTED, "not_implemented")
        );
        assert_eq!(
            status(ServiceError::Payment(PaymentError::NotImplemented("x"))).0,
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            status(ServiceError::Payment(PaymentError::EmailRequired)),
            (StatusCode::UNPROCESSABLE_ENTITY, "payment.email_required")
        );
        assert_eq!(
            status(ServiceError::rejected(RATE_LIMITED, "подождите")),
            (StatusCode::TOO_MANY_REQUESTS, RATE_LIMITED)
        );
        assert_eq!(
            status(ServiceError::rejected(TOTP_REQUIRED, "код")).0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(ServiceError::Remna(RemnaError::NotFound {
                code: None,
                message: "Node".into()
            }))
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(ServiceError::Remna(RemnaError::Timeout)).0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        let internal = ApiError::from(ServiceError::Internal("секрет 4321:TOKEN".into()));
        assert_eq!(internal.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!internal.message.contains("TOKEN"));
        let refused = ApiError::from(ServiceError::Module(ModuleError::Invalid(
            "Промокод уже применён к неоплаченному счёту".into(),
        )));
        assert_eq!(
            (refused.status, refused.code),
            (StatusCode::UNPROCESSABLE_ENTITY, "module.invalid")
        );
        assert_eq!(
            refused.message,
            "Промокод уже применён к неоплаченному счёту"
        );
    }
}
