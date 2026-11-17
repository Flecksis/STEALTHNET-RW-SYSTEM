use crate::error::ApiError;
use crate::state::AppState;
use crate::web::escape_html;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use sn_services::branding::brand_initial;
use sn_store::Db;

const LOGO_CACHE: &str = "public, max-age=31536000, immutable";
const ICON_CACHE: &str = "public, max-age=300";
const IMAGE_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/version", get(version))
        .route("/api/public/site", get(site))
        .route("/api/public/catalog", get(catalog))
        .route("/api/public/logo/{file}", get(logo))
        .route("/api/public/icon.svg", get(icon))
}

async fn catalog(State(state): State<AppState>) -> Response {
    match state.services.checkout().public_catalog().await {
        Ok(offers) => Json(serde_json::json!({
            "currency": state.services.settings().currency,
            "tariffs": offers,
        }))
        .into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
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
        "title": settings.site.title.as_deref().unwrap_or(&settings.brand_name),
        "logo_url": settings.logo.as_ref().map(|logo| logo.path()),
        "currency": settings.currency,
        "support_url": settings.support_url,
        "bot_username": bot_username,
        "appearance": settings.appearance,
        "connect": settings.connect,
        "autorenew_days_before": settings.autorenew.charge_days_before,
        "trial": { "enabled": settings.trial.enabled, "days": settings.trial.days },
    }))
    .into_response()
}

async fn logo(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    let Some((tag, extension)) = file.split_once('.') else {
        return ApiError::not_found().into_response();
    };
    match state.services.branding().logo(tag).await {
        Ok(Some((kind, bytes))) if kind.extension() == extension => (
            [
                (header::CONTENT_TYPE, kind.content_type()),
                (header::CACHE_CONTROL, LOGO_CACHE),
                (header::CONTENT_SECURITY_POLICY, IMAGE_POLICY),
            ],
            bytes,
        )
            .into_response(),
        Ok(_) => ApiError::not_found().into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn icon(State(state): State<AppState>) -> Response {
    let letter = escape_html(&brand_initial(&state.services.settings().brand_name));
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\">\
         <rect width=\"64\" height=\"64\" rx=\"16\" fill=\"#0b0b0c\"/>\
         <text x=\"32\" y=\"44\" text-anchor=\"middle\" fill=\"#ffffff\" font-size=\"36\" font-weight=\"800\" \
         font-family=\"system-ui, -apple-system, 'Segoe UI', Roboto, Arial, sans-serif\">{letter}</text></svg>"
    );
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, ICON_CACHE),
            (header::CONTENT_SECURITY_POLICY, IMAGE_POLICY),
        ],
        svg,
    )
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
