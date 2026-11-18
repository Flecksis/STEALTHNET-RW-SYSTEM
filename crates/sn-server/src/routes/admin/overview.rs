use crate::error::ApiResult;
use crate::extract::{ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use sn_domain::access::Permission;
use sn_services::admin::{DashboardView, FeedPage, FeedQuery};
use sn_services::decisions::{Decisions, Scope};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dashboard", get(dashboard))
        .route("/feed", get(feed))
        .route("/badges", get(badges))
}

#[derive(Serialize)]
struct DashboardReply {
    #[serde(flatten)]
    view: DashboardView,
    decisions: Decisions,
}

fn scope(admin: &CurrentAdmin) -> Scope {
    Scope {
        money: admin.can(Permission::Money),
        refunds: admin.can(Permission::Refunds),
    }
}

async fn badges(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let support = if admin.can(Permission::Support) {
        state.services.admin().support_waiting().await?
    } else {
        0
    };
    let money = state.services.decisions().waiting(scope(&admin)).await?;
    Ok(Json(
        serde_json::json!({ "support": support, "money": money }),
    ))
}

async fn dashboard(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<DashboardReply>> {
    admin.require(Permission::ClientsRead)?;
    let view = state
        .services
        .admin()
        .dashboard(admin.can(Permission::Money))
        .await?;
    let decisions = state.services.decisions().summary(scope(&admin)).await?;
    Ok(Json(DashboardReply { view, decisions }))
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
