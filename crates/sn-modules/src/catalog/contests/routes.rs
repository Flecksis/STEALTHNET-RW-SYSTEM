use super::{Contest, ContestDraft, EntryFilter, EntryItem, ListFilter, ListItem, Options};
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::ClientId;
use sn_domain::paging::{Page, PageRequest};

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/options", get(options))
        .route("/contests", get(list).post(create))
        .route("/contests/{id}", get(one).put(update).delete(remove))
        .route("/contests/{id}/cancel", post(cancel))
        .route("/contests/{id}/entries", get(entries))
        .route("/contests/{id}/entries.csv", get(csv))
        .route("/contests/{id}/winners/{client}/retry", post(retry))
        .route(
            "/contests/{id}/winners/{client}/handed",
            post(handed).delete(unhanded),
        )
        .with_state(ctx)
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn options(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<Options>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::options(&ctx).await?))
}

async fn list(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<ListItem>>, WebError> {
    admin.require(Permission::Marketing)?;
    let filter = ListFilter::parse(query.status.as_deref()).ok_or_else(|| {
        WebError::rejected("validation.invalid_value", "фильтр: all, live или done")
    })?;
    Ok(Json(
        super::list(&ctx, filter, PageRequest::new(query.limit, query.after)).await?,
    ))
}

async fn create(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(body): Json<ContestDraft>,
) -> Result<Json<Contest>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::create(&ctx, &body, Some(admin.0.admin_id)).await?,
    ))
}

async fn one(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Contest>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::contest(&ctx, id).await?))
}

async fn update(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(body): Json<ContestDraft>,
) -> Result<Json<Contest>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::update(&ctx, id, &body, Some(admin.0.admin_id)).await?,
    ))
}

async fn remove(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<StatusCode, WebError> {
    admin.require(Permission::Marketing)?;
    super::delete(&ctx, id, Some(admin.0.admin_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn cancel(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Contest>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::cancel(&ctx, id, Some(admin.0.admin_id)).await?))
}

#[derive(Deserialize)]
struct EntriesQuery {
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn entries(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    Query(query): Query<EntriesQuery>,
) -> Result<Json<Page<EntryItem>>, WebError> {
    admin.require(Permission::Marketing)?;
    let filter = EntryFilter::parse(query.filter.as_deref()).ok_or_else(|| {
        WebError::rejected(
            "validation.invalid_value",
            "фильтр: all, drawn, excluded или winners",
        )
    })?;
    Ok(Json(
        super::entries(&ctx, id, filter, PageRequest::new(query.limit, query.after)).await?,
    ))
}

async fn csv(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Response, WebError> {
    admin.require(Permission::Marketing)?;
    let body = super::entries_csv(&ctx, id).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"contest-{id}.csv\""),
            ),
        ],
        body,
    )
        .into_response())
}

async fn retry(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path((id, client)): Path<(i64, i64)>,
) -> Result<Json<Contest>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::retry_prize(&ctx, id, ClientId(client), Some(admin.0.admin_id)).await?,
    ))
}

async fn handed(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path((id, client)): Path<(i64, i64)>,
) -> Result<Json<Contest>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::hand_prize(&ctx, id, ClientId(client), true, Some(admin.0.admin_id)).await?,
    ))
}

async fn unhanded(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path((id, client)): Path<(i64, i64)>,
) -> Result<Json<Contest>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::hand_prize(&ctx, id, ClientId(client), false, Some(admin.0.admin_id)).await?,
    ))
}
