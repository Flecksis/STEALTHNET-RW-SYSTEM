use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_services::ext_modules::{
    Card, ConfigUpdate, ConfigView, Credentials, InstallRequest, InstallResult, Journal, PageToken,
    PingResult, PreviewResult, SelfView, Summary,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ext-modules", get(list).post(install))
        .route("/ext-modules/preview", post(preview))
        .route("/ext-modules/me", get(me))
        .route("/ext-modules/{id}", get(card).patch(toggle).delete(remove))
        .route("/ext-modules/{id}/config", get(config).put(update_config))
        .route("/ext-modules/{id}/credentials", post(reissue))
        .route("/ext-modules/{id}/deliveries", get(journal))
        .route("/ext-modules/{id}/deliveries/{delivery}/retry", post(retry))
        .route(
            "/ext-modules/{id}/deliveries/retry-failed",
            post(retry_failed),
        )
        .route("/ext-modules/{id}/ping", post(ping))
        .route("/ext-modules/{id}/page-token", post(page_token))
}

async fn list(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<Vec<Summary>>> {
    admin.require(Permission::Modules)?;
    Ok(Json(state.services.ext_modules().list(&admin.0).await?))
}

#[derive(Deserialize)]
struct PreviewRequest {
    manifest: String,
    #[serde(default)]
    update: Option<String>,
}

async fn preview(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<PreviewRequest>,
) -> ApiResult<Json<PreviewResult>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .preview(&admin.0, &body.manifest, body.update.as_deref())
            .await?,
    ))
}

async fn install(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<InstallRequest>,
) -> ApiResult<Json<InstallResult>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .install(&admin.0, &body)
            .await?,
    ))
}

async fn me(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<SelfView>> {
    Ok(Json(state.services.ext_modules().me(&admin.0).await?))
}

async fn card(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Card>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state.services.ext_modules().card_for(&admin.0, &id).await?,
    ))
}

#[derive(Deserialize)]
struct ToggleRequest {
    enabled: bool,
}

async fn toggle(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ToggleRequest>,
) -> ApiResult<Json<Card>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .set_enabled(&admin.0, &id, body.enabled)
            .await?,
    ))
}

async fn remove(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    state.services.ext_modules().remove(&admin.0, &id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn config(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ConfigView>> {
    admin.require(Permission::Modules)?;
    Ok(Json(
        state.services.ext_modules().config(&admin.0, &id).await?,
    ))
}

async fn update_config(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ConfigUpdate>,
) -> ApiResult<Json<ConfigView>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .update_config(&admin.0, &id, &body)
            .await?,
    ))
}

async fn reissue(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Credentials>> {
    admin.require(Permission::Modules)?;
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().reissue(&admin.0, &id).await?,
    ))
}

async fn journal(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Journal>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().journal(&admin.0, &id).await?,
    ))
}

async fn retry(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path((id, delivery)): Path<(String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    let queued = state
        .services
        .ext_modules()
        .retry(&admin.0, &id, delivery)
        .await?;
    Ok(Json(serde_json::json!({ "queued": queued })))
}

async fn retry_failed(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    let queued = state
        .services
        .ext_modules()
        .retry_failed(&admin.0, &id)
        .await?;
    Ok(Json(serde_json::json!({ "queued": queued })))
}

async fn ping(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PingResult>> {
    admin.require(Permission::Modules)?;
    admin.require_session()?;
    Ok(Json(
        state.services.ext_modules().ping(&admin.0, &id).await?,
    ))
}

async fn page_token(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PageToken>> {
    admin.require_session()?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .page_token(&admin.0, &id)
            .await?,
    ))
}
