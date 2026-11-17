use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::ids::PaymentId;
use sn_domain::money::Amount;
use sn_domain::payment::{Payment, PaymentStatus, providers};
use sn_domain::settings::keys;
use sn_payments::providers::GramInvoice;
use sn_services::ServiceError;
use sn_store::Db;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

const POLL_EVERY: Duration = Duration::from_secs(10);
const POLL_TIMEOUT: Duration = Duration::from_secs(8);
const REMEMBERED_MAX: usize = 2_000;

static LAST_POLL: LazyLock<Mutex<HashMap<PaymentId, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/public/pay/{payment}", get(status))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PayState {
    Pending,
    Paid,
    Expired,
    Closed,
}

#[derive(Debug, Serialize)]
struct GramDetails {
    amount: String,
    nano: String,
    address: String,
    comment: String,
    ton_link: String,
    tonkeeper_link: String,
}

#[derive(Debug, Serialize)]
struct Back {
    bot: Option<String>,
    cabinet: Option<String>,
}

#[derive(Debug, Serialize)]
struct PayView {
    status: PayState,
    provider: &'static str,
    amount: Amount,
    expires_at: Option<DateTime<Utc>>,
    gram: GramDetails,
    back: Back,
}

fn state_of(payment: &Payment, now: DateTime<Utc>) -> PayState {
    if payment.status.is_paid_like() {
        PayState::Paid
    } else if payment.status != PaymentStatus::Pending {
        PayState::Closed
    } else if payment.expires_at.is_some_and(|at| at <= now) {
        PayState::Expired
    } else {
        PayState::Pending
    }
}

fn due_for_poll(id: PaymentId) -> bool {
    let mut guard = match LAST_POLL.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.len() > REMEMBERED_MAX {
        guard.retain(|_, at| at.elapsed() < POLL_EVERY * 6);
    }
    match guard.get(&id) {
        Some(at) if at.elapsed() < POLL_EVERY => false,
        _ => {
            guard.insert(id, Instant::now());
            true
        }
    }
}

async fn load(state: &AppState, payment: Uuid) -> ApiResult<Payment> {
    let mut conn = state
        .services
        .store()
        .conn()
        .await
        .map_err(ServiceError::from)?;
    conn.payments()
        .by_public_id(payment)
        .await
        .map_err(|_| ApiError::not_found())
}

async fn status(
    State(state): State<AppState>,
    Path(payment): Path<Uuid>,
) -> ApiResult<Json<PayView>> {
    let mut found = load(&state, payment).await?;
    if found.provider.as_str() != providers::GRAM {
        return Err(ApiError::not_found());
    }
    let Some(invoice) = found.provider_txid.as_deref().and_then(GramInvoice::parse) else {
        return Err(ApiError::not_found());
    };
    let now = state.services.now();
    if state_of(&found, now) == PayState::Pending && due_for_poll(found.id) {
        match tokio::time::timeout(POLL_TIMEOUT, state.services.inbox().poll_payment(found.id))
            .await
        {
            Ok(Ok(_)) => found = load(&state, payment).await?,
            Ok(Err(err)) => {
                tracing::debug!(payment = %found.id, error = %err, "оплата GRAM не проверена")
            }
            Err(_) => {
                tracing::debug!(payment = %found.id, "проверка оплаты GRAM не уложилась во время")
            }
        }
    }
    let settings = state.services.settings();
    let bot = match state.services.store().conn().await {
        Ok(mut conn) => conn
            .settings()
            .get(keys::BOT_USERNAME)
            .await
            .ok()
            .flatten()
            .and_then(|value: serde_json::Value| value.as_str().map(str::to_string))
            .filter(|name| !name.trim().is_empty())
            .map(|name| format!("https://t.me/{}", name.trim_start_matches('@'))),
        Err(_) => None,
    };
    let cabinet = settings
        .public_url
        .as_deref()
        .map(|base| format!("{}/cabinet", base.trim_end_matches('/')));
    Ok(Json(PayView {
        status: state_of(&found, state.services.now()),
        provider: providers::GRAM,
        amount: found.amount,
        expires_at: found.expires_at,
        gram: GramDetails {
            amount: invoice.grams(),
            nano: invoice.nano.to_string(),
            ton_link: invoice.ton_link(),
            tonkeeper_link: invoice.tonkeeper_link(),
            address: invoice.address,
            comment: invoice.comment,
        },
        back: Back { bot, cabinet },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn проверка_не_чаще_раза_в_десять_секунд() {
        let id = PaymentId(987_654_321);
        assert!(due_for_poll(id));
        assert!(!due_for_poll(id));
        assert!(due_for_poll(PaymentId(987_654_322)));
    }
}
