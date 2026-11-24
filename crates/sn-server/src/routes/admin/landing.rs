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
use sn_domain::landing_css::CssPlace;
use sn_services::landing::{CssCheck, CssReport, LandingUpdate, LandingView, UploadedImage};
use std::collections::BTreeMap;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/landing", get(view).put(update))
        .route("/settings/landing/css", post(check_css))
        .route(
            "/settings/landing/images",
            post(upload).layer(DefaultBodyLimit::max(IMAGE_UPLOAD_LIMIT)),
        )
}

async fn view(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<LandingView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.landing().view()))
}

async fn update(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<LandingUpdate>,
) -> ApiResult<Json<LandingView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .landing()
            .update(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn check_css(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CssCheck>,
) -> ApiResult<Json<BTreeMap<CssPlace, CssReport>>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.landing().check_css(&body).await?))
}

async fn upload(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<UploadedImage>> {
    admin.require(Permission::Settings)?;
    if !matches!(
        declared_type(&headers).as_str(),
        "image/png" | "image/jpeg" | "image/webp"
    ) {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "landing_image.format",
            "Подходят только PNG, JPEG и WebP",
        ));
    }
    Ok(Json(
        state
            .services
            .landing()
            .upload_image(&body, admin.0.admin_id)
            .await?,
    ))
}
