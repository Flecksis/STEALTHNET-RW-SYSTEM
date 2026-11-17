use crate::error::{ApiError, ApiResult};
use crate::extract::client_ip;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use sn_domain::payment::PaymentStatus;
use sn_payments::{WebhookReply, WebhookRequest};
use sn_remnawave::webhook::SIGNATURE_HEADER;
use sn_services::inbox::{InboundWebhook, MAX_WEBHOOK_BODY};
use sn_services::remna_sync::RemnaWebhookResult;
use sn_store::Db;
use std::net::SocketAddr;
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/webhooks/pay/{provider}",
            post(payment_webhook).layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY)),
        )
        .route(
            "/api/webhooks/pay/{provider}/{kind}",
            post(payment_webhook_kind).layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY)),
        )
        .route("/api/remnawave/webhook", post(remnawave_webhook))
        .route("/api/telegram/webhook", post(telegram_webhook))
        .route("/api/pay/{payment}", get(pay_redirect))
}

fn reply(reply: WebhookReply) -> Response {
    let status = |code: u16| StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
    match reply {
        WebhookReply::Empty { status: code } => status(code).into_response(),
        WebhookReply::Text { status: code, body } => (status(code), body).into_response(),
        WebhookReply::Json { status: code, body } => (status(code), Json(body)).into_response(),
    }
}

async fn handle_payment(
    state: AppState,
    provider: String,
    route_suffix: Option<String>,
    peer: Option<SocketAddr>,
    query: Option<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let request = WebhookRequest {
        headers: headers
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_string(), v.to_string()))
            })
            .collect(),
        body,
        remote_ip: client_ip(
            peer.map(|p| p.ip()),
            &headers,
            &state.config.trusted_proxies,
        ),
        route_suffix,
        query,
    };
    let response = state
        .services
        .inbox()
        .handle(InboundWebhook { provider, request })
        .await?;
    Ok(reply(response.reply))
}

async fn payment_webhook(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    handle_payment(
        state,
        provider,
        None,
        peer.map(|p| p.0.0),
        query,
        headers,
        body,
    )
    .await
}

async fn payment_webhook_kind(
    State(state): State<AppState>,
    Path((provider, kind)): Path<(String, String)>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    handle_payment(
        state,
        provider,
        Some(kind),
        peer.map(|p| p.0.0),
        query,
        headers,
        body,
    )
    .await
}

async fn remnawave_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    if body.len() > sn_remnawave::webhook::MAX_BODY_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "webhook.too_large",
            "слишком большое тело",
        ));
    }
    let signature = headers.get(SIGNATURE_HEADER).and_then(|v| v.to_str().ok());
    let result = state
        .services
        .remna_sync()
        .accept_webhook(signature, &body)
        .await?;
    Ok(match result {
        RemnaWebhookResult::Accepted { .. } | RemnaWebhookResult::Duplicate => {
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        RemnaWebhookResult::NotConfigured => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "remnawave.webhook_secret",
            "секрет вебхуков Remnawave не задан",
        )
        .into_response(),
        RemnaWebhookResult::BadSignature => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "remnawave.bad_signature",
            "подпись не совпала",
        )
        .into_response(),
        RemnaWebhookResult::Malformed { reason } => {
            ApiError::bad_request("remnawave.malformed", reason).into_response()
        }
    })
}

async fn telegram_webhook() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "telegram.webhook_disabled",
        "бот получает обновления через long polling, вебхук Telegram выключен",
    )
}

async fn pay_redirect(
    State(state): State<AppState>,
    Path(payment): Path<Uuid>,
) -> ApiResult<Response> {
    let mut conn = state
        .services
        .store()
        .conn()
        .await
        .map_err(sn_services::ServiceError::from)?;
    let payment = conn
        .payments()
        .by_public_id(payment)
        .await
        .map_err(sn_services::ServiceError::from)?;
    match (payment.status, payment.pay_url) {
        (PaymentStatus::Pending, Some(url)) => {
            let mut response = Redirect::to(&url).into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            Ok(response)
        }
        (PaymentStatus::Pending, None) => Err(ApiError::not_found()),
        _ => Err(ApiError::new(
            StatusCode::GONE,
            "payment.closed",
            "счёт уже оплачен или отменён",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ServerConfig, router};
    use axum::body::Body;
    use axum::http::Request;
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_services::{EnvFallbacks, ServiceDeps, Services, SystemClock};
    use sn_store::{Store, StoreOptions};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn offline() -> AppState {
        let store = Store::connect_lazy(
            "postgres://sn:sn@127.0.0.1:1/sn_test",
            StoreOptions {
                acquire_timeout: std::time::Duration::from_millis(300),
                ..StoreOptions::default()
            },
        )
        .unwrap();
        AppState::new(
            ServerConfig::default(),
            Services::new(ServiceDeps {
                store,
                payments: Arc::new(ProviderRegistry::builtin()),
                modules: Arc::new(ModuleRegistry::catalog_only()),
                secrets: None,
                clock: Arc::new(SystemClock),
                env: EnvFallbacks::default(),
            }),
        )
    }

    async fn post(path: &str, body: Vec<u8>) -> StatusCode {
        router(offline())
            .oneshot(Request::post(path).body(Body::from(body)).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn большое_тело_платёжного_вебхука_отклоняется_до_разбора() {
        for path in [
            "/api/webhooks/pay/platega",
            "/api/webhooks/pay/cloudpayments/pay",
        ] {
            assert_eq!(
                post(path, vec![b' '; MAX_WEBHOOK_BODY + 1]).await,
                StatusCode::PAYLOAD_TOO_LARGE,
                "{path}"
            );
        }
        assert_eq!(
            post("/api/webhooks/pay/platega", b"{}".to_vec()).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            post("/api/webhooks/pay/overpay", vec![b' '; 1024]).await,
            StatusCode::NOT_FOUND
        );
    }
}
