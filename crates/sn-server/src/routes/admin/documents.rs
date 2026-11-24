use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_services::documents::{CleanView, DocumentFull, DocumentInput, DocumentsView, ListUpdate};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/documents", get(list).put(replace).post(create))
        .route("/settings/documents/clean", post(clean))
        .route(
            "/settings/documents/{id}",
            get(one).put(update).delete(remove),
        )
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<DocumentsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.documents().view().await?))
}

async fn replace(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ListUpdate>,
) -> ApiResult<Json<DocumentsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .documents()
            .web_dir(state.config.web_dir.as_deref())
            .replace(&body, Some(admin.0.admin_id))
            .await?,
    ))
}

async fn create(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<DocumentInput>,
) -> ApiResult<Json<DocumentFull>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .documents()
            .web_dir(state.config.web_dir.as_deref())
            .create(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn one(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<DocumentFull>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.documents().get(&id).await?))
}

async fn update(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<DocumentInput>,
) -> ApiResult<Json<DocumentFull>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .documents()
            .web_dir(state.config.web_dir.as_deref())
            .update(&id, &body, admin.0.admin_id)
            .await?,
    ))
}

async fn remove(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<DocumentsView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .documents()
            .remove(&id, admin.0.admin_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct CleanBody {
    html: String,
}

async fn clean(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CleanBody>,
) -> ApiResult<Json<CleanView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.documents().clean(&body.html)))
}
