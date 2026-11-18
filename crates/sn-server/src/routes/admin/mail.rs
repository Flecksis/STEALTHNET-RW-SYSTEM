use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_services::mail_letters::{DraftLetter, LettersUpdate, LettersView, PreviewView};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/test", post(test))
        .route("/mail/letters", get(letters).put(update_letters))
        .route("/mail/letters/preview", post(preview))
        .route("/mail/letters/test", post(test_letter))
}

#[derive(Debug, Deserialize)]
struct TestBody {
    to: String,
}

async fn test(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TestBody>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    state.services.mail().test(&body.to).await?;
    Ok(Json(serde_json::json!({ "sent": true })))
}

async fn letters(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<LettersView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.letters().view().await?))
}

async fn update_letters(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<LettersUpdate>,
) -> ApiResult<Json<LettersView>> {
    admin.require(Permission::Settings)?;
    if body.touches_module() {
        admin.require(Permission::Modules)?;
    }
    Ok(Json(
        state
            .services
            .letters()
            .update(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn preview(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<DraftLetter>,
) -> ApiResult<Json<PreviewView>> {
    admin.require(Permission::Settings)?;
    Ok(Json(state.services.letters().preview(&body).await?))
}

#[derive(Debug, Deserialize)]
struct LetterTestBody {
    to: String,
    #[serde(flatten)]
    draft: DraftLetter,
}

async fn test_letter(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<LetterTestBody>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Settings)?;
    admin.require_session()?;
    state.services.letters().test(&body.draft, &body.to).await?;
    Ok(Json(serde_json::json!({ "sent": true })))
}
