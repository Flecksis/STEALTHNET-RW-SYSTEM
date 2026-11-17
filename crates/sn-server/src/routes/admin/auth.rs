use crate::cookies::{self, SameSite};
use crate::error::ApiResult;
use crate::extract::{ADMIN_COOKIE, ApiJson, CurrentAdmin, Meta};
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/password", post(change_password))
        .route("/auth/security", get(security))
        .route("/auth/2fa/setup", post(totp_setup))
        .route("/auth/2fa/enable", post(totp_enable))
        .route("/auth/2fa/disable", post(totp_disable))
        .route("/me", get(me))
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
    #[serde(default)]
    totp: Option<String>,
}

async fn login(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<LoginRequest>,
) -> ApiResult<Response> {
    let login = state
        .services
        .auth()
        .admin_login(&body.email, &body.password, body.totp.as_deref(), &meta)
        .await?;
    let cookie = cookies::session(
        &state,
        ADMIN_COOKIE,
        &login.token,
        Some(login.expires_at),
        SameSite::Strict,
    );
    Ok(cookies::with_cookie(cookie, Json(login)))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(token) = crate::extract::token(&headers, ADMIN_COOKIE) {
        state.services.auth().admin_logout(&token).await?;
    }
    let cookie = cookies::session(&state, ADMIN_COOKIE, "", None, SameSite::Strict);
    Ok(cookies::with_cookie(
        cookie,
        Json(serde_json::json!({ "ok": true })),
    ))
}

#[derive(Deserialize)]
struct PasswordRequest {
    current: String,
    new: String,
}

async fn change_password(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<PasswordRequest>,
) -> ApiResult<Response> {
    let login = state
        .services
        .auth()
        .change_admin_password(admin.0.admin_id, &body.current, &body.new, &meta)
        .await?;
    let cookie = cookies::session(
        &state,
        ADMIN_COOKIE,
        &login.token,
        Some(login.expires_at),
        SameSite::Strict,
    );
    Ok(cookies::with_cookie(cookie, Json(login)))
}

async fn security(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_services::auth::AdminSecurity>> {
    Ok(Json(
        state
            .services
            .auth()
            .admin_security(admin.0.admin_id)
            .await?,
    ))
}

async fn totp_setup(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_services::auth::TotpSetup>> {
    Ok(Json(
        state
            .services
            .auth()
            .admin_totp_setup(admin.0.admin_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct TotpEnableRequest {
    secret: String,
    code: String,
    password: String,
}

async fn totp_enable(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TotpEnableRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .services
        .auth()
        .admin_totp_enable(admin.0.admin_id, &body.secret, &body.code, &body.password)
        .await?;
    Ok(Json(serde_json::json!({ "totp_enabled": true })))
}

#[derive(Deserialize)]
struct TotpDisableRequest {
    password: String,
    code: String,
}

async fn totp_disable(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TotpDisableRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .services
        .auth()
        .admin_totp_disable(admin.0.admin_id, &body.password, &body.code)
        .await?;
    Ok(Json(serde_json::json!({ "totp_enabled": false })))
}

async fn me(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let permissions: Vec<Permission> = Permission::ALL
        .into_iter()
        .filter(|p| admin.can(*p))
        .collect();
    let settings = state.services.settings();
    let email = state
        .services
        .auth()
        .admin_security(admin.0.admin_id)
        .await
        .map(|security| security.email)
        .ok();
    Ok(Json(serde_json::json!({
        "admin": admin.0,
        "email": email,
        "permissions": permissions,
        "menu": state.services.modules().menu(),
        "currency": settings.currency,
        "brand_name": settings.brand_name,
        "logo_url": settings.logo_url(),
        "appearance": settings.appearance,
    })))
}
