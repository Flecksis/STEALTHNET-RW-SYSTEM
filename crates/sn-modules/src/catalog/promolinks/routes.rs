use super::{LinkError, LinkView, ListFilter, NewLink};
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::paging::{Page, PageRequest};

impl From<LinkError> for WebError {
    fn from(err: LinkError) -> Self {
        match err {
            LinkError::Rejected(rejection) => {
                WebError::rejected(rejection.code(), rejection.message())
            }
            LinkError::Module(err) => err.into(),
        }
    }
}

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/links", get(list).post(create))
        .route("/links/{id}/switch", post(switch))
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

async fn list(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<LinkView>>, WebError> {
    admin.require(Permission::Marketing)?;
    let filter = ListFilter::parse(query.status.as_deref()).ok_or_else(|| {
        WebError::rejected("validation.invalid_value", "фильтр: all, active или off")
    })?;
    Ok(Json(
        super::list(&ctx, filter, PageRequest::new(query.limit, query.after)).await?,
    ))
}

async fn create(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(body): Json<NewLink>,
) -> Result<Json<LinkView>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::create(&ctx, &body, Some(admin.0.admin_id)).await?,
    ))
}

#[derive(Deserialize)]
struct SwitchRequest {
    enabled: bool,
}

async fn switch(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(body): Json<SwitchRequest>,
) -> Result<Json<LinkView>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::set_enabled(&ctx, id, body.enabled, Some(admin.0.admin_id)).await?,
    ))
}
