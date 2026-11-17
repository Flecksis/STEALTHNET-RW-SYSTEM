use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/version", get(version))
}

async fn health(State(state): State<AppState>) -> Response {
    match state.services.store().ping().await {
        Ok(()) => Json(serde_json::json!({ "status": "ok" })).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "база недоступна");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "db_unavailable" })),
            )
                .into_response()
        }
    }
}

async fn version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "name": "stealthnet",
        "version": env!("CARGO_PKG_VERSION"),
        "remnawave": sn_remnawave::SUPPORTED_VERSION,
    }))
}
