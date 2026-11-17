use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::{ClientId, PaymentId};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{Payment, PaymentStatus};
use sn_services::admin::{MarkPaidRequest, MarkPaidResult, PaymentFilter, RefundRequest};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/payments", get(list))
        .route("/payments/{id}/mark-paid", post(mark_paid))
        .route("/payments/{id}/refund", post(refund))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    client_id: Option<i64>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> ApiResult<Json<Page<Payment>>> {
    admin.require(Permission::Money)?;
    let status = match query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(value) => Some(PaymentStatus::parse(value).map_err(|_| {
            ApiError::bad_request("validation.status", format!("неизвестный статус {value}"))
        })?),
        None => None,
    };
    let filter = PaymentFilter {
        status,
        provider: query.provider,
        client_id: query.client_id.map(ClientId),
    };
    Ok(Json(
        state
            .services
            .admin()
            .payments(&filter, PageRequest::new(query.limit, query.after))
            .await?,
    ))
}

async fn mark_paid(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<MarkPaidRequest>>,
) -> ApiResult<Json<MarkPaidResult>> {
    admin.require(Permission::Money)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(
        state
            .services
            .admin()
            .mark_paid(PaymentId(id), &body, admin.0.admin_id)
            .await?,
    ))
}

async fn refund(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<RefundRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let refund = state
        .services
        .admin()
        .refund(PaymentId(id), &body, admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "refund": refund })))
}
