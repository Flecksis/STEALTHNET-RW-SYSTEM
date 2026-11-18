use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_services::ServiceError;
use sn_services::remna_transfer::{Direction, StartOptions, SyncView, TransferView};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/remna-sync", get(view))
        .route("/remna-sync/check", post(check))
        .route("/remna-sync/{id}", get(run).delete(discard))
        .route("/remna-sync/{id}/start", post(start))
        .route("/remna-sync/{id}/pause", post(pause))
        .route("/remna-sync/{id}/notify", post(notify))
        .route("/remna-sync/{id}/report", get(report))
}

fn allowed(admin: &CurrentAdmin) -> ApiResult<()> {
    admin.require_session()?;
    admin.require(Permission::RemnaSync)
}

fn mapped(err: ServiceError) -> ApiError {
    let busy = err.code() == "remna_sync.busy";
    let mut api = ApiError::from(err);
    if busy {
        api.status = StatusCode::CONFLICT;
    }
    api
}

async fn view(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<SyncView>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .remna_transfer()
            .view()
            .await
            .map_err(mapped)?,
    ))
}

async fn run(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TransferView>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .remna_transfer()
            .get(id)
            .await
            .map_err(mapped)?,
    ))
}

#[derive(Debug, Deserialize)]
struct CheckBody {
    direction: String,
}

async fn check(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CheckBody>,
) -> ApiResult<Json<TransferView>> {
    allowed(&admin)?;
    let Some(direction) = Direction::parse(&body.direction) else {
        return Err(ApiError::bad_request(
            "remna_sync.direction",
            "Выберите направление: в Remnawave или из Remnawave",
        ));
    };
    Ok(Json(
        state
            .services
            .remna_transfer()
            .check(direction, Some(admin.0.admin_id))
            .await
            .map_err(mapped)?,
    ))
}

async fn start(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> ApiResult<Json<TransferView>> {
    allowed(&admin)?;
    let options: StartOptions = if body.iter().all(u8::is_ascii_whitespace) {
        StartOptions::default()
    } else {
        serde_json::from_slice(&body).map_err(|_| {
            ApiError::from(ServiceError::rejected(
                "remna_sync.options",
                "Не удалось прочитать выбор для переноса",
            ))
        })?
    };
    Ok(Json(
        state
            .services
            .remna_transfer()
            .start(id, options, Some(admin.0.admin_id))
            .await
            .map_err(mapped)?,
    ))
}

async fn pause(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TransferView>> {
    allowed(&admin)?;
    Ok(Json(
        state
            .services
            .remna_transfer()
            .pause(id)
            .await
            .map_err(mapped)?,
    ))
}

async fn discard(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    allowed(&admin)?;
    state
        .services
        .remna_transfer()
        .discard(id)
        .await
        .map_err(mapped)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct Notified {
    sent: u64,
}

async fn notify(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Notified>> {
    allowed(&admin)?;
    let sent = state
        .services
        .remna_transfer()
        .notify_links(id, Some(admin.0.admin_id))
        .await
        .map_err(mapped)?;
    Ok(Json(Notified { sent }))
}

async fn report(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    allowed(&admin)?;
    let (name, bytes) = state
        .services
        .remna_transfer()
        .report(id)
        .await
        .map_err(mapped)?;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"));
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/csv; charset=utf-8"),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}
