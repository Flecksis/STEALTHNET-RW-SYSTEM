use crate::error::{ApiError, ApiResult};
use crate::extract::CurrentAdmin;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use sn_domain::access::Permission;
use sn_remnawave::{InfraBillingNodes, SquadAccessibleNodes};
use sn_services::infra::{NodeAction, NodeActionResult, SquadView};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/remnawave/status", get(remna_status))
        .route("/infra/nodes", get(nodes))
        .route("/infra/nodes/{uuid}/actions/{action}", post(node_action))
        .route("/infra/squads", get(squads))
        .route("/infra/squads/{uuid}/nodes", get(squad_nodes))
        .route("/infra/billing", get(billing))
}

async fn remna_status(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_services::settings::RemnaCheck>> {
    admin.require(Permission::Settings)?;
    Ok(Json(
        state
            .services
            .settings_ops()
            .check_remnawave(&Default::default())
            .await?,
    ))
}

async fn nodes(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Infrastructure)?;
    let (summary, nodes) = state.services.infra().nodes().await?;
    Ok(Json(
        serde_json::json!({ "summary": summary, "nodes": nodes }),
    ))
}

async fn node_action(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path((uuid, action)): Path<(Uuid, String)>,
) -> ApiResult<Json<NodeActionResult>> {
    admin.require(Permission::Infrastructure)?;
    let action = NodeAction::parse(&action).ok_or_else(ApiError::not_found)?;
    Ok(Json(
        state
            .services
            .infra()
            .node_action(uuid, action, admin.0.admin_id)
            .await?,
    ))
}

async fn squads(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<SquadView>>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(state.services.infra().squads().await?))
}

async fn squad_nodes(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(uuid): Path<Uuid>,
) -> ApiResult<Json<SquadAccessibleNodes>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(state.services.infra().squad_nodes(uuid).await?))
}

async fn billing(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<InfraBillingNodes>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(state.services.infra().billing().await?))
}
