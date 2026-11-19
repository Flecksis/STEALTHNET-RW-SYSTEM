use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentClient, Meta};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_services::email_change::{EmailPending, EmailProofInput, EmailState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/security/email", get(email_state))
        .route("/security/email/start", post(start))
        .route("/security/email/proof", post(proof))
        .route("/security/email/confirm", post(confirm))
}

async fn email_state(
    client: CurrentClient,
    State(state): State<AppState>,
) -> ApiResult<Json<EmailState>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .email_state(client.0.client_id)
            .await?,
    ))
}

async fn proof(
    client: CurrentClient,
    State(state): State<AppState>,
    Meta(meta): Meta,
) -> ApiResult<Json<EmailPending>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .email_proof_start(&client.0, &meta)
            .await?,
    ))
}

#[derive(Deserialize)]
struct StartBody {
    email: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

async fn start(
    client: CurrentClient,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<StartBody>,
) -> ApiResult<Json<EmailPending>> {
    client.require_full()?;
    let proof = EmailProofInput {
        password: body.password.as_deref(),
        code: body.code.as_deref(),
    };
    Ok(Json(
        state
            .services
            .auth()
            .email_change_start(&client.0, &body.email, proof, &meta)
            .await?,
    ))
}

#[derive(Deserialize)]
struct ConfirmBody {
    email: String,
    code: String,
}

async fn confirm(
    client: CurrentClient,
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<ConfirmBody>,
) -> ApiResult<Json<EmailState>> {
    client.require_full()?;
    Ok(Json(
        state
            .services
            .auth()
            .email_change_confirm(&client.0, &body.email, &body.code, &meta)
            .await?,
    ))
}
