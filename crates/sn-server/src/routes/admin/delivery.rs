use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::ids::{EventId, JobId, OrderId};
use sn_domain::jobs::JobPayload;
use sn_domain::module::ModuleId;
use sn_domain::order::OrderStatus;
use sn_services::ServiceError;
use sn_store::{Db, Delivery, audit_entity};

const REVIEW_LIMIT: i64 = 50;
const REVIEW_LIMIT_MAX: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orders/{id}/redeliver", post(redeliver))
        .route("/events/review", get(events_review))
        .route("/events/{id}/retry", post(event_retry))
}

#[derive(Debug, Deserialize)]
struct ReviewQuery {
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RetryRequest {
    module: ModuleId,
}

#[derive(Debug, Serialize)]
struct EventRetry {
    ok: bool,
    event_id: EventId,
    module: ModuleId,
}

async fn events_review(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ReviewQuery>,
) -> ApiResult<Json<Vec<Delivery>>> {
    admin.require(Permission::Modules)?;
    let limit = query
        .limit
        .unwrap_or(REVIEW_LIMIT)
        .clamp(1, REVIEW_LIMIT_MAX);
    Ok(Json(state.services.events().review(limit).await?))
}

async fn event_retry(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<RetryRequest>,
) -> ApiResult<Json<EventRetry>> {
    admin.require(Permission::Modules)?;
    let event_id = EventId(id);
    if !state.services.events().retry(event_id, body.module).await? {
        return Err(ServiceError::rejected(
            "event.retry_status",
            "Доставка этого события модулю не ждёт ручной проверки",
        )
        .into());
    }
    let mut conn = state
        .services
        .store()
        .conn()
        .await
        .map_err(ServiceError::from)?;
    conn.audit()
        .record(&admin.actor().audit(
            "module.event_retry",
            "domain_event",
            Some(event_id.0),
            serde_json::json!({ "module": body.module }),
        ))
        .await
        .map_err(ServiceError::from)?;
    Ok(Json(EventRetry {
        ok: true,
        event_id,
        module: body.module,
    }))
}

#[derive(Debug, Serialize)]
struct Redelivery {
    ok: bool,
    order_id: OrderId,
    job_id: JobId,
    state: &'static str,
}

fn not_deliverable(status: OrderStatus) -> ServiceError {
    let message = match status {
        OrderStatus::Fulfilled => "Заказ уже выдан",
        OrderStatus::Refunded => "Заказ возвращён, выдавать его нельзя",
        OrderStatus::Pending | OrderStatus::Canceled => "Заказ не оплачен",
        OrderStatus::Paid | OrderStatus::Review => "Заказ нельзя выдать повторно",
    };
    ServiceError::rejected("order.redeliver_status", message)
}

async fn redeliver(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Redelivery>> {
    admin.require(Permission::Money)?;
    let mut tx = state
        .services
        .store()
        .begin()
        .await
        .map_err(ServiceError::from)?;
    let order = tx
        .orders()
        .lock(OrderId(id))
        .await
        .map_err(ServiceError::from)?;
    if !matches!(order.status, OrderStatus::Paid | OrderStatus::Review) {
        return Err(not_deliverable(order.status).into());
    }
    let queued = tx
        .jobs()
        .ensure_queued(&JobPayload::FulfillOrder { order_id: order.id })
        .await
        .map_err(ServiceError::from)?;
    tx.audit()
        .record(&admin.actor().audit(
            "order.redeliver",
            audit_entity::ORDER,
            Some(order.id.0),
            serde_json::json!({
                "order_status": order.status,
                "job_id": queued.id(),
                "state": queued.state(),
            }),
        ))
        .await
        .map_err(ServiceError::from)?;
    tx.commit().await.map_err(ServiceError::from)?;
    Ok(Json(Redelivery {
        ok: true,
        order_id: order.id,
        job_id: queued.id(),
        state: queued.state(),
    }))
}

#[cfg(test)]
mod tests {
    use crate::{AppState, ServerConfig, router};
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode, header};
    use serde_json::{Value, json};
    use sn_domain::access::AdminRole;
    use sn_domain::client::{RegistrationSource, TelegramProfile};
    use sn_domain::events::DomainEvent;
    use sn_domain::ids::{ClientId, JobId, OrderId};
    use sn_domain::jobs::JobPayload;
    use sn_domain::module::ModuleId;
    use sn_domain::money::{Currency, Money};
    use sn_domain::order::{NewOrder, OrderKind, OrderStatus, PurchaseIntent};
    use sn_domain::pricing::Quote;
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_services::{EnvFallbacks, ServiceDeps, Services, SystemClock};
    use sn_store::testing::{TestDb, test_store};
    use sn_store::{Db, Delivery, DeliveryStatus, NewAdmin, SecretBox, audit_entity};
    use std::sync::Arc;
    use tower::ServiceExt;

    const PASSWORD: &str = "очень-надёжный-пароль";

    struct Harness {
        db: TestDb,
        state: AppState,
    }

    async fn harness() -> Option<Harness> {
        let db = test_store().await?;
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::catalog_only()),
            secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
            clock: Arc::new(SystemClock),
            env: EnvFallbacks::default(),
        });
        services.reload().await.unwrap();
        Some(Harness {
            db,
            state: AppState::new(ServerConfig::default(), services),
        })
    }

    impl Harness {
        fn services(&self) -> &Services {
            &self.state.services
        }

        async fn send(
            &self,
            method: Method,
            path: &str,
            token: Option<&str>,
            body: Option<Value>,
        ) -> (StatusCode, Value) {
            let mut builder = Request::builder().method(method).uri(path);
            if let Some(token) = token {
                builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            let request = match body {
                Some(body) => builder
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
                None => builder.body(Body::empty()).unwrap(),
            };
            let response = router(self.state.clone()).oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }

        async fn login(&self, email: &str) -> String {
            let (status, body) = self
                .send(
                    Method::POST,
                    "/api/admin/auth/login",
                    None,
                    Some(json!({ "email": email, "password": PASSWORD })),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body["token"].as_str().unwrap().to_string()
        }

        async fn owner(&self) -> String {
            self.services()
                .auth()
                .upsert_owner("owner@example.com", PASSWORD)
                .await
                .unwrap();
            self.login("owner@example.com").await
        }

        async fn staff(&self, email: &str, role: AdminRole) -> String {
            let hash = sn_services::auth::hash_password(PASSWORD).unwrap();
            {
                let mut conn = self.services().store().conn().await.unwrap();
                conn.admins()
                    .create(&NewAdmin {
                        email: email.into(),
                        password_hash: hash,
                        role,
                        must_change_password: false,
                    })
                    .await
                    .unwrap();
            }
            self.login(email).await
        }

        async fn client(&self, telegram_id: i64) -> ClientId {
            self.services()
                .clients()
                .ensure_telegram(
                    &TelegramProfile {
                        id: telegram_id,
                        username: None,
                        first_name: Some("Алексей".into()),
                        last_name: None,
                        language_code: Some("ru".into()),
                        is_premium: false,
                    },
                    RegistrationSource::Bot,
                    None,
                )
                .await
                .unwrap()
                .0
                .id
        }

        async fn paid_order(&self, client_id: ClientId) -> OrderId {
            let amount = Money::new(699, Currency::Usd);
            let mut tx = self.services().store().begin().await.unwrap();
            let order = tx
                .orders()
                .create(&NewOrder {
                    client_id,
                    subscription_id: None,
                    kind: OrderKind::TopUp,
                    intent: PurchaseIntent::TopUp { amount },
                    quote: Quote {
                        currency: Currency::Usd,
                        lines: Vec::new(),
                        subtotal: amount,
                        discount: Money::zero(Currency::Usd),
                        total: amount,
                        stars: None,
                        days: 0,
                        extra_devices: 0,
                    },
                    snapshot: None,
                })
                .await
                .unwrap();
            tx.orders()
                .transition(order.id, OrderStatus::Paid)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            order.id
        }

        async fn stuck_order(&self, client_id: ClientId) -> (OrderId, JobId) {
            let order_id = self.paid_order(client_id).await;
            let mut conn = self.services().store().conn().await.unwrap();
            let job = conn
                .jobs()
                .enqueue(&JobPayload::FulfillOrder { order_id }, None)
                .await
                .unwrap()
                .unwrap();
            let claimed = conn
                .jobs()
                .claim(1, std::time::Duration::from_secs(60))
                .await
                .unwrap();
            assert_eq!(claimed[0].id, job);
            conn.jobs().review(job, "попытки кончились").await.unwrap();
            (order_id, job)
        }

        async fn job_counts(&self) -> Vec<(String, i64)> {
            let mut conn = self.services().store().conn().await.unwrap();
            conn.jobs().counts().await.unwrap()
        }
    }

    #[tokio::test]
    async fn повтор_выдачи_ставит_задачу_снова_и_требует_права_деньги() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let support = h.staff("support@example.com", AdminRole::Support).await;
        let client_id = h.client(701).await;
        let (order_id, job_id) = h.stuck_order(client_id).await;
        let path = format!("/api/admin/orders/{}/redeliver", order_id.0);
        let (status, _) = h.send(Method::POST, &path, Some(&support), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(h.job_counts().await, vec![("review".to_string(), 1)]);
        let (status, body) = h.send(Method::POST, &path, Some(&owner), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "requeued");
        assert_eq!(body["job_id"], job_id.0);
        assert_eq!(body["order_id"], order_id.0);
        assert_eq!(h.job_counts().await, vec![("queued".to_string(), 1)]);
        let (status, body) = h.send(Method::POST, &path, Some(&owner), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "already_queued");
        assert_eq!(h.job_counts().await, vec![("queued".to_string(), 1)]);
        let mut conn = h.services().store().conn().await.unwrap();
        let audit = conn
            .audit()
            .for_entity(audit_entity::ORDER, order_id.0, 10)
            .await
            .unwrap();
        assert_eq!(
            audit
                .iter()
                .filter(|a| a.action == "order.redeliver")
                .count(),
            2
        );
        assert_eq!(audit[0].details["state"], "already_queued");
        drop(conn);
        h.db.cleanup().await;
    }

    #[tokio::test]
    async fn заказ_без_задачи_получает_новую_а_выданный_и_неоплаченный_отклоняются() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let client_id = h.client(702).await;
        let orphan = h.paid_order(client_id).await;
        let (status, body) = h
            .send(
                Method::POST,
                &format!("/api/admin/orders/{}/redeliver", orphan.0),
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "created");
        assert_eq!(h.job_counts().await, vec![("queued".to_string(), 1)]);
        let fulfilled = h.paid_order(client_id).await;
        {
            let mut conn = h.services().store().conn().await.unwrap();
            conn.orders()
                .transition(fulfilled, OrderStatus::Fulfilled)
                .await
                .unwrap();
        }
        let (status, body) = h
            .send(
                Method::POST,
                &format!("/api/admin/orders/{}/redeliver", fulfilled.0),
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "order.redeliver_status");
        assert_eq!(body["error"]["message"], "Заказ уже выдан");
        let (status, _) = h
            .send(
                Method::POST,
                "/api/admin/orders/999999/redeliver",
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(h.job_counts().await, vec![("queued".to_string(), 1)]);
        h.db.cleanup().await;
    }

    #[tokio::test]
    async fn заказ_на_ручной_проверке_выдаётся_снова_а_неоплаченный_нет() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let readonly = h.staff("readonly@example.com", AdminRole::ReadOnly).await;
        let client_id = h.client(703).await;
        let reviewed = h.paid_order(client_id).await;
        {
            let mut conn = h.services().store().conn().await.unwrap();
            conn.orders()
                .mark_review(reviewed, "сквада нет в панели")
                .await
                .unwrap();
        }
        let path = format!("/api/admin/orders/{}/redeliver", reviewed.0);
        let (status, _) = h.send(Method::POST, &path, Some(&readonly), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = h.send(Method::POST, &path, Some(&owner), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "created");
        let pending = {
            let amount = Money::new(299, Currency::Usd);
            let mut conn = h.services().store().conn().await.unwrap();
            conn.orders()
                .create(&NewOrder {
                    client_id,
                    subscription_id: None,
                    kind: OrderKind::TopUp,
                    intent: PurchaseIntent::TopUp { amount },
                    quote: Quote {
                        currency: Currency::Usd,
                        lines: Vec::new(),
                        subtotal: amount,
                        discount: Money::zero(Currency::Usd),
                        total: amount,
                        stars: None,
                        days: 0,
                        extra_devices: 0,
                    },
                    snapshot: None,
                })
                .await
                .unwrap()
                .id
        };
        let (status, body) = h
            .send(
                Method::POST,
                &format!("/api/admin/orders/{}/redeliver", pending.0),
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["message"], "Заказ не оплачен");
        assert_eq!(h.job_counts().await, vec![("queued".to_string(), 1)]);
        h.db.cleanup().await;
    }

    #[tokio::test]
    async fn событие_на_ручной_проверке_видно_админу_и_уходит_модулю_снова() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let support = h.staff("support@example.com", AdminRole::Support).await;
        let client_id = h.client(704).await;
        let event_id = {
            let mut conn = h.services().store().conn().await.unwrap();
            let id = conn
                .events()
                .push(&DomainEvent::ClientUnblocked { client_id })
                .await
                .unwrap();
            conn.events().mark_dispatched(&[id]).await.unwrap();
            conn.events()
                .save_delivery(&Delivery {
                    event_id: id,
                    module: ModuleId::Promo,
                    status: DeliveryStatus::Review,
                    attempts: 12,
                    next_attempt_at: None,
                    last_error: Some("модуль не ответил".into()),
                })
                .await
                .unwrap();
            id
        };
        let (status, _) = h
            .send(
                Method::GET,
                "/api/admin/events/review",
                Some(&support),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = h
            .send(Method::GET, "/api/admin/events/review", Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body[0]["event_id"], event_id.0);
        assert_eq!(body[0]["module"], "promo");
        assert_eq!(body[0]["status"], "review");
        let path = format!("/api/admin/events/{}/retry", event_id.0);
        let (status, _) = h
            .send(
                Method::POST,
                &path,
                Some(&support),
                Some(json!({ "module": "promo" })),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = h
            .send(
                Method::POST,
                &path,
                Some(&owner),
                Some(json!({ "module": "promo" })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["event_id"], event_id.0);
        let (status, body) = h
            .send(Method::GET, "/api/admin/events/review", Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!([]));
        let (status, body) = h
            .send(
                Method::POST,
                &path,
                Some(&owner),
                Some(json!({ "module": "promo" })),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "event.retry_status");
        let mut conn = h.services().store().conn().await.unwrap();
        let deliveries = conn.events().deliveries(&[event_id]).await.unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].status, DeliveryStatus::Pending);
        assert_eq!(deliveries[0].attempts, 0);
        let due = conn
            .events()
            .claim_due(chrono::Utc::now() + chrono::Duration::hours(1), 10)
            .await
            .unwrap();
        assert!(due.iter().any(|e| e.id == event_id));
        let audit = conn
            .audit()
            .for_entity("domain_event", event_id.0, 10)
            .await
            .unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].action, "module.event_retry");
        drop(conn);
        h.db.cleanup().await;
    }
}
