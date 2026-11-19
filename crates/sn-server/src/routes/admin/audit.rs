use crate::error::ApiResult;
use crate::extract::{ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_services::audit_journal::{
    ClientChoice, JournalItem, JournalMeta, JournalPage, JournalQuery,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/audit", get(list))
        .route("/audit/meta", get(meta))
        .route("/audit/clients", get(clients))
        .route("/audit/{id}", get(entry))
}

fn allowed(admin: &CurrentAdmin) -> ApiResult<()> {
    admin.require_session()?;
    admin.require(Permission::AuditLog)
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<JournalQuery>,
) -> ApiResult<Json<JournalPage>> {
    allowed(&admin)?;
    Ok(Json(state.services.audit_journal().page(&query).await?))
}

async fn meta(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<JournalMeta>> {
    allowed(&admin)?;
    Ok(Json(state.services.audit_journal().meta().await?))
}

async fn entry(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<JournalItem>> {
    allowed(&admin)?;
    Ok(Json(state.services.audit_journal().entry(id).await?))
}

#[derive(Debug, Default, Deserialize)]
struct ClientsQuery {
    #[serde(default)]
    q: String,
}

async fn clients(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ClientsQuery>,
) -> ApiResult<Json<Vec<ClientChoice>>> {
    allowed(&admin)?;
    Ok(Json(
        state.services.audit_journal().clients(&query.q).await?,
    ))
}
