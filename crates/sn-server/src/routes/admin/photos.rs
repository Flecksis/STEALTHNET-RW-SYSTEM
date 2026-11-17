use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::ClientId;
use sn_store::Db;
use std::collections::BTreeMap;

pub const BATCH_MAX: usize = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clients/photos", get(versions))
        .route("/clients/{id}/photo", get(photo))
}

#[derive(Debug, Deserialize)]
struct Ids {
    #[serde(default)]
    ids: String,
}

fn parse_ids(raw: &str) -> Vec<ClientId> {
    let mut ids: Vec<ClientId> = raw
        .split(',')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
        .map(ClientId)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.truncate(BATCH_MAX);
    ids
}

async fn versions(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<Ids>,
) -> ApiResult<Json<BTreeMap<String, String>>> {
    admin.require(Permission::ClientsRead)?;
    let ids = parse_ids(&query.ids);
    if ids.is_empty() {
        return Ok(Json(BTreeMap::new()));
    }
    let mut conn = state
        .services
        .store()
        .conn()
        .await
        .map_err(sn_services::ServiceError::from)?;
    let mut photos = conn.client_photos();
    if let Err(err) = photos.request_missing(&ids).await {
        tracing::debug!(error = %err, "запрос фото клиентов не сохранён");
    }
    let found = photos
        .versions(&ids)
        .await
        .map_err(sn_services::ServiceError::from)?;
    Ok(Json(
        found
            .into_iter()
            .map(|(id, version)| (id.0.to_string(), version))
            .collect(),
    ))
}

#[derive(Debug, Default, Deserialize)]
struct PhotoQuery {
    #[serde(default)]
    v: Option<String>,
}

async fn photo(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<PhotoQuery>,
) -> ApiResult<Response> {
    admin.require(Permission::ClientsRead)?;
    let mut conn = state
        .services
        .store()
        .conn()
        .await
        .map_err(sn_services::ServiceError::from)?;
    let Some(photo) = conn
        .client_photos()
        .get(ClientId(id))
        .await
        .map_err(sn_services::ServiceError::from)?
    else {
        return Err(ApiError::not_found());
    };
    let cache = if query.v.as_deref() == Some(photo.version.as_str()) {
        "private, max-age=31536000, immutable"
    } else {
        "private, no-cache"
    };
    image(photo, cache)
}

pub(crate) fn image(photo: sn_store::ClientPhoto, cache: &'static str) -> ApiResult<Response> {
    let content_type =
        HeaderValue::from_str(&photo.content_type).map_err(|_| ApiError::not_found())?;
    let etag = HeaderValue::from_str(&format!("\"{}\"", photo.version))
        .unwrap_or_else(|_| HeaderValue::from_static("\"photo\""));
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            (header::ETAG, etag),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'"),
            ),
        ],
        photo.content,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn номера_клиентов_разбираются_без_мусора_и_повторов() {
        assert_eq!(
            parse_ids("3, 1,abc,3,-5,0,,2"),
            vec![ClientId(1), ClientId(2), ClientId(3)]
        );
        let many: String = (1..=500).map(|i| format!("{i},")).collect();
        assert_eq!(parse_ids(&many).len(), BATCH_MAX);
    }
}
