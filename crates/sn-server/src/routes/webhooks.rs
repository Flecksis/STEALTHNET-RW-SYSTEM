use crate::error::{ApiError, ApiResult};
use crate::extract::client_ip;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
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
            post(payment_webhook)
                .get(payment_webhook)
                .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY)),
        )
        .route(
            "/api/webhooks/pay/{provider}/{kind}",
            post(payment_webhook_kind)
                .get(payment_webhook_kind)
                .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY)),
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

fn payload(method: &Method, query: Option<&str>, body: Bytes) -> Bytes {
    if method == Method::GET || method == Method::HEAD {
        Bytes::from(query.unwrap_or_default().to_owned())
    } else {
        body
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
    method: Method,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let body = payload(&method, query.as_deref(), body);
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
    method: Method,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let body = payload(&method, query.as_deref(), body);
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
    use sn_domain::client::{Locale, RegistrationSource};
    use sn_domain::money::{Currency, Money};
    use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
    use sn_domain::payment::{NewPayment, ProviderId, providers};
    use sn_domain::pricing::Quote;
    use sn_domain::settings::{keys, secret_keys};
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_payments::providers::{cloudpayments_signature, cryptopay_signature};
    use sn_services::{EnvFallbacks, ServiceDeps, Services, SystemClock};
    use sn_store::{InboxOutcome, NewClient, SecretBox, Store, StoreOptions};
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

    async fn get_status(path: &str) -> StatusCode {
        router(offline())
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn платёжное_уведомление_методом_get_доходит_до_разбора() {
        for path in [
            "/api/webhooks/pay/platega?id=1",
            "/api/webhooks/pay/cloudpayments/check?TransactionId=1504",
        ] {
            assert_eq!(get_status(path).await, StatusCode::NOT_FOUND, "{path}");
        }
        assert_eq!(
            payload(&Method::GET, Some("a=1&b=%20"), Bytes::from_static(b"x")),
            Bytes::from_static(b"a=1&b=%20")
        );
        assert_eq!(
            payload(&Method::POST, Some("a=1"), Bytes::from_static(b"x")),
            Bytes::from_static(b"x")
        );
        assert!(payload(&Method::GET, None, Bytes::new()).is_empty());
    }

    #[tokio::test]
    async fn уведомления_cloudpayments_методом_get_засчитывают_оплату() {
        let Some(db) = sn_store::testing::test_store().await else {
            return;
        };
        let secret = "cp-api-secret";
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::catalog_only()),
            secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
            clock: Arc::new(SystemClock),
            env: EnvFallbacks::default(),
        });
        let total = Money::new(49_900, Currency::Rub);
        let payment = {
            let mut conn = db.store.conn().await.unwrap();
            conn.settings()
                .set(
                    &keys::provider(providers::CLOUDPAYMENTS),
                    &serde_json::json!({ "enabled": true, "config": { "public_id": "pk_get_test" } }),
                    None,
                )
                .await
                .unwrap();
            sn_services::settings::write_secret(
                &mut conn,
                services.secrets(),
                &secret_keys::provider(providers::CLOUDPAYMENTS, "api_secret"),
                Some(secret),
                None,
            )
            .await
            .unwrap();
            let client = conn
                .clients()
                .create(&NewClient {
                    locale: Locale::Ru,
                    registration_source: RegistrationSource::Bot,
                    note: None,
                })
                .await
                .unwrap()
                .id;
            let order = conn
                .orders()
                .create(&NewOrder {
                    client_id: client,
                    subscription_id: None,
                    kind: OrderKind::TopUp,
                    intent: PurchaseIntent::TopUp { amount: total },
                    quote: Quote {
                        currency: Currency::Rub,
                        lines: Vec::new(),
                        subtotal: total,
                        discount: Money::zero(Currency::Rub),
                        total,
                        stars: None,
                        days: 0,
                        extra_devices: 0,
                    },
                    snapshot: None,
                })
                .await
                .unwrap();
            conn.payments()
                .create(&NewPayment {
                    order_id: order.id,
                    client_id: client,
                    provider: ProviderId::from(providers::CLOUDPAYMENTS),
                    method: None,
                    amount: total.into(),
                    expires_at: None,
                })
                .await
                .unwrap()
        };
        services.reload().await.unwrap();
        let app = router(AppState::new(ServerConfig::default(), services.clone()));
        let check = format!(
            "TransactionId=1504&Amount=499.00&Currency=RUB&DateTime=2026-11-18+10%3A00%3A00&TestMode=0&Status=Completed&OperationType=Payment&InvoiceId={}&Email=client%40example.com",
            payment.public_id
        );
        let pay = format!("{check}&GatewayName=Test&AuthCode=A1B2C3&TotalFee=0");
        let decoded_pay = pay
            .replace('+', " ")
            .replace("%3A", ":")
            .replace("%40", "@");
        let send = |kind: &'static str, query: String, header: &'static str, signed: String| {
            let app = app.clone();
            async move {
                let response = app
                    .oneshot(
                        Request::get(format!("/api/webhooks/pay/cloudpayments/{kind}?{query}"))
                            .header(header, cloudpayments_signature(secret, signed.as_bytes()))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let status = response.status();
                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                (
                    status,
                    serde_json::from_slice::<serde_json::Value>(&body).ok(),
                )
            }
        };
        let code_zero = Some(serde_json::json!({ "code": 0 }));
        assert_eq!(
            send("check", check.clone(), "Content-HMAC", check.clone()).await,
            (StatusCode::OK, code_zero.clone())
        );
        for _ in 0..2 {
            assert_eq!(
                send("pay", pay.clone(), "X-Content-HMAC", decoded_pay.clone()).await,
                (StatusCode::OK, code_zero.clone())
            );
        }
        assert_eq!(
            send("pay", pay.clone(), "Content-HMAC", decoded_pay.clone())
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let mut conn = db.store.conn().await.unwrap();
        assert_eq!(
            conn.payments().get(payment.id).await.unwrap().status,
            PaymentStatus::Paid
        );
        let journal: Vec<(InboxOutcome, bool)> = conn
            .webhooks()
            .recent(10)
            .await
            .unwrap()
            .into_iter()
            .rev()
            .map(|entry| (entry.outcome, entry.has_body))
            .collect();
        assert_eq!(
            journal,
            vec![
                (InboxOutcome::Ignored, true),
                (InboxOutcome::Accepted, true),
                (InboxOutcome::RejectedSignature, false),
            ]
        );
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn уведомление_выключенной_системы_принимается_а_ненастроенной_нет() {
        let Some(db) = sn_store::testing::test_store().await else {
            return;
        };
        let token = "4321:TEST";
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::catalog_only()),
            secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
            clock: Arc::new(SystemClock),
            env: EnvFallbacks::default(),
        });
        let total = Money::new(500, Currency::Usd);
        let payment = {
            let mut conn = db.store.conn().await.unwrap();
            conn.settings()
                .set(
                    &keys::provider(providers::CRYPTOPAY),
                    &serde_json::json!({ "enabled": false }),
                    None,
                )
                .await
                .unwrap();
            sn_services::settings::write_secret(
                &mut conn,
                services.secrets(),
                &secret_keys::provider(providers::CRYPTOPAY, "api_token"),
                Some(token),
                None,
            )
            .await
            .unwrap();
            let client = conn
                .clients()
                .create(&NewClient {
                    locale: Locale::Ru,
                    registration_source: RegistrationSource::Bot,
                    note: None,
                })
                .await
                .unwrap()
                .id;
            let order = conn
                .orders()
                .create(&NewOrder {
                    client_id: client,
                    subscription_id: None,
                    kind: OrderKind::TopUp,
                    intent: PurchaseIntent::TopUp { amount: total },
                    quote: Quote {
                        currency: Currency::Usd,
                        lines: Vec::new(),
                        subtotal: total,
                        discount: Money::zero(Currency::Usd),
                        total,
                        stars: None,
                        days: 0,
                        extra_devices: 0,
                    },
                    snapshot: None,
                })
                .await
                .unwrap();
            conn.payments()
                .create(&NewPayment {
                    order_id: order.id,
                    client_id: client,
                    provider: ProviderId::from(providers::CRYPTOPAY),
                    method: None,
                    amount: total.into(),
                    expires_at: None,
                })
                .await
                .unwrap()
        };
        services.reload().await.unwrap();
        assert!(
            !services
                .payments()
                .is_available(providers::CRYPTOPAY, Currency::Usd)
        );
        let app = router(AppState::new(ServerConfig::default(), services.clone()));
        let body = serde_json::json!({
            "update_id": 1, "update_type": "invoice_paid", "request_date": "2026-11-17T09:00:00.000Z",
            "payload": {"invoice_id": 9001, "status": "paid", "currency_type": "fiat", "fiat": "USD",
                        "amount": "5.00", "payload": payment.public_id.to_string()}
        })
        .to_string();
        let accepted = app
            .clone()
            .oneshot(
                Request::post("/api/webhooks/pay/cryptopay")
                    .header(
                        "crypto-pay-api-signature",
                        cryptopay_signature(token, body.as_bytes()),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);
        let unconfigured = app
            .oneshot(
                Request::post("/api/webhooks/pay/platega")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unconfigured.status(), StatusCode::NOT_FOUND);
        let mut conn = db.store.conn().await.unwrap();
        assert_eq!(
            conn.payments().get(payment.id).await.unwrap().status,
            PaymentStatus::Paid
        );
        let journal: Vec<(String, InboxOutcome, bool)> = conn
            .webhooks()
            .recent(10)
            .await
            .unwrap()
            .into_iter()
            .rev()
            .map(|entry| (entry.source, entry.outcome, entry.has_body))
            .collect();
        assert_eq!(
            journal,
            vec![
                (
                    providers::CRYPTOPAY.to_string(),
                    InboxOutcome::Accepted,
                    true
                ),
                (
                    providers::PLATEGA.to_string(),
                    InboxOutcome::NotAccepted,
                    false
                ),
            ]
        );
        drop(conn);
        db.cleanup().await;
    }
}
