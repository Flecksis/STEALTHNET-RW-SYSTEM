use super::store::{self, JournalFilter, JournalItem, MarkItem, Summary};
use super::{AntibotSettings, active};
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::ids::ClientId;
use sn_domain::paging::{Page, PageRequest};

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/summary", get(summary))
        .route("/journal", get(journal))
        .route("/marks", get(marks))
        .route("/marks/{client_id}", get(mark))
        .route("/marks/{client_id}/clear", post(clear))
        .with_state(ctx)
}

#[derive(Serialize)]
struct CaptchaState {
    secret_set: bool,
    ready: bool,
    problem: Option<super::captcha::Health>,
}

#[derive(Serialize)]
struct SummaryView {
    settings: AntibotSettings,
    captcha: CaptchaState,
    #[serde(flatten)]
    counts: Summary,
}

fn captcha_state(
    ctx: &ModuleContext,
    settings: &AntibotSettings,
    health: Option<super::captcha::Health>,
) -> CaptchaState {
    let config = ctx.core.module_config(sn_domain::module::ModuleId::Antibot);
    let ready = super::captcha::Captcha::of(&config).is_some();
    CaptchaState {
        secret_set: config.text("captcha_secret").is_some(),
        ready,
        problem: health.filter(|health| ready && health.provider == settings.captcha),
    }
}

async fn summary(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<SummaryView>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = ctx.store.conn().await?;
    let settings = active(&mut conn).await?.unwrap_or_default();
    let counts = store::summary(&mut conn).await?;
    let health = super::captcha::health(&mut conn).await?;
    let captcha = captcha_state(&ctx, &settings, health);
    Ok(Json(SummaryView {
        settings,
        captcha,
        counts,
    }))
}

#[derive(Deserialize)]
struct JournalQuery {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    client: Option<i64>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn journal(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<JournalQuery>,
) -> Result<Json<Page<JournalItem>>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let filter = JournalFilter::parse(query.kind.as_deref()).ok_or_else(|| {
        WebError::rejected(
            "validation.invalid_value",
            "kind: all, signup, trial, promo, referral, roulette, contest или cleared",
        )
    })?;
    let mut conn = ctx.store.conn().await?;
    Ok(Json(
        store::journal(
            &mut conn,
            filter,
            query.client.map(ClientId),
            PageRequest::new(query.limit, query.after),
        )
        .await?,
    ))
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn marks(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<MarkItem>>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = ctx.store.conn().await?;
    Ok(Json(
        store::open_marks(&mut conn, PageRequest::new(query.limit, query.after)).await?,
    ))
}

#[derive(Serialize)]
struct MarkView {
    marked: bool,
    reasons: Vec<store::Reason>,
    marked_at: Option<chrono::DateTime<chrono::Utc>>,
    confirmed_at: Option<chrono::DateTime<chrono::Utc>>,
    cleared_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn mark(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(client_id): Path<i64>,
) -> Result<Json<MarkView>, WebError> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = ctx.store.conn().await?;
    let found = store::mark_of(&mut conn, ClientId(client_id)).await?;
    Ok(Json(match found {
        Some(mark) => MarkView {
            marked: mark.cleared_at.is_none(),
            reasons: store::reasons(&mark.reasons),
            marked_at: Some(mark.marked_at),
            confirmed_at: mark.confirmed_at,
            cleared_at: mark.cleared_at,
        },
        None => MarkView {
            marked: false,
            reasons: Vec::new(),
            marked_at: None,
            confirmed_at: None,
            cleared_at: None,
        },
    }))
}

async fn clear(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(client_id): Path<i64>,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::ClientsWrite)?;
    let cleared = super::clear(&ctx, ClientId(client_id), Some(admin.0.admin_id)).await?;
    Ok(Json(serde_json::json!({ "cleared": cleared })))
}
