use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::extract::CurrentAdmin;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use sn_domain::access::Permission;
use sn_remnawave::{Host, InfraBillingNodes, Node, SquadAccessibleNodes};
use sn_services::infra::{
    BillingView, CreatedNode, HostAction, HostInput, HostOrderInput, HostsView, NewNodeInput,
    NodeAction, NodeActionResult, NodeEditInput, PaymentHistory, ProfileView, ProviderInput,
    ProviderView, SquadInput, SquadView,
};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/remnawave/status", get(remna_status))
        .route("/infra/nodes", get(nodes).post(create_node))
        .route("/infra/nodes/{uuid}", axum::routing::put(update_node))
        .route("/infra/hosts", get(hosts).post(create_host))
        .route("/infra/hosts/order", axum::routing::put(reorder_hosts))
        .route(
            "/infra/hosts/{uuid}",
            axum::routing::put(update_host).delete(delete_host),
        )
        .route("/infra/hosts/{uuid}/actions/{action}", post(host_action))
        .route("/infra/profiles", get(profiles))
        .route("/infra/providers", get(providers).post(create_provider))
        .route("/infra/nodes/{uuid}/actions/{action}", post(node_action))
        .route("/infra/squads", get(squads).post(create_squad))
        .route("/infra/squads/{uuid}", axum::routing::put(update_squad))
        .route("/infra/billing/view", get(billing_view))
        .route("/infra/billing/history", get(billing_history))
        .route("/infra/billing/{uuid}/paid", post(mark_paid))
        .route("/infra/prices/{uuid}", axum::routing::put(set_price))
        .route(
            "/infra/providers/{uuid}",
            axum::routing::put(update_provider),
        )
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

async fn create_node(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<NewNodeInput>,
) -> ApiResult<Json<CreatedNode>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .create_node(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn update_node(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(uuid): Path<Uuid>,
    ApiJson(body): ApiJson<NodeEditInput>,
) -> ApiResult<Json<Node>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .update_node(uuid, &body, admin.0.admin_id)
            .await?,
    ))
}

async fn hosts(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<HostsView>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(state.services.infra().hosts().await?))
}

async fn create_host(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<HostInput>,
) -> ApiResult<Json<Host>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .create_host(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn update_host(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(uuid): Path<Uuid>,
    ApiJson(body): ApiJson<HostInput>,
) -> ApiResult<Json<Host>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .update_host(uuid, &body, admin.0.admin_id)
            .await?,
    ))
}

async fn host_action(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path((uuid, action)): Path<(Uuid, String)>,
) -> ApiResult<Json<Host>> {
    admin.require(Permission::Infrastructure)?;
    let action = HostAction::parse(&action).ok_or_else(ApiError::not_found)?;
    Ok(Json(
        state
            .services
            .infra()
            .host_action(uuid, action, admin.0.admin_id)
            .await?,
    ))
}

async fn delete_host(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(uuid): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Infrastructure)?;
    state
        .services
        .infra()
        .delete_host(uuid, admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn reorder_hosts(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<HostOrderInput>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Infrastructure)?;
    state
        .services
        .infra()
        .reorder_hosts(&body.uuids, admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn profiles(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ProfileView>>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(state.services.infra().profiles().await?))
}

async fn providers(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ProviderView>>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(state.services.infra().providers().await?))
}

async fn create_squad(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<SquadInput>,
) -> ApiResult<Json<sn_remnawave::InternalSquad>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .save_squad(None, &body, admin.0.admin_id)
            .await?,
    ))
}

async fn update_squad(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(uuid): Path<Uuid>,
    ApiJson(body): ApiJson<SquadInput>,
) -> ApiResult<Json<sn_remnawave::InternalSquad>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .save_squad(Some(uuid), &body, admin.0.admin_id)
            .await?,
    ))
}

async fn billing_view(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<BillingView>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .billing_view(admin.can(Permission::Money))
            .await?,
    ))
}

#[derive(serde::Deserialize)]
struct HistoryQuery {
    #[serde(default)]
    start: u32,
    #[serde(default = "history_size")]
    size: u32,
}

const fn history_size() -> u32 {
    20
}

async fn billing_history(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<PaymentHistory>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .payment_history(query.start, query.size)
            .await?,
    ))
}

#[derive(serde::Deserialize, Default)]
struct PaidRequest {
    #[serde(default)]
    amount_minor: Option<i64>,
}

async fn mark_paid(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(uuid): Path<Uuid>,
    body: Option<ApiJson<PaidRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Infrastructure)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    state
        .services
        .infra()
        .mark_paid(uuid, body.amount_minor, admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(serde::Deserialize)]
struct PriceRequest {
    amount_minor: Option<i64>,
}

async fn set_price(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(uuid): Path<Uuid>,
    ApiJson(body): ApiJson<PriceRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Infrastructure)?;
    state
        .services
        .infra()
        .set_price(uuid, body.amount_minor, admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn create_provider(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ProviderInput>,
) -> ApiResult<Json<ProviderView>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .save_provider(None, &body, admin.0.admin_id)
            .await?,
    ))
}

async fn update_provider(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(uuid): Path<Uuid>,
    ApiJson(body): ApiJson<ProviderInput>,
) -> ApiResult<Json<ProviderView>> {
    admin.require(Permission::Infrastructure)?;
    Ok(Json(
        state
            .services
            .infra()
            .save_provider(Some(uuid), &body, admin.0.admin_id)
            .await?,
    ))
}
