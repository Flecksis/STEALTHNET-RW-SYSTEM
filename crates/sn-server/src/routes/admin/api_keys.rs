use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use sn_domain::access::Permission;
use sn_services::api_keys::{ApiKeyView, IssuedKey, NewKeyRequest};
use sn_services::auth::{TOTP_INVALID, TOTP_REQUIRED};

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
    let issued = state
        .services
        .api_keys()
        .create(&admin.0, &body)
        .await
        .map_err(|err| {
            let code = err.code();
            let mut api = ApiError::from(err);
            if code == TOTP_REQUIRED || code == TOTP_INVALID {
                api.status = StatusCode::UNPROCESSABLE_ENTITY;
            }
            api
        })?;
    Ok(Json(issued))
}

async fn revoke(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<ApiKeyView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.api_keys().revoke(&admin.0, id).await?))
}
