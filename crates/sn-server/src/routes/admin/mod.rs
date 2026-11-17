mod admins;
mod auth;
mod bot_content;
mod catalog;
mod clients;
mod delivery;
mod infra;
mod landing;
mod money;
mod overview;
pub(crate) mod photos;
#[cfg(test)]
mod review_tests;
mod settings;
mod stars;
mod support;
mod system;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::Router;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sn_domain::paging::PageRequest;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(auth::routes())
        .merge(admins::routes())
        .merge(overview::routes())
        .merge(clients::routes())
        .merge(photos::routes())
        .merge(money::routes())
        .merge(stars::routes())
        .merge(delivery::routes())
        .merge(catalog::routes())
        .merge(infra::routes())
        .merge(support::routes())
        .merge(settings::routes())
        .merge(landing::routes())
        .merge(system::routes())
        .merge(bot_content::routes())
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub(crate) struct Paging {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub after: Option<i64>,
}

impl Paging {
    pub(crate) fn request(self) -> PageRequest {
        PageRequest::new(self.limit, self.after)
    }
}

pub(crate) fn decode<T: DeserializeOwned>(value: serde_json::Value) -> ApiResult<T> {
    serde_json::from_value(value)
        .map_err(|e| ApiError::bad_request("validation.invalid_body", e.to_string()))
}

pub(crate) fn json<T: serde::Serialize>(value: &T) -> ApiResult<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|e| ApiError::from(sn_services::ServiceError::Internal(e.to_string())))
}
