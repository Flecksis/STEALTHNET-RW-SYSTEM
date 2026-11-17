use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::ids::{EventId, JobId, OrderId};
use sn_domain::module::ModuleId;
use sn_domain::order::OrderStatus;
use sn_services::ServiceError;
use sn_services::fulfillment::{REVIEW_ORDERS_LIMIT, ReviewItem};
use sn_store::{Db, Delivery, PlanStage};

const REVIEW_LIMIT: i64 = 50;
const REVIEW_LIMIT_MAX: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orders/review", get(orders_review))
        .route("/orders/{id}/redeliver", post(redeliver))
        .route("/orders/{id}/recalculate", post(recalculate))
        .route("/orders/{id}/mark-fulfilled", post(mark_fulfilled))
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

#[derive(Debug, Serialize)]
struct Recalculation {
    ok: bool,
    order_id: OrderId,
    job_id: JobId,
    state: &'static str,
    plan: PlanStage,
}

#[derive(Debug, Default, Deserialize)]
struct RecalculateRequest {
    #[serde(default)]
    reapply: bool,
}

#[derive(Debug, Default, Deserialize)]
struct MarkFulfilledRequest {
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct MarkedFulfilled {
    ok: bool,
    order_id: OrderId,
    status: OrderStatus,
    fulfilled_at: Option<DateTime<Utc>>,
}

async fn orders_review(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ReviewQuery>,
) -> ApiResult<Json<Vec<ReviewItem>>> {
    admin.require(Permission::Money)?;
    let limit = query.limit.unwrap_or(REVIEW_ORDERS_LIMIT);
    Ok(Json(
        state.services.fulfillment().review_orders(limit).await?,
    ))
}

async fn redeliver(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Redelivery>> {
    admin.require(Permission::Money)?;
    let queued = state
        .services
        .fulfillment()
        .redeliver(OrderId(id), admin.actor())
        .await?;
    Ok(Json(Redelivery {
        ok: true,
        order_id: queued.order_id,
        job_id: queued.job_id,
        state: queued.state,
    }))
}

async fn recalculate(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<RecalculateRequest>>,
) -> ApiResult<Json<Recalculation>> {
    admin.require(Permission::Money)?;
    let reapply = body.is_some_and(|b| b.0.reapply);
    let queued = state
        .services
        .fulfillment()
        .recalculate(OrderId(id), reapply, admin.actor())
        .await?;
    Ok(Json(Recalculation {
        ok: true,
        order_id: queued.order_id,
        job_id: queued.job_id,
        state: queued.state,
        plan: queued.plan,
    }))
}

async fn mark_fulfilled(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<MarkFulfilledRequest>>,
) -> ApiResult<Json<MarkedFulfilled>> {
    admin.require(Permission::Money)?;
    let note = body.and_then(|b| b.0.note).unwrap_or_default();
    let order = state
        .services
        .fulfillment()
        .mark_fulfilled(OrderId(id), &note, admin.actor())
        .await?;
    Ok(Json(MarkedFulfilled {
        ok: true,
        order_id: order.id,
        status: order.status,
        fulfilled_at: order.fulfilled_at,
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
    use sn_domain::ids::{ClientId, JobId, OrderId, PeriodId, TariffId};
    use sn_domain::jobs::JobPayload;
    use sn_domain::module::ModuleId;
    use sn_domain::money::{Currency, Money};
    use sn_domain::order::{NewOrder, OrderKind, OrderStatus, PurchaseIntent};
    use sn_domain::plan::FulfillmentPlan;
    use sn_domain::pricing::Quote;
    use sn_domain::subscription::{AutoRenewState, SubscriptionKind};
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_services::{EnvFallbacks, ServiceDeps, Services, SystemClock};
    use sn_store::testing::{TestDb, test_store};
    use sn_store::{
        Db, Delivery, DeliveryStatus, NewAdmin, NewSubscription, PlanStage, SecretBox, audit_entity,
    };
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

        async fn stale_renewal(&self, client_id: ClientId) -> OrderId {
            let amount = Money::new(299, Currency::Usd);
            let plan: FulfillmentPlan = serde_json::from_value(json!({
                "kind": "renewal",
                "action": {"type": "update", "user_id": 77},
                "target": {
                    "expire_at": "2027-03-16T00:00:00Z",
                    "squads": ["00000000-0000-0000-0000-000000000001"],
                    "external_squad": null,
                    "traffic_limit_bytes": 0,
                    "traffic_strategy": "NO_RESET",
                    "hwid_device_limit": 3,
                    "tag": null
                },
                "reset_traffic": false,
                "activate": false,
                "period_started_at": "2026-11-16T14:52:00Z",
                "period_days": 120,
                "base_expire_at": "2027-02-14T00:00:00Z"
            }))
            .unwrap();
            let mut tx = self.services().store().begin().await.unwrap();
            let subscription = tx
                .subscriptions()
                .create(&NewSubscription {
                    client_id,
                    tariff_id: None,
                    kind: SubscriptionKind::Paid,
                    extra_devices: 0,
                    autorenew: AutoRenewState::default(),
                })
                .await
                .unwrap();
            let order = tx
                .orders()
                .create(&NewOrder {
                    client_id,
                    subscription_id: Some(subscription.id),
                    kind: OrderKind::Renewal,
                    intent: PurchaseIntent::Subscription {
                        tariff_id: TariffId(1),
                        period_id: PeriodId(1),
                        extra_devices: 0,
                        target: Some(subscription.id),
                    },
                    quote: Quote {
                        currency: Currency::Usd,
                        lines: Vec::new(),
                        subtotal: amount,
                        discount: Money::zero(Currency::Usd),
                        total: amount,
                        stars: None,
                        days: 30,
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
            tx.orders().save_plan(order.id, &plan).await.unwrap();
            tx.orders()
                .mark_review(
                    order.id,
                    "срок подписки в Remnawave изменился после расчёта плана",
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            order.id
        }

        async fn plan_stage(&self, order_id: OrderId) -> PlanStage {
            let mut conn = self.services().store().conn().await.unwrap();
            conn.orders().plan_state(order_id).await.unwrap().stage()
        }

        async fn order_status(&self, order_id: OrderId) -> OrderStatus {
            let mut conn = self.services().store().conn().await.unwrap();
            conn.orders().get(order_id).await.unwrap().status
        }

        async fn order_audit(&self, order_id: OrderId) -> Vec<sn_store::AuditRecord> {
            let mut conn = self.services().store().conn().await.unwrap();
            conn.audit()
                .for_entity(audit_entity::ORDER, order_id.0, 10)
                .await
                .unwrap()
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
    async fn список_заказов_на_ручной_проверке_показывает_причину_и_доступные_действия() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let support = h.staff("support@example.com", AdminRole::Support).await;
        let client_id = h.client(705).await;
        let topup = h.paid_order(client_id).await;
        {
            let mut conn = h.services().store().conn().await.unwrap();
            conn.orders()
                .mark_review(topup, "оплата получена, но модуль не подтвердил заказ")
                .await
                .unwrap();
        }
        let stale = h.stale_renewal(client_id).await;
        h.paid_order(client_id).await;
        let path = "/api/admin/orders/review";
        let (status, _) = h.send(Method::GET, path, Some(&support), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = h.send(Method::GET, path, Some(&owner), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let items = body.as_array().unwrap();
        assert_eq!(items.len(), 2, "{body}");
        assert_eq!(items[0]["order_id"], stale.0);
        assert_eq!(items[0]["kind"], "renewal");
        assert_eq!(items[0]["client_id"], client_id.0);
        assert_eq!(
            items[0]["reason"],
            "срок подписки в Remnawave изменился после расчёта плана"
        );
        assert_eq!(items[0]["plan"], "stale");
        assert_eq!(items[0]["plan_base_expire_at"], "2027-02-14T00:00:00Z");
        assert_eq!(items[0]["plan_target_expire_at"], "2027-03-16T00:00:00Z");
        assert_eq!(items[0]["refund_pending"], false);
        assert_eq!(items[0]["job"], Value::Null);
        assert_eq!(items[0]["total"], json!({"minor": 299, "currency": "USD"}));
        assert_eq!(
            items[0]["actions"],
            json!(["recalculate", "mark_fulfilled"])
        );
        assert_eq!(items[1]["order_id"], topup.0);
        assert_eq!(items[1]["kind"], "top_up");
        assert_eq!(
            items[1]["reason"],
            "оплата получена, но модуль не подтвердил заказ"
        );
        assert_eq!(items[1]["plan"], "none");
        assert_eq!(items[1]["actions"], json!(["redeliver", "mark_fulfilled"]));
        let (status, body) = h
            .send(
                Method::GET,
                "/api/admin/orders/review?limit=1",
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);
        h.db.cleanup().await;
    }

    #[tokio::test]
    async fn пересчитать_и_выдать_сбрасывает_план_ставит_задачу_и_требует_права_деньги() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let support = h.staff("support@example.com", AdminRole::Support).await;
        let readonly = h.staff("readonly@example.com", AdminRole::ReadOnly).await;
        let client_id = h.client(706).await;
        let order_id = h.stale_renewal(client_id).await;
        let redeliver = format!("/api/admin/orders/{}/redeliver", order_id.0);
        let (status, body) = h.send(Method::POST, &redeliver, Some(&owner), None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "order.plan_stale");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Пересчитать и выдать")
        );
        assert!(h.job_counts().await.is_empty());
        let path = format!("/api/admin/orders/{}/recalculate", order_id.0);
        for token in [&support, &readonly] {
            let (status, _) = h.send(Method::POST, &path, Some(token), None).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
        }
        assert_eq!(h.plan_stage(order_id).await, PlanStage::Stale);
        let (status, body) = h.send(Method::POST, &path, Some(&owner), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["order_id"], order_id.0);
        assert_eq!(body["state"], "created");
        assert_eq!(body["plan"], "stale");
        assert_eq!(h.plan_stage(order_id).await, PlanStage::None);
        assert_eq!(h.order_status(order_id).await, OrderStatus::Review);
        assert_eq!(h.job_counts().await, vec![("queued".to_string(), 1)]);
        let (status, body) = h.send(Method::POST, &path, Some(&owner), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "already_queued");
        assert_eq!(body["plan"], "none");
        assert_eq!(h.job_counts().await, vec![("queued".to_string(), 1)]);
        let audit = h.order_audit(order_id).await;
        let records: Vec<_> = audit
            .iter()
            .filter(|a| a.action == "order.recalculate")
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].actor_kind, "admin");
        assert_eq!(records[1].details["plan"], "stale");
        assert_eq!(
            records[1].details["previous_plan"]["target_expire_at"],
            "2027-03-16T00:00:00Z"
        );
        let (status, body) = h.send(Method::POST, &redeliver, Some(&owner), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "already_queued");
        let applied = h.stale_renewal(client_id).await;
        {
            let mut conn = h.services().store().conn().await.unwrap();
            let reset = conn.orders().reset_plan(applied).await.unwrap();
            conn.orders()
                .save_plan(applied, &reset.plan.unwrap())
                .await
                .unwrap();
            conn.orders()
                .mark_plan_applied(applied, chrono::Utc::now())
                .await
                .unwrap();
        }
        let again = format!("/api/admin/orders/{}/recalculate", applied.0);
        for body in [None, Some(json!({})), Some(json!({ "reapply": false }))] {
            let (status, body) = h.send(Method::POST, &again, Some(&owner), body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            assert_eq!(body["error"]["code"], "order.plan_applied");
        }
        assert_eq!(h.plan_stage(applied).await, PlanStage::Applied);
        let (status, body) = h
            .send(
                Method::POST,
                &again,
                Some(&owner),
                Some(json!({ "reapply": true })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["plan"], "applied");
        assert_eq!(h.plan_stage(applied).await, PlanStage::None);
        let topup = h.paid_order(client_id).await;
        {
            let mut conn = h.services().store().conn().await.unwrap();
            conn.orders()
                .mark_review(topup, "модуль не подтвердил заказ")
                .await
                .unwrap();
        }
        let (status, body) = h
            .send(
                Method::POST,
                &format!("/api/admin/orders/{}/recalculate", topup.0),
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "order.recalculate_kind");
        let paid = h.paid_order(client_id).await;
        let (status, body) = h
            .send(
                Method::POST,
                &format!("/api/admin/orders/{}/recalculate", paid.0),
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "order.review_status");
        let (status, _) = h
            .send(
                Method::POST,
                "/api/admin/orders/999999/recalculate",
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        h.db.cleanup().await;
    }

    #[tokio::test]
    async fn отметить_выданным_требует_пояснение_и_завершает_заказ_на_проверке() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let support = h.staff("support@example.com", AdminRole::Support).await;
        let client_id = h.client(707).await;
        let order_id = h.stale_renewal(client_id).await;
        let path = format!("/api/admin/orders/{}/mark-fulfilled", order_id.0);
        let note = json!({ "note": "Срок в панели уже учитывает эту оплату" });
        let (status, _) = h
            .send(Method::POST, &path, Some(&support), Some(note.clone()))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        for body in [None, Some(json!({})), Some(json!({ "note": "   " }))] {
            let (status, body) = h.send(Method::POST, &path, Some(&owner), body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            assert_eq!(body["error"]["code"], "order.mark_fulfilled_note");
        }
        assert_eq!(h.order_status(order_id).await, OrderStatus::Review);
        let (status, body) = h
            .send(Method::POST, &path, Some(&owner), Some(note.clone()))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["order_id"], order_id.0);
        assert_eq!(body["status"], "fulfilled");
        assert!(body["fulfilled_at"].is_string());
        assert_eq!(h.order_status(order_id).await, OrderStatus::Fulfilled);
        assert!(h.job_counts().await.is_empty());
        let audit = h.order_audit(order_id).await;
        assert_eq!(audit[0].action, "order.mark_fulfilled");
        assert_eq!(audit[0].actor_kind, "admin");
        assert_eq!(
            audit[0].details["note"],
            "Срок в панели уже учитывает эту оплату"
        );
        assert_eq!(audit[0].details["plan"], "stale");
        let (status, body) = h
            .send(Method::POST, &path, Some(&owner), Some(note.clone()))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "order.review_status");
        assert_eq!(body["error"]["message"], "Заказ уже выдан");
        let paid = h.paid_order(client_id).await;
        let (status, body) = h
            .send(
                Method::POST,
                &format!("/api/admin/orders/{}/mark-fulfilled", paid.0),
                Some(&owner),
                Some(note),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "order.review_status");
        let (status, body) = h
            .send(Method::GET, "/api/admin/orders/review", Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!([]));
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
