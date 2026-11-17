use super::Paging;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::TicketId;
use sn_domain::support::TicketFilter;
use sn_services::support::{ReplyRequest, TicketThread};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/support/tickets", get(list))
        .route("/support/tickets/{id}", get(thread).post(reply))
        .route("/support/tickets/{id}/close", post(close))
        .route("/support/tickets/{id}/reopen", post(reopen))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

fn filter(value: Option<&str>) -> ApiResult<TicketFilter> {
    match value.map(str::trim).unwrap_or("waiting") {
        "" | "waiting" => Ok(TicketFilter::Waiting),
        "answered" => Ok(TicketFilter::Answered),
        "closed" => Ok(TicketFilter::Closed),
        other => Err(ApiError::bad_request(
            "validation.filter",
            format!("неизвестный фильтр {other}"),
        )),
    }
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    let filter = filter(query.filter.as_deref())?;
    let paging = Paging {
        limit: query.limit,
        after: query.after,
    };
    let (page, counts) = state
        .services
        .support()
        .list_view(filter, paging.request(), query.q.as_deref())
        .await?;
    Ok(Json(serde_json::json!({
        "items": page.items,
        "next": page.next,
        "counts": counts,
    })))
}

async fn thread(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<TicketThread>> {
    admin.require(Permission::Support)?;
    Ok(Json(
        state
            .services
            .support()
            .thread(TicketId(id), admin.actor())
            .await?,
    ))
}

async fn reply(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<ReplyRequest>,
) -> ApiResult<Json<TicketThread>> {
    admin.require(Permission::Support)?;
    let support = state.services.support();
    support.reply(TicketId(id), &body, admin.actor()).await?;
    Ok(Json(support.thread(TicketId(id), admin.actor()).await?))
}

async fn close(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    state
        .services
        .support()
        .close(TicketId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn reopen(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    state
        .services
        .support()
        .reopen(TicketId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
