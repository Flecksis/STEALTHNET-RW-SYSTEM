use super::{Paging, json};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::{ClientId, SubscriptionId};
use sn_domain::money::Money;
use sn_domain::subscription::admin_status;
use sn_services::clients::HistoryEntry;
use sn_store::{ClientFilter, ClientListItem, ClientSegment};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clients", get(list))
        .route("/clients/segments", get(segments))
        .route("/clients/{id}", get(card))
        .route("/clients/{id}/history", get(history))
        .route("/clients/{id}/note", put(note))
        .route("/clients/{id}/balance", post(balance))
        .route("/clients/{id}/block", post(block))
        .route("/clients/{id}/unblock", post(unblock))
        .route("/subscriptions/{id}", get(subscription))
        .route(
            "/subscriptions/{id}/actions/{action}",
            post(subscription_action),
        )
}

pub(crate) fn segment(value: &str) -> Option<ClientSegment> {
    Some(match value {
        "active" => ClientSegment::Active,
        "trial" => ClientSegment::Trial,
        "no_subscription" => ClientSegment::NoSubscription,
        "expiring_soon" => ClientSegment::ExpiringSoon,
        "never_connected" => ClientSegment::NeverConnected,
        "expired" => ClientSegment::Expired,
        "limited" => ClientSegment::Limited,
        "blocked" => ClientSegment::Blocked,
        _ => return None,
    })
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    segment: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

fn list_item(
    item: &ClientListItem,
    state: &AppState,
    money: bool,
    tariffs: &std::collections::HashMap<sn_domain::ids::TariffId, String>,
) -> serde_json::Value {
    let currency = state.services.settings().currency;
    let tariff_name = item
        .subscription
        .as_ref()
        .and_then(|s| s.tariff_id)
        .and_then(|id| tariffs.get(&id).cloned());
    serde_json::json!({
        "tariff_name": tariff_name,
        "client": item.client,
        "name": item.name,
        "telegram_id": item.telegram_id,
        "telegram_username": item.telegram_username,
        "email": item.email,
        "status": admin_status(item.subscription.as_ref(), item.client.is_blocked(), state.services.now()),
        "subscription": item.subscription,
        "balance": money.then(|| Money::new(item.balance_minor, currency)),
        "paid_total": money.then(|| Money::new(item.paid_total_minor, currency)),
    })
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsRead)?;
    let segment = match query
        .segment
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all")
    {
        Some(value) => Some(segment(value).ok_or_else(|| {
            ApiError::bad_request("validation.segment", format!("неизвестный сегмент {value}"))
        })?),
        None => None,
    };
    let filter = ClientFilter {
        segment,
        query: query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| q.chars().take(200).collect()),
    };
    let paging = Paging {
        limit: query.limit,
        after: query.after,
    };
    let page = state
        .services
        .clients()
        .search(&filter, paging.request())
        .await?;
    let money = admin.can(Permission::Money);
    let tariffs = state.services.clients().tariff_names().await?;
    let items: Vec<serde_json::Value> = page
        .items
        .iter()
        .map(|item| list_item(item, &state, money, &tariffs))
        .collect();
    Ok(Json(
        serde_json::json!({ "items": items, "next": page.next }),
    ))
}

async fn segments(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_store::SegmentCounts>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(state.services.clients().segment_counts().await?))
}

async fn card(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsRead)?;
    let card = state.services.clients().card(ClientId(id)).await?;
    let mut value = json(&card)?;
    if !admin.can(Permission::Money)
        && let Some(object) = value.as_object_mut()
    {
        object.remove("balance");
        object.remove("referral_balance");
    }
    Ok(Json(value))
}

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(default)]
    limit: Option<u32>,
}

async fn history(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
) -> ApiResult<Json<Vec<HistoryEntry>>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(
        state
            .services
            .clients()
            .history(
                ClientId(id),
                query.limit.unwrap_or(50),
                admin.can(Permission::Money),
            )
            .await?,
    ))
}

#[derive(Deserialize)]
struct NoteRequest {
    note: Option<String>,
}

async fn note(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<NoteRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    state
        .services
        .clients()
        .set_note(ClientId(id), body.note.as_deref(), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct BalanceRequest {
    delta: Money,
    comment: String,
    idempotency_key: String,
}

async fn balance(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<BalanceRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Money)?;
    if body.comment.trim().is_empty() || body.idempotency_key.trim().is_empty() {
        return Err(ApiError::bad_request(
            "validation.balance",
            "укажите причину изменения баланса",
        ));
    }
    let balance = state
        .services
        .clients()
        .adjust_balance(
            ClientId(id),
            body.delta,
            body.comment.trim(),
            admin.0.admin_id,
            body.idempotency_key.trim(),
        )
        .await?;
    Ok(Json(serde_json::json!({ "balance": balance })))
}

#[derive(Deserialize, Default)]
struct BlockRequest {
    #[serde(default)]
    reason: Option<String>,
}

async fn block(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<BlockRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    state
        .services
        .clients()
        .block(ClientId(id), body.reason.as_deref(), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn unblock(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    state
        .services
        .clients()
        .unblock(ClientId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn subscription(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<sn_services::subscriptions::SubscriptionView>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(
        state
            .services
            .subscriptions()
            .view(SubscriptionId(id), admin.actor(), true)
            .await?,
    ))
}

#[derive(Deserialize, Default)]
struct ActionRequest {
    #[serde(default)]
    days: Option<u32>,
    #[serde(default)]
    hwid: Option<String>,
    #[serde(default)]
    limits: Option<sn_services::subscriptions::LimitsChange>,
}

async fn subscription_action(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path((id, action)): Path<(i64, String)>,
    body: Option<ApiJson<ActionRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    let ops = state.services.subscriptions();
    let actor = admin.actor();
    let id = SubscriptionId(id);
    let result = match action.as_str() {
        "extend" => {
            let days = body.days.unwrap_or(7);
            if !(1..=3650).contains(&days) {
                return Err(ApiError::bad_request(
                    "validation.days",
                    "продлить можно на 1–3650 дней",
                ));
            }
            json(&ops.add_days(id, days, actor).await?)
        }
        "reset-traffic" => json(&ops.reset_traffic(id, actor).await?),
        "reissue-link" => json(&ops.reissue_link(id, actor).await?),
        "disconnect-devices" => json(&ops.disconnect_all_devices(id, actor).await?),
        "disconnect-device" => {
            let hwid = body
                .hwid
                .filter(|h| !h.trim().is_empty())
                .ok_or_else(|| ApiError::bad_request("validation.hwid", "не указано устройство"))?;
            json(&ops.disconnect_device(id, &hwid, actor).await?)
        }
        "devices" => json(&ops.devices(id, actor).await?),
        "limits" => {
            let change = body.limits.ok_or_else(|| {
                ApiError::bad_request("validation.limits", "не указаны новые лимиты")
            })?;
            json(&ops.change_limits(id, &change, actor).await?)
        }
        _ => return Err(ApiError::not_found()),
    };
    result.map(Json)
}
