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
use sn_services::inbox::{PendingRefundRequest, RefundActionReview};

const REFUND_LIST_LIMIT: i64 = 100;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/payments", get(list))
        .route("/payments/{id}/mark-paid", post(mark_paid))
        .route("/payments/{id}/refund", post(refund))
        .route("/refunds/pending", get(pending_refunds))
        .route("/refunds/actions", get(refund_actions))
        .route("/refunds/{id}/cancel", post(cancel_refund_request))
        .route("/refunds/{id}/confirm", post(confirm_refund_request))
        .route("/refunds/{id}/action/retry", post(retry_refund_action))
        .route("/refunds/{id}/action/close", post(close_refund_action))
}

#[derive(Debug, Default, Deserialize)]
struct LimitQuery {
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct NoteRequest {
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ReasonRequest {
    #[serde(default)]
    reason: Option<String>,
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

async fn pending_refunds(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<LimitQuery>,
) -> ApiResult<Json<Vec<PendingRefundRequest>>> {
    admin.require(Permission::Refunds)?;
    Ok(Json(
        state
            .services
            .inbox()
            .pending_refund_requests(query.limit.unwrap_or(REFUND_LIST_LIMIT))
            .await?,
    ))
}

async fn refund_actions(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<LimitQuery>,
) -> ApiResult<Json<Vec<RefundActionReview>>> {
    admin.require(Permission::Refunds)?;
    Ok(Json(
        state
            .services
            .inbox()
            .refund_actions_review(query.limit.unwrap_or(REFUND_LIST_LIMIT))
            .await?,
    ))
}

async fn cancel_refund_request(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<ReasonRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let reason = body.and_then(|b| b.0.reason).unwrap_or_default();
    state
        .services
        .inbox()
        .cancel_refund_request(id, admin.0.admin_id, &reason)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "request_id": id })))
}

async fn confirm_refund_request(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<NoteRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let note = body.and_then(|b| b.0.note).unwrap_or_default();
    let result = state
        .services
        .inbox()
        .confirm_refund_request(id, admin.0.admin_id, &note)
        .await?;
    Ok(Json(
        serde_json::json!({ "ok": true, "request_id": id, "result": result }),
    ))
}

async fn retry_refund_action(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let outcome = state
        .services
        .inbox()
        .retry_refund_action(id, admin.0.admin_id)
        .await?;
    Ok(Json(
        serde_json::json!({ "ok": true, "request_id": id, "outcome": outcome }),
    ))
}

async fn close_refund_action(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<NoteRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let note = body.and_then(|b| b.0.note).unwrap_or_default();
    state
        .services
        .inbox()
        .close_refund_action(id, admin.0.admin_id, &note)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "request_id": id })))
}
