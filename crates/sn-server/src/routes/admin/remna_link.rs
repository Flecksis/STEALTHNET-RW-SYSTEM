use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::{ClientId, SubscriptionId};
use sn_services::remna_link::{AddSearch, Added, LinkRequest, LinkSearch, Linked};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/subscriptions/{id}/remna/search", post(search))
        .route("/subscriptions/{id}/remna/link", post(link))
        .route("/clients/{id}/remna/search", post(add_search))
        .route("/clients/{id}/remna/add", post(add))
}

fn allowed(admin: &CurrentAdmin) -> ApiResult<()> {
    admin.require(Permission::ClientsWrite)?;
    admin.require_session()
}

#[derive(Deserialize)]
struct SearchRequest {
    q: String,
}

async fn search(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<SearchRequest>,
) -> ApiResult<Json<LinkSearch>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .remna_links()
            .search(SubscriptionId(id), &body.q)
            .await?,
    ))
}

async fn link(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<LinkRequest>,
) -> ApiResult<Json<Linked>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .remna_links()
            .link(SubscriptionId(id), &body, admin.actor())
            .await?,
    ))
}

async fn add_search(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<SearchRequest>,
) -> ApiResult<Json<AddSearch>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .remna_links()
            .add_search(ClientId(id), &body.q)
            .await?,
    ))
}

async fn add(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<LinkRequest>,
) -> ApiResult<Json<Added>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .remna_links()
            .add(ClientId(id), &body, admin.actor())
            .await?,
    ))
}
