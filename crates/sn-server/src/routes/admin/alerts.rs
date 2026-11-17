use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use sn_domain::access::Permission;
use sn_services::alerts::{AlertLink, AlertsUpdate, AlertsView};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/alerts", get(view).put(update))
        .route("/alerts/link", post(link).delete(unlink))
        .route("/alerts/test", post(test))
}

async fn view(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<AlertsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.alerts().view().await?))
}

async fn update(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<AlertsUpdate>,
) -> ApiResult<Json<AlertsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .alerts()
            .update(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn link(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<AlertLink>> {
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(state.services.alerts().link().await?))
}

async fn unlink(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<AlertsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state.services.alerts().unlink(admin.0.admin_id).await?,
    ))
}

async fn test(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    state.services.alerts().request_test().await?;
    Ok(Json(serde_json::json!({ "queued": true })))
}
