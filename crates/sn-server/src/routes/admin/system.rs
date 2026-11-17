use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CurrentAdmin, Meta};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_services::ServiceError;
use sn_services::updates::{
    CHECK_DISABLED, CHECK_RATE_LIMITED, HELPER_MISSING, SummaryView, SystemView,
    UPDATE_IN_PROGRESS, UPDATE_RATE_LIMITED, UPDATE_STALE, UPDATE_STARTED,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/system", get(overview))
        .route("/system/summary", get(summary))
        .route("/system/check", post(check))
        .route("/system/settings", put(settings))
        .route("/system/update", post(request).delete(cancel))
}

fn mapped(err: ServiceError) -> ApiError {
    let status = match err.code() {
        HELPER_MISSING | UPDATE_IN_PROGRESS | UPDATE_STALE | UPDATE_STARTED | CHECK_DISABLED => {
            Some(StatusCode::CONFLICT)
        }
        UPDATE_RATE_LIMITED | CHECK_RATE_LIMITED => Some(StatusCode::TOO_MANY_REQUESTS),
        sn_services::auth::TOTP_REQUIRED | sn_services::auth::TOTP_INVALID => {
            Some(StatusCode::UNPROCESSABLE_ENTITY)
        }
        _ => None,
    };
    let mut api = ApiError::from(err);
    if let Some(status) = status {
        api.status = status;
    }
    api
}

async fn summary(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<SummaryView>> {
    Ok(Json(
        state
            .services
            .updates()
            .summary(admin.0.role)
            .await
            .map_err(mapped)?,
    ))
}

async fn overview(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<SystemView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .updates()
            .overview(admin.0.admin_id, admin.0.role)
            .await
            .map_err(mapped)?,
    ))
}

async fn check(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<SystemView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .updates()
            .check_now(admin.0.admin_id, admin.0.role)
            .await
            .map_err(mapped)?,
    ))
}

#[derive(Deserialize)]
struct SettingsRequest {
    check_enabled: bool,
}

async fn settings(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<SettingsRequest>,
) -> ApiResult<Json<SystemView>> {
    admin.require(Permission::Settings)?;
    let updates = state.services.updates();
    updates
        .set_check_enabled(admin.0.admin_id, admin.0.role, body.check_enabled)
        .await
        .map_err(mapped)?;
    Ok(Json(
        updates
            .overview(admin.0.admin_id, admin.0.role)
            .await
            .map_err(mapped)?,
    ))
}

#[derive(Deserialize)]
struct UpdateRequest {
    target: String,
    password: String,
    #[serde(default)]
    code: Option<String>,
}

async fn request(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<UpdateRequest>,
) -> ApiResult<Response> {
    admin.require(Permission::Settings)?;
    let id = state
        .services
        .updates()
        .request(
            admin.0.admin_id,
            admin.0.role,
            &body.target,
            &body.password,
            body.code.as_deref(),
            &meta,
        )
        .await
        .map_err(mapped)?;
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({ "id": id }))).into_response())
}

async fn cancel(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<StatusCode> {
    admin.require(Permission::Settings)?;
    state
        .services
        .updates()
        .cancel(admin.0.admin_id, admin.0.role)
        .await
        .map_err(mapped)?;
    Ok(StatusCode::NO_CONTENT)
}
