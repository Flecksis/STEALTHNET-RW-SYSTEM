use crate::error::ModuleError;
use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use sn_domain::access::{AdminPrincipal, ClientPrincipal, Permission};
use sn_store::StoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl WebError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        WebError {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn rejected(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "auth.forbidden", "недостаточно прав")
    }
}

impl IntoResponse for WebError {
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

impl From<ModuleError> for WebError {
    fn from(err: ModuleError) -> Self {
        match err {
            ModuleError::NotFound(what) | ModuleError::Store(StoreError::NotFound(what)) => {
                WebError::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    format!("не найдено: {what}"),
                )
            }
            ModuleError::Invalid(message) => {
                WebError::rejected("validation.invalid_value", message)
            }
            ModuleError::Store(StoreError::Invalid(message)) => {
                WebError::rejected("validation.invalid_value", message)
            }
            ModuleError::Store(StoreError::Conflict { message, .. }) => {
                WebError::new(StatusCode::CONFLICT, "conflict", message)
            }
            ModuleError::Store(StoreError::InsufficientFunds) => WebError::new(
                StatusCode::CONFLICT,
                "balance.insufficient",
                "на балансе не хватает средств",
            ),
            ModuleError::Disabled(module) => WebError::new(
                StatusCode::NOT_FOUND,
                "module.disabled",
                format!("Модуль «{}» выключен", module.info().title),
            ),
            ModuleError::Core(message) => {
                WebError::new(StatusCode::CONFLICT, "core.rejected", message)
            }
            ModuleError::NotImplemented(what) => WebError::new(
                StatusCode::NOT_IMPLEMENTED,
                "not_implemented",
                format!("ещё не реализовано: {what}"),
            ),
            other => {
                tracing::error!(error = %other, "внутренняя ошибка модуля");
                WebError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "внутренняя ошибка",
                )
            }
        }
    }
}

impl From<StoreError> for WebError {
    fn from(err: StoreError) -> Self {
        ModuleError::Store(err).into()
    }
}

pub struct Admin(pub AdminPrincipal);

impl Admin {
    pub fn require(&self, permission: Permission) -> Result<(), WebError> {
        if self.0.can(permission) {
            Ok(())
        } else {
            Err(WebError::forbidden())
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Admin {
    type Rejection = WebError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AdminPrincipal>()
            .cloned()
            .map(Admin)
            .ok_or_else(|| {
                WebError::new(StatusCode::UNAUTHORIZED, "auth.required", "требуется вход")
            })
    }
}

pub struct Customer(pub ClientPrincipal);

impl Customer {
    pub fn require_full(&self) -> Result<(), WebError> {
        if self.0.full_access {
            Ok(())
        } else {
            Err(WebError::forbidden())
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Customer {
    type Rejection = WebError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<ClientPrincipal>()
            .cloned()
            .map(Customer)
            .ok_or_else(|| {
                WebError::new(StatusCode::UNAUTHORIZED, "auth.required", "требуется вход")
            })
    }
}
