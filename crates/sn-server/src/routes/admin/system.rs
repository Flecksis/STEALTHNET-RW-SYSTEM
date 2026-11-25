use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CurrentAdmin, Meta, trusts_everyone};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_services::ServiceError;
use sn_services::server_clock::Evidence;
use sn_services::updates::{
    CHECK_DISABLED, CHECK_RATE_LIMITED, HELPER_MISSING, SummaryView, SystemView,
    UPDATE_IN_PROGRESS, UPDATE_RATE_LIMITED, UPDATE_STALE, UPDATE_STARTED,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/system", get(overview))
        .route("/system/summary", get(summary))
        .route("/system/warnings", get(warnings))
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

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum MiniApp {
    Off,
    Open,
    Closed,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Warning {
    TrustedProxiesEveryone {
        networks: Vec<String>,
    },
    ServerClock {
        seconds: i64,
        span: String,
        evidence: Evidence,
        miniapp: MiniApp,
    },
}

async fn warnings(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    let mut warnings = Vec::new();
    let everyone = trusts_everyone(&state.config.trusted_proxies);
    if !everyone.is_empty() {
        warnings.push(Warning::TrustedProxiesEveryone {
            networks: everyone.iter().map(ToString::to_string).collect(),
        });
    }
    if let Some(drift) = state.services.server_clock_ops().notice().await? {
        let miniapp = if !state.services.settings().client_access.miniapp {
            MiniApp::Off
        } else if drift.closes_miniapp() {
            MiniApp::Closed
        } else {
            MiniApp::Open
        };
        warnings.push(Warning::ServerClock {
            seconds: drift.seconds,
            span: drift.span(),
            evidence: drift.evidence,
            miniapp,
        });
    }
    Ok(Json(serde_json::json!({ "warnings": warnings })))
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
    admin.require_session()?;
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
    admin.require_session()?;
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
    admin.require_session()?;
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
    admin.require_session()?;
    state
        .services
        .updates()
        .cancel(admin.0.admin_id, admin.0.role)
        .await
        .map_err(mapped)?;
    Ok(StatusCode::NO_CONTENT)
}
