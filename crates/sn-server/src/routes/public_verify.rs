use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, Meta};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::client::Locale;
use sn_domain::settings::keys;
use sn_modules::catalog::antibot;
use sn_modules::catalog::antibot::captcha::Widget;
use sn_services::ServiceError;
use sn_store::Db;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/public/verify", post(view))
        .route("/api/public/verify/pass", post(pass))
}

#[derive(Deserialize)]
struct Opened {
    token: String,
}

#[derive(Deserialize)]
struct Passed {
    token: String,
    captcha: String,
}

#[derive(Debug, Serialize)]
struct Back {
    bot: Option<String>,
}

#[derive(Debug, Serialize)]
struct VerifyView {
    purpose: String,
    expires_at: DateTime<Utc>,
    captcha: Widget,
    back: Back,
    locale: Locale,
}

fn expired() -> ApiError {
    ServiceError::rejected(
        antibot::EXPIRED,
        "Ссылка на проверку устарела. Вернитесь в бот и запросите пробный период ещё раз",
    )
    .into()
}

async fn opened(
    state: &AppState,
    meta: &sn_services::auth::RequestMeta,
    token: &str,
) -> ApiResult<antibot::checks::PageCheck> {
    let limits = state.services.limits();
    limits
        .hit_ip(limits.verify_page(), meta.ip.as_deref())
        .await?;
    let ctx = state.services.module_context();
    antibot::checks::page_check(&ctx, token)
        .await
        .map_err(ServiceError::from)?
        .ok_or_else(expired)
}

async fn bot_link(state: &AppState) -> Option<String> {
    let mut conn = state.services.store().conn().await.ok()?;
    let name: serde_json::Value = conn.settings().get(keys::BOT_USERNAME).await.ok()??;
    let name = name.as_str()?.trim().trim_start_matches('@').to_string();
    (!name.is_empty()).then(|| format!("https://t.me/{name}"))
}

async fn view(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<Opened>,
) -> ApiResult<Json<VerifyView>> {
    let check = opened(&state, &meta, &body.token).await?;
    let locale = match state.services.store().conn().await {
        Ok(mut conn) => conn
            .clients()
            .get(check.client_id)
            .await
            .map(|client| client.locale)
            .unwrap_or_default(),
        Err(_) => Locale::default(),
    };
    Ok(Json(VerifyView {
        purpose: check.target,
        expires_at: check.expires_at,
        captcha: check.widget,
        back: Back {
            bot: bot_link(&state).await,
        },
        locale,
    }))
}

async fn pass(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<Passed>,
) -> ApiResult<Json<serde_json::Value>> {
    let check = opened(&state, &meta, &body.token).await?;
    if check.target != antibot::TRIAL {
        return Err(expired());
    }
    let answer = antibot::Answer::web(None, Some(&body.captcha), meta.ip.as_deref());
    let order_id = state
        .services
        .checkout()
        .confirm_trial(check.client_id, &body.token, answer, check.variant)
        .await?;
    Ok(Json(serde_json::json!({
        "done": true,
        "purpose": antibot::TRIAL,
        "order_id": order_id,
    })))
}
