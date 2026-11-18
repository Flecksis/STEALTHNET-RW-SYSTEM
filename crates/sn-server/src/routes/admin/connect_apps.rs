use super::settings::{IMAGE_UPLOAD_LIMIT, declared_type};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use sn_domain::access::Permission;
use sn_services::connect_apps::{
    ConnectAppsUpdate, ConnectAppsView, ImportPreview, ImportRequest, UploadedIcon,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/connect-apps", get(view).put(update))
        .route(
            "/settings/connect-apps/icons",
            post(upload).layer(DefaultBodyLimit::max(IMAGE_UPLOAD_LIMIT)),
        )
        .route("/settings/connect-apps/import", post(import))
}

async fn view(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<ConnectAppsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.connect_apps().view()))
}

async fn update(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ConnectAppsUpdate>,
) -> ApiResult<Json<ConnectAppsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .connect_apps()
            .update(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn upload(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<UploadedIcon>> {
    admin.require(Permission::Settings)?;
    let svg = match declared_type(&headers).as_str() {
        "image/svg+xml" => true,
        "image/png" | "image/jpeg" | "image/webp" => false,
        _ => {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "connect_apps.icon",
                "Подходят PNG, JPEG, WebP и SVG",
            ));
        }
    };
    Ok(Json(
        state
            .services
            .connect_apps()
            .upload_icon(&body, svg, admin.0.admin_id)
            .await?,
    ))
}

async fn import(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ImportRequest>,
) -> ApiResult<Json<ImportPreview>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .connect_apps()
            .import(&body, admin.0.admin_id)
            .await?,
    ))
}
