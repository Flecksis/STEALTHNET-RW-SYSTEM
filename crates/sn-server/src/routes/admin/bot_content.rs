use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::bot_content::ImageMode;
use sn_services::bot_content::{
    ContentUpdate, ContentView, DocumentsUpdate, DocumentsView, ImagesView,
};
use sn_services::bot_layout::{LayoutUpdate, LayoutView};
use sn_store::BOT_IMAGE_MAX_BYTES;

const UPLOAD_LIMIT: usize = BOT_IMAGE_MAX_BYTES + 64 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/bot/content", get(content).put(update_content))
        .route("/bot/layout", get(layout).put(update_layout))
        .route("/bot/images", get(images))
        .route(
            "/bot/images/{slot}",
            post(upload)
                .delete(remove)
                .layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route("/bot/images/{slot}/mode", put(mode))
        .route("/bot/images/{slot}/file", get(file))
        .route("/settings/documents", get(documents).put(update_documents))
}

async fn content(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<ContentView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.bot_content_ops().content().await?))
}

async fn update_content(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ContentUpdate>,
) -> ApiResult<Json<ContentView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .bot_content_ops()
            .update_content(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn layout(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<LayoutView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.bot_layout().view().await?))
}

async fn update_layout(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<LayoutUpdate>,
) -> ApiResult<Json<LayoutView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .bot_layout()
            .update(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn images(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<ImagesView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.bot_content_ops().images()))
}

fn image_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| matches!(value.as_str(), "image/png" | "image/jpeg"))
}

async fn upload(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(slot): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<ImagesView>> {
    admin.require(Permission::Settings)?;
    if !image_type(&headers) {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "bot_image.format",
            "Подходят только JPEG и PNG",
        ));
    }
    Ok(Json(
        state
            .services
            .bot_content_ops()
            .upload_image(&slot, &body, admin.0.admin_id)
            .await?,
    ))
}

async fn remove(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(slot): Path<String>,
) -> ApiResult<Json<ImagesView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .bot_content_ops()
            .delete_image(&slot, admin.0.admin_id)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct ModeBody {
    mode: ImageMode,
}

async fn mode(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(slot): Path<String>,
    ApiJson(body): ApiJson<ModeBody>,
) -> ApiResult<Json<ImagesView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .bot_content_ops()
            .set_image_mode(&slot, body.mode, admin.0.admin_id)
            .await?,
    ))
}

async fn file(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(slot): Path<String>,
) -> ApiResult<Response> {
    admin.require(Permission::Settings)?;
    let Some(image) = state.services.bot_content_ops().image(&slot).await? else {
        return Err(ApiError::not_found());
    };
    let content_type =
        HeaderValue::from_str(&image.content_type).map_err(|_| ApiError::not_found())?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=60"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'"),
            ),
        ],
        image.bytes,
    )
        .into_response())
}

async fn documents(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<DocumentsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.bot_content_ops().documents()))
}

async fn update_documents(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<DocumentsUpdate>,
) -> ApiResult<Json<DocumentsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .bot_content_ops()
            .update_documents(&body, admin.0.admin_id)
            .await?,
    ))
}
