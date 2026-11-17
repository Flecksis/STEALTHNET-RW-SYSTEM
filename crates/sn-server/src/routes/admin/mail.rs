use crate::error::ApiResult;
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;

pub fn routes() -> Router<AppState> {
    Router::new().route("/mail/test", post(test))
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
