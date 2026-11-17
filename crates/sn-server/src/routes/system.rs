use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use sn_store::Db;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/version", get(version))
        .route("/api/public/site", get(site))
}

async fn site(State(state): State<AppState>) -> Response {
    let settings = state.services.settings();
    let bot_username = match state.services.store().conn().await {
        Ok(mut conn) => conn
            .settings()
            .get(sn_domain::settings::keys::BOT_USERNAME)
            .await
            .ok()
            .flatten()
            .and_then(|value: serde_json::Value| value.as_str().map(str::to_string)),
        Err(_) => None,
    };
    Json(serde_json::json!({
        "brand_name": settings.brand_name,
        "currency": settings.currency,
        "support_url": settings.support_url,
        "bot_username": bot_username,
        "appearance": settings.appearance,
        "connect": settings.connect,
    }))
    .into_response()
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
