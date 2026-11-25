use crate::error::ApiResult;
use crate::extract::{ApiJson, Meta};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use sn_services::app_links::OpenedApp;
use sn_services::limits::rules;

pub const PATH: &str = "/api/public/app-link";

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, post(open))
}

#[derive(Deserialize)]
struct Open {
    token: String,
    holder: String,
}

async fn open(
    State(state): State<AppState>,
    Meta(meta): Meta,
    ApiJson(body): ApiJson<Open>,
) -> ApiResult<Json<OpenedApp>> {
    state
        .services
        .limits()
        .hit_ip(rules::APP_LINK_IP, meta.ip.as_deref())
        .await?;
    Ok(Json(
        state
            .services
            .app_links()
            .open(&body.token, &body.holder)
            .await?,
    ))
}
