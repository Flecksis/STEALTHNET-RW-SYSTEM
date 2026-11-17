use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use sn_domain::access::Permission;
use sn_services::api_keys::{ApiKeyView, IssuedKey, NewKeyRequest};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api-keys", get(list).post(create))
        .route("/api-keys/{id}/revoke", post(revoke))
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ApiKeyView>>> {
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(state.services.api_keys().list().await?))
}

async fn create(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<NewKeyRequest>,
) -> ApiResult<Json<IssuedKey>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state.services.api_keys().create(&admin.0, &body).await?,
    ))
}

async fn revoke(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<ApiKeyView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.api_keys().revoke(&admin.0, id).await?))
}
