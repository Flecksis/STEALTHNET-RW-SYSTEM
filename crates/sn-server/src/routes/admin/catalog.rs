use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::{CategoryId, TariffId};
use sn_domain::tariff::{Category, Tariff};
use sn_services::tariffs::{CategoryInput, TariffInput, TariffsView};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tariffs", get(list).post(create))
        .route("/tariffs/{id}", get(one).put(update).patch(visibility))
        .route("/categories", get(categories).post(create_category))
        .route("/categories/{id}", put(update_category))
}

async fn list(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<TariffsView>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(state.services.tariffs().list().await?))
}

async fn one(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Tariff>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(state.services.tariffs().get(TariffId(id)).await?))
}

async fn create(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TariffInput>,
) -> ApiResult<Json<Tariff>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .create(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn update(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<TariffInput>,
) -> ApiResult<Json<Tariff>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .update(TariffId(id), &body, admin.0.admin_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct VisibilityRequest {
    on_sale: bool,
}

async fn visibility(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<VisibilityRequest>,
) -> ApiResult<Json<Tariff>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .set_on_sale(TariffId(id), body.on_sale, admin.0.admin_id)
            .await?,
    ))
}

async fn categories(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Tariffs)?;
    let view = state.services.tariffs().list().await?;
    Ok(Json(serde_json::json!({ "categories": view.categories })))
}

async fn create_category(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CategoryInput>,
) -> ApiResult<Json<Category>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .create_category(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn update_category(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<CategoryInput>,
) -> ApiResult<Json<Category>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .update_category(CategoryId(id), &body, admin.0.admin_id)
            .await?,
    ))
}
