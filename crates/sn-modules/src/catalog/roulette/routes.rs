use super::{AdminView, ClientState, ConfigDraft, RouletteError, SpinItem, SpinResult};
use crate::catalog::antibot;
use crate::catalog::web::{Admin, Customer, WebError};
use crate::context::ModuleContext;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::ids::SubscriptionId;
use sn_domain::paging::{Page, PageRequest};
use uuid::Uuid;

impl From<RouletteError> for WebError {
    fn from(err: RouletteError) -> Self {
        match err {
            RouletteError::Rejected { code, message } => match code {
                "roulette.blocked" => WebError::new(StatusCode::FORBIDDEN, code, message),
                _ => WebError::rejected(code, message),
            },
            RouletteError::Module(err) => err.into(),
        }
    }
}

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/settings", get(settings).put(save))
        .route("/spins", get(spins))
        .with_state(ctx)
}

pub fn client(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/state", get(state))
        .route("/spin", post(spin))
        .with_state(ctx)
}

async fn settings(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<AdminView>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::settings(&ctx).await?))
}

async fn save(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(body): Json<ConfigDraft>,
) -> Result<Json<AdminView>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::save(&ctx, &body, Some(admin.0.admin_id)).await?,
    ))
}

#[derive(Deserialize)]
struct SpinsQuery {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn spins(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<SpinsQuery>,
) -> Result<Json<Page<SpinItem>>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::spins(&ctx, PageRequest::new(query.limit, query.after)).await?,
    ))
}

async fn state(
    State(ctx): State<ModuleContext>,
    customer: Customer,
) -> Result<Json<ClientState>, WebError> {
    Ok(Json(super::state(&ctx, customer.0.client_id).await?))
}

#[derive(Deserialize)]
struct SpinRequest {
    request_id: Uuid,
    #[serde(default)]
    subscription_id: Option<SubscriptionId>,
    #[serde(default)]
    confirm: Option<String>,
}

#[derive(Serialize)]
struct Prompt {
    token: String,
    text: &'static str,
}

#[derive(Serialize)]
#[serde(untagged)]
enum SpinReply {
    Spun(Box<SpinResult>),
    Confirm { confirm: Prompt },
}

async fn spin(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Json(body): Json<SpinRequest>,
) -> Result<Json<SpinReply>, WebError> {
    customer.require_full()?;
    let client_id = customer.0.client_id;
    let token = body
        .confirm
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let result = match token {
        Some(token) => {
            super::spin_confirmed(
                &ctx,
                client_id,
                body.request_id,
                body.subscription_id,
                token,
            )
            .await
        }
        None => super::spin(&ctx, client_id, body.request_id, body.subscription_id).await,
    };
    match result {
        Ok(spun) => Ok(Json(SpinReply::Spun(Box::new(spun)))),
        Err(RouletteError::Rejected { code, .. }) if code == antibot::CONFIRM_CODE => {
            let token = antibot::challenge(&ctx, client_id).await?;
            Ok(Json(SpinReply::Confirm {
                confirm: Prompt {
                    token,
                    text: super::CONFIRM_TEXT,
                },
            }))
        }
        Err(err) => Err(err.into()),
    }
}
