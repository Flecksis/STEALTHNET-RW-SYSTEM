use crate::error::{ApiError, ApiResult};
use crate::extract::CurrentClient;
use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use sn_services::ServiceError;
use sn_store::Db;

pub fn routes() -> Router<AppState> {
    Router::new().route("/photo", get(photo))
}

async fn photo(client: CurrentClient, State(state): State<AppState>) -> ApiResult<Response> {
    let id = client.0.client_id;
    let mut conn = state
        .services
        .store()
        .conn()
        .await
        .map_err(ServiceError::from)?;
    let mut photos = conn.client_photos();
    match photos.get(id).await.map_err(ServiceError::from)? {
        Some(photo) => super::admin::photos::image(photo, "private, max-age=3600"),
        None => {
            if let Err(err) = photos.request_missing(&[id]).await {
                tracing::debug!(error = %err, "запрос фото клиента не сохранён");
            }
            Err(ApiError::not_found())
        }
    }
}
