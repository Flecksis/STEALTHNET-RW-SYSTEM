use crate::registry::ModuleRegistry;
use axum::Json;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sn_domain::module::ModuleId;
use std::sync::Arc;

#[derive(Clone)]
pub struct GateState {
    pub registry: Arc<ModuleRegistry>,
    pub module: ModuleId,
}

pub fn disabled_response(module: ModuleId) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "code": "module.disabled",
                "message": format!("Модуль «{}» выключен", module.info().title),
            }
        })),
    )
        .into_response()
}

pub async fn gate(State(state): State<GateState>, request: Request, next: Next) -> Response {
    if state.registry.is_enabled(state.module) {
        next.run(request).await
    } else {
        disabled_response(state.module)
    }
}
