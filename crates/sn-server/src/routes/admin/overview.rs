use crate::error::ApiResult;
use crate::extract::{ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use sn_domain::access::Permission;
use sn_services::admin::{DashboardView, FeedPage, FeedQuery};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dashboard", get(dashboard))
        .route("/feed", get(feed))
}

async fn dashboard(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<DashboardView>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(
        state
            .services
            .admin()
            .dashboard(admin.can(Permission::Money))
            .await?,
    ))
}

async fn feed(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<FeedQuery>,
) -> ApiResult<Json<FeedPage>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(
        state
            .services
            .admin()
            .feed(&query, admin.can(Permission::Money))
            .await?,
    ))
}
