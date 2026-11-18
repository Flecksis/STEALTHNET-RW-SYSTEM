use super::store::{self, Counts, Exceed, ExceedFilter, ExceedItem};
use super::{Rule, RuleInput, lift_by_admin, validate};
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::ids::ClientId;
use sn_domain::paging::{Page, PageRequest};

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/summary", get(summary))
        .route("/rules", get(rules).post(create))
        .route("/rules/{id}", put(update).delete(remove))
        .route("/exceeds", get(exceeds))
        .route("/exceeds/{id}/lift", post(lift))
        .with_state(ctx)
}

#[derive(Serialize)]
struct SummaryView {
    #[serde(flatten)]
    counts: Counts,
    offset_minutes: i32,
    last_scan: Option<serde_json::Value>,
}

async fn summary(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<SummaryView>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = ctx.store.conn().await?;
    let now = ctx.core.now();
    Ok(Json(SummaryView {
        counts: store::counts(&mut conn, now).await?,
        offset_minutes: store::offset_minutes(&mut conn).await?,
        last_scan: store::last_scan(&mut conn).await?,
    }))
}

#[derive(Serialize)]
struct RuleView {
    #[serde(flatten)]
    rule: Rule,
    group: String,
    description: String,
}

fn view(rule: Rule) -> RuleView {
    RuleView {
        group: rule.group(),
        description: rule.describe(),
        rule,
    }
}

async fn rules(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<Vec<RuleView>>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = ctx.store.conn().await?;
    Ok(Json(
        store::rules(&mut conn, false)
            .await?
            .into_iter()
            .map(view)
            .collect(),
    ))
}

async fn create(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(body): Json<RuleInput>,
) -> Result<Json<RuleView>, WebError> {
    admin.require(Permission::Modules)?;
    let rule = validate(&body, ctx.core.now())?;
    let mut conn = ctx.store.conn().await?;
    let saved = store::save_rule(&mut conn, None, &rule, Some(admin.0.admin_id))
        .await?
        .ok_or(ModuleError::NotFound("правило"))?;
    Ok(Json(view(saved)))
}

async fn update(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(body): Json<RuleInput>,
) -> Result<Json<RuleView>, WebError> {
    admin.require(Permission::Modules)?;
    let rule = validate(&body, ctx.core.now())?;
    let mut conn = ctx.store.conn().await?;
    let saved = store::save_rule(&mut conn, Some(id), &rule, Some(admin.0.admin_id))
        .await?
        .ok_or(ModuleError::NotFound("правило"))?;
    Ok(Json(view(saved)))
}

async fn remove(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::Modules)?;
    let mut conn = ctx.store.conn().await?;
    if !store::delete_rule(&mut conn, id).await? {
        return Err(ModuleError::NotFound("правило").into());
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ExceedQuery {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    client: Option<i64>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

#[derive(Serialize)]
struct ExceedPage {
    #[serde(flatten)]
    page: Page<ExceedItem>,
    offset_minutes: i32,
}

async fn exceeds(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ExceedQuery>,
) -> Result<Json<ExceedPage>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let filter = ExceedFilter::parse(query.state.as_deref()).ok_or_else(|| {
        WebError::rejected("validation.invalid_value", "state: current или history")
    })?;
    let mut conn = ctx.store.conn().await?;
    let offset_minutes = store::offset_minutes(&mut conn).await?;
    let page = store::list(
        &mut conn,
        filter,
        query.client.map(ClientId),
        ctx.core.now(),
        PageRequest::new(query.limit, query.after),
    )
    .await?;
    Ok(Json(ExceedPage {
        page,
        offset_minutes,
    }))
}

async fn lift(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<Exceed>, WebError> {
    admin.require(Permission::ClientsWrite)?;
    Ok(Json(lift_by_admin(&ctx, id, admin.0.admin_id).await?))
}
