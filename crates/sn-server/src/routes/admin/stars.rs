use super::Paging;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_services::stars_review::{StarsResolution, StarsRetry, StarsReviewPage};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/stars/review", get(list))
        .route("/stars/review/{id}/refund", post(refund))
        .route("/stars/review/{id}/retry", post(retry))
        .route("/stars/review/{id}/close", post(close))
}

#[derive(Debug, Default, Deserialize)]
struct ResolveRequest {
    #[serde(default)]
    comment: Option<String>,
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(paging): ApiQuery<Paging>,
) -> ApiResult<Json<StarsReviewPage>> {
    admin.require(Permission::Money)?;
    Ok(Json(
        state.services.stars_review().list(paging.request()).await?,
    ))
}

async fn refund(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<ResolveRequest>>,
) -> ApiResult<Json<StarsResolution>> {
    admin.require(Permission::Money)?;
    admin.require(Permission::Refunds)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(
        state
            .services
            .stars_review()
            .refund(id, body.comment.as_deref(), admin.0.admin_id)
            .await?,
    ))
}

async fn retry(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<StarsRetry>> {
    admin.require(Permission::Money)?;
    Ok(Json(
        state
            .services
            .stars_review()
            .retry(id, admin.0.admin_id)
            .await?,
    ))
}

async fn close(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<ResolveRequest>>,
) -> ApiResult<Json<StarsResolution>> {
    admin.require(Permission::Money)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(
        state
            .services
            .stars_review()
            .close(
                id,
                body.comment.as_deref().unwrap_or_default(),
                admin.0.admin_id,
            )
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use crate::{AppState, ServerConfig, router};
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode, header};
    use serde_json::{Value, json};
    use sn_domain::access::{AdminRole, Permission};
    use sn_domain::client::{RegistrationSource, TelegramProfile};
    use sn_domain::ids::ClientId;
    use sn_domain::money::{Amount, Currency, Money, PayCurrency};
    use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
    use sn_domain::payment::{NewPayment, Payment, PaymentStatus, ProviderId, providers};
    use sn_domain::pricing::Quote;
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_services::stars_review::{CLOSED_ACTION, REFUNDED_ACTION, RETRIED_ACTION};
    use sn_services::{EnvFallbacks, ServiceDeps, Services, SystemClock};
    use sn_store::testing::{TestDb, test_store};
    use sn_store::{Db, NewAdmin, SecretBox, audit_entity};
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

        async fn staff_with(&self, email: &str, extra: &[Permission]) -> String {
            let hash = sn_services::auth::hash_password(PASSWORD).unwrap();
            {
                let mut conn = self.services().store().conn().await.unwrap();
                let created = conn
                    .admins()
                    .create(&NewAdmin {
                        email: email.into(),
                        password_hash: hash,
                        role: AdminRole::ReadOnly,
                        must_change_password: false,
                    })
                    .await
                    .unwrap();
                conn.admins()
                    .set_role(created.id, AdminRole::ReadOnly, extra)
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

        async fn stars_invoice(&self, client_id: ClientId) -> Payment {
            let total = Money::new(699, Currency::Usd);
            let mut tx = self.services().store().begin().await.unwrap();
            let order = tx
                .orders()
                .create(&NewOrder {
                    client_id,
                    subscription_id: None,
                    kind: OrderKind::Module,
                    intent: PurchaseIntent::Module {
                        module: sn_domain::module::ModuleId::Options,
                        kind: "тест".into(),
                        payload: json!({}),
                    },
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
            let payment = tx
                .payments()
                .create(&NewPayment {
                    order_id: order.id,
                    client_id,
                    provider: ProviderId::from(providers::STARS),
                    method: None,
                    amount: Amount::new(350, PayCurrency::Xtr),
                    expires_at: None,
                })
                .await
                .unwrap();
            tx.commit().await.unwrap();
            payment
        }

        async fn seed_review(&self, charge: &str, event: &str, payer: i64, payment: &Payment) {
            self.seed_review_of(charge, event, payer, payment, payment.amount.minor - 1)
                .await;
        }

        async fn seed_review_of(
            &self,
            charge: &str,
            event: &str,
            payer: i64,
            payment: &Payment,
            stars: i64,
        ) {
            let payload = json!({
                "event": event,
                "invoice_payload": payment.public_id.to_string(),
                "total_amount": stars,
                "currency": "XTR",
                "telegram_payment_charge_id": charge,
                "telegram_user_id": payer,
            });
            let row = format!(
                "{charge}\t{event}\t{payload}\tupdate\treview\t1\t2026-11-17 09:00:00+00\tсумма не сошлась\t{}\n",
                payment.id.0
            );
            let mut conn = self.services().store().conn().await.unwrap();
            let mut copy = conn
                .conn()
                .copy_in_raw(
                    "COPY bot_star_events (charge_id, event, payload, source, status, attempts, \
                     next_attempt_at, last_error, payment_id) FROM STDIN",
                )
                .await
                .unwrap();
            copy.send(row.as_bytes()).await.unwrap();
            assert_eq!(copy.finish().await.unwrap(), 1);
        }

        async fn audit(&self, payment: &Payment, action: &str) -> Vec<sn_store::AuditRecord> {
            let mut conn = self.services().store().conn().await.unwrap();
            conn.audit()
                .for_entity(audit_entity::PAYMENT, payment.id.0, 20)
                .await
                .unwrap()
                .into_iter()
                .filter(|record| record.action == action)
                .collect()
        }

        async fn payment_status(&self, payment: &Payment) -> PaymentStatus {
            let mut conn = self.services().store().conn().await.unwrap();
            conn.payments().get(payment.id).await.unwrap().status
        }
    }

    #[tokio::test]
    async fn разбор_оплат_звёздами_требует_права_деньги() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let support = h.staff("support@example.com", AdminRole::Support).await;
        let readonly = h.staff("readonly@example.com", AdminRole::ReadOnly).await;
        let client = h.client(901).await;
        let payment = h.stars_invoice(client).await;
        h.seed_review("stx-901", "successful_payment", 901, &payment)
            .await;
        let (status, page) = h
            .send(Method::GET, "/api/admin/stars/review", Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let id = page["items"][0]["id"].as_i64().unwrap();
        let refund = format!("/api/admin/stars/review/{id}/refund");
        let close = format!("/api/admin/stars/review/{id}/close");
        for token in [&support, &readonly] {
            let (status, _) = h
                .send(Method::GET, "/api/admin/stars/review", Some(token), None)
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            let (status, _) = h.send(Method::POST, &refund, Some(token), None).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            let (status, _) = h
                .send(
                    Method::POST,
                    &close,
                    Some(token),
                    Some(json!({ "comment": "закрыть" })),
                )
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
        }
        for (method, path) in [
            (Method::GET, "/api/admin/stars/review"),
            (Method::POST, refund.as_str()),
            (Method::POST, close.as_str()),
        ] {
            let (status, _) = h.send(method, path, None, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        }
        let (status, page) = h
            .send(Method::GET, "/api/admin/stars/review", Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert!(h.audit(&payment, CLOSED_ACTION).await.is_empty());
        assert!(h.audit(&payment, REFUNDED_ACTION).await.is_empty());
        h.db.cleanup().await;
    }

    #[tokio::test]
    async fn событие_звёзд_на_проверке_видно_в_списке_и_закрывается_с_пояснением() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let client = h.client(902).await;
        let payment = h.stars_invoice(client).await;
        h.seed_review("stx-902", "successful_payment", 902, &payment)
            .await;
        let (status, page) = h
            .send(Method::GET, "/api/admin/stars/review", Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["next"], Value::Null);
        assert_eq!(page["total"], 1);
        let item = &page["items"][0];
        let id = item["id"].as_i64().unwrap();
        assert_eq!(item["charge_id"], "stx-902");
        assert_eq!(item["event"], "successful_payment");
        assert_eq!(item["stars"], 349);
        assert_eq!(item["telegram_user_id"], 902);
        assert_eq!(item["payer_client_id"], client.0);
        assert_eq!(item["payment"]["id"], payment.id.0);
        assert_eq!(item["payment"]["status"], "pending");
        assert_eq!(
            item["payment"]["amount"],
            json!({ "minor": 350, "currency": "XTR" })
        );
        assert_eq!(item["order"]["id"], payment.order_id.0);
        assert_eq!(item["order"]["status"], "pending");
        assert_eq!(item["reason"], "сумма не сошлась");
        assert_eq!(item["refundable"], true);
        assert!(item["created_at"].is_string() && item["updated_at"].is_string());
        let close = format!("/api/admin/stars/review/{id}/close");
        for body in [None, Some(json!({})), Some(json!({ "comment": "  " }))] {
            let (status, body) = h.send(Method::POST, &close, Some(&owner), body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            assert_eq!(body["error"]["code"], "stars.review_comment");
        }
        let mark = format!("/api/admin/payments/{}/mark-paid", payment.id.0);
        let (status, body) = h
            .send(Method::POST, &mark, Some(&owner), Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "payment.manual_stars");
        let (status, body) = h
            .send(
                Method::POST,
                &close,
                Some(&owner),
                Some(json!({ "comment": "Продлили подписку вручную" })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["id"], id);
        assert_eq!(body["status"], "closed");
        assert_eq!(body["repeated"], false);
        assert_eq!(body["comment"], "Продлили подписку вручную");
        assert_eq!(body["payment_id"], payment.id.0);
        let (status, page) = h
            .send(Method::GET, "/api/admin/stars/review", Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"], json!([]));
        assert_eq!(page["total"], 0);
        let (status, body) = h
            .send(
                Method::POST,
                &close,
                Some(&owner),
                Some(json!({ "comment": "ещё раз" })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["repeated"], true);
        assert_eq!(body["comment"], "Продлили подписку вручную");
        let (status, body) = h
            .send(
                Method::POST,
                &format!("/api/admin/stars/review/{id}/refund"),
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "stars.review_closed");
        let audit = h.audit(&payment, CLOSED_ACTION).await;
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].actor_kind, "admin");
        assert!(audit[0].actor_id.is_some());
        assert_eq!(audit[0].details["comment"], "Продлили подписку вручную");
        assert_eq!(h.payment_status(&payment).await, PaymentStatus::Pending);
        h.db.cleanup().await;
    }

    #[tokio::test]
    async fn возврат_звёзд_через_http_отклоняется_без_токена_бота_и_для_уведомления_о_возврате() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let client = h.client(903).await;
        let payment = h.stars_invoice(client).await;
        h.seed_review("stx-903", "successful_payment", 903, &payment)
            .await;
        h.seed_review("stx-904", "refunded_payment", 903, &payment)
            .await;
        let (status, page) = h
            .send(
                Method::GET,
                "/api/admin/stars/review?limit=1",
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        let notice = page["items"][0]["id"].as_i64().unwrap();
        assert_eq!(page["items"][0]["event"], "refunded_payment");
        assert_eq!(page["items"][0]["refundable"], false);
        assert_eq!(page["next"], notice);
        assert_eq!(page["total"], 2);
        let (status, page) = h
            .send(
                Method::GET,
                &format!("/api/admin/stars/review?limit=1&after={notice}"),
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let paid = page["items"][0]["id"].as_i64().unwrap();
        assert_eq!(page["items"][0]["charge_id"], "stx-903");
        assert_eq!(page["next"], Value::Null);
        let (status, body) = h
            .send(
                Method::POST,
                &format!("/api/admin/stars/review/{notice}/refund"),
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "stars.review_not_payment");
        let (status, body) = h
            .send(
                Method::POST,
                &format!("/api/admin/stars/review/{paid}/refund"),
                Some(&owner),
                Some(json!({ "comment": "вернуть" })),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "stars.bot_token_missing");
        for path in [
            "/api/admin/stars/review/999999/refund",
            "/api/admin/stars/review/999999/close",
        ] {
            let (status, body) = h
                .send(
                    Method::POST,
                    path,
                    Some(&owner),
                    Some(json!({ "comment": "нет такого" })),
                )
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        }
        let (status, _) = h
            .send(
                Method::POST,
                "/api/admin/stars/review/abc/close",
                Some(&owner),
                Some(json!({ "comment": "нет такого" })),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, page) = h
            .send(Method::GET, "/api/admin/stars/review", Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"].as_array().unwrap().len(), 2);
        assert!(h.audit(&payment, REFUNDED_ACTION).await.is_empty());
        h.db.cleanup().await;
    }

    #[tokio::test]
    async fn вернуть_звёзды_можно_только_с_правами_деньги_и_возвраты() {
        let Some(h) = harness().await else { return };
        let cashier = h
            .staff_with("cashier@example.com", &[Permission::Money])
            .await;
        let refunder = h
            .staff_with("refunds@example.com", &[Permission::Refunds])
            .await;
        let both = h
            .staff_with(
                "both@example.com",
                &[Permission::Money, Permission::Refunds],
            )
            .await;
        let client = h.client(905).await;
        let payment = h.stars_invoice(client).await;
        h.seed_review("stx-905", "successful_payment", 905, &payment)
            .await;
        let (status, page) = h
            .send(Method::GET, "/api/admin/stars/review", Some(&cashier), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let id = page["items"][0]["id"].as_i64().unwrap();
        let refund = format!("/api/admin/stars/review/{id}/refund");
        for token in [&cashier, &refunder] {
            let (status, body) = h
                .send(
                    Method::POST,
                    &refund,
                    Some(token),
                    Some(json!({ "comment": "вернуть" })),
                )
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert_eq!(body["error"]["code"], "auth.forbidden");
        }
        let (status, _) = h
            .send(
                Method::GET,
                "/api/admin/stars/review",
                Some(&refunder),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = h
            .send(
                Method::POST,
                &refund,
                Some(&both),
                Some(json!({ "comment": "вернуть" })),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "stars.bot_token_missing");
        assert!(h.audit(&payment, REFUNDED_ACTION).await.is_empty());
        h.db.cleanup().await;
    }

    #[tokio::test]
    async fn провести_снова_доступно_только_после_сбоя() {
        let Some(h) = harness().await else { return };
        let owner = h.owner().await;
        let support = h.staff("support@example.com", AdminRole::Support).await;
        let client = h.client(906).await;
        let mismatch = h.stars_invoice(client).await;
        h.seed_review("stx-906", "successful_payment", 906, &mismatch)
            .await;
        let failed = h.stars_invoice(client).await;
        h.seed_review_of(
            "stx-907",
            "successful_payment",
            906,
            &failed,
            failed.amount.minor,
        )
        .await;
        let (status, page) = h
            .send(Method::GET, "/api/admin/stars/review", Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let item = |charge: &str| {
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["charge_id"] == charge)
                .cloned()
                .unwrap()
        };
        let (mismatch_item, failed_item) = (item("stx-906"), item("stx-907"));
        assert_eq!(mismatch_item["retryable"], false);
        assert_eq!(failed_item["retryable"], true);
        let retry_mismatch = format!(
            "/api/admin/stars/review/{}/retry",
            mismatch_item["id"].as_i64().unwrap()
        );
        let retry_failed = format!(
            "/api/admin/stars/review/{}/retry",
            failed_item["id"].as_i64().unwrap()
        );
        let (status, _) = h.send(Method::POST, &retry_failed, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = h
            .send(Method::POST, &retry_failed, Some(&support), None)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = h
            .send(Method::POST, &retry_mismatch, Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "stars.review_not_retryable");
        assert!(h.audit(&mismatch, RETRIED_ACTION).await.is_empty());
        let (status, body) = h
            .send(Method::POST, &retry_failed, Some(&owner), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["charge_id"], "stx-907");
        assert_eq!(body["payment_id"], failed.id.0);
        assert!(
            ["done", "review", "pending"].contains(&body["outcome"].as_str().unwrap()),
            "{body}"
        );
        assert_eq!(h.audit(&failed, RETRIED_ACTION).await.len(), 1);
        let (status, body) = h
            .send(
                Method::POST,
                "/api/admin/stars/review/999999/retry",
                Some(&owner),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        h.db.cleanup().await;
    }
}
