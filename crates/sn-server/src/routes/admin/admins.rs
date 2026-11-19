use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::{AdminRole, Permission};
use sn_domain::ids::AdminId;
use sn_services::admins::{AdminView, IssuedPassword, NewAdminRequest};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admins", get(list).post(create))
        .route("/admins/{id}/role", post(set_role))
        .route("/admins/{id}/remna-sync", post(set_remna_sync))
        .route("/admins/{id}/backups", post(set_backups))
        .route("/admins/{id}/audit-log", post(set_audit_log))
        .route("/admins/{id}/disable", post(disable))
        .route("/admins/{id}/enable", post(enable))
        .route("/admins/{id}/password", post(reset_password))
        .route("/admins/{id}/2fa/reset", post(reset_two_factor))
}

#[derive(Debug, Deserialize)]
struct RoleBody {
    role: AdminRole,
}

#[derive(Debug, Deserialize)]
struct GrantBody {
    on: bool,
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<AdminView>>> {
    admin.require(Permission::Admins)?;
    Ok(Json(state.services.admins().list().await?))
}

async fn create(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<NewAdminRequest>,
) -> ApiResult<Json<IssuedPassword>> {
    admin.require(Permission::Admins)?;
    Ok(Json(
        state
            .services
            .admins()
            .create(admin.0.admin_id, &body)
            .await?,
    ))
}

async fn set_role(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<RoleBody>,
) -> ApiResult<Json<AdminView>> {
    admin.require(Permission::Admins)?;
    Ok(Json(
        state
            .services
            .admins()
            .set_role(admin.0.admin_id, AdminId(id), body.role)
            .await?,
    ))
}

async fn set_remna_sync(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<GrantBody>,
) -> ApiResult<Json<AdminView>> {
    admin.require(Permission::Admins)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .admins()
            .set_remna_sync(admin.0.admin_id, AdminId(id), body.on)
            .await?,
    ))
}

async fn set_backups(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<GrantBody>,
) -> ApiResult<Json<AdminView>> {
    admin.require(Permission::Admins)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .admins()
            .set_backups(admin.0.admin_id, AdminId(id), body.on)
            .await?,
    ))
}

async fn set_audit_log(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<GrantBody>,
) -> ApiResult<Json<AdminView>> {
    admin.require(Permission::Admins)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .admins()
            .set_audit_log(admin.0.admin_id, AdminId(id), body.on)
            .await?,
    ))
}

async fn disable(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<AdminView>> {
    admin.require(Permission::Admins)?;
    Ok(Json(
        state
            .services
            .admins()
            .disable(admin.0.admin_id, AdminId(id))
            .await?,
    ))
}

async fn enable(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<AdminView>> {
    admin.require(Permission::Admins)?;
    Ok(Json(
        state
            .services
            .admins()
            .enable(admin.0.admin_id, AdminId(id))
            .await?,
    ))
}

async fn reset_password(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<IssuedPassword>> {
    admin.require(Permission::Admins)?;
    Ok(Json(
        state
            .services
            .admins()
            .reset_password(admin.0.admin_id, AdminId(id))
            .await?,
    ))
}

async fn reset_two_factor(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<AdminView>> {
    admin.require(Permission::Admins)?;
    Ok(Json(
        state
            .services
            .admins()
            .reset_two_factor(admin.0.admin_id, AdminId(id))
            .await?,
    ))
}
