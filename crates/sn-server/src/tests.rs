use crate::{AppState, ServerConfig, router};
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{Value, json};
use sn_domain::access::AdminRole;
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::{ClientId, SquadId};
use sn_domain::money::{Currency, Stars};
use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
use sn_domain::payment::{NewPayment, ProviderId, providers};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::settings::keys;
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, Tariff};
use sn_domain::traffic::TrafficPolicy;
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_remnawave::fake::FakeRemnawave;
use sn_remnawave::{InternalSquad, Node, SquadInfo};
use sn_services::{EnvFallbacks, FixedClock, ServiceDeps, Services, SystemClock};
use sn_store::testing::{TestDb, test_store};
use sn_store::{Db, NewAdmin, NewCategory, NewPeriod, NewTariff, SecretBox, Store, StoreOptions};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "0123456789abcdef0123456789abcdef";
const PASSWORD: &str = "очень-надёжный-пароль";

fn services(store: Store) -> Services {
    Services::new(ServiceDeps {
        store,
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: None,
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    })
}

fn offline_state() -> AppState {
    let store = Store::connect_lazy(
        "postgres://sn:sn@127.0.0.1:1/sn_test",
        StoreOptions {
            acquire_timeout: std::time::Duration::from_millis(300),
            ..StoreOptions::default()
        },
    )
    .unwrap();
    AppState::new(ServerConfig::default(), services(store))
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

struct Http {
    db: TestDb,
    state: AppState,
    clock: Arc<FixedClock>,
    remna: Arc<FakeRemnawave>,
}

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap()
}

async fn http() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

impl Http {
    fn services(&self) -> &Services {
        &self.state.services
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
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
        let headers = response.headers().clone();
        (status, headers, body_json(response).await)
    }

    async fn get(&self, path: &str, token: &str) -> (StatusCode, Value) {
        let (status, _, body) = self.send(Method::GET, path, Some(token), None).await;
        (status, body)
    }

    async fn write(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let (status, _, body) = self.send(method, path, Some(token), Some(body)).await;
        (status, body)
    }

    async fn login(&self, email: &str) -> String {
        let (status, _, body) = self
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

    async fn admin(&self, email: &str, role: AdminRole) -> String {
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
                    username: Some("morozov_a".into()),
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

    async fn client_token(&self, client: ClientId) -> String {
        self.services()
            .auth()
            .client_session_for(
                client,
                sn_store::SessionChannel::Cabinet,
                &Default::default(),
                false,
            )
            .await
            .unwrap()
            .token
    }

    async fn set_bot(&self) {
        let mut conn = self.services().store().conn().await.unwrap();
        conn.settings()
            .set_as(keys::BOT_USERNAME, &"stealth_vpn_bot", None)
            .await
            .unwrap();
    }

    async fn tariff(&self) -> Tariff {
        let mut tx = self.services().store().begin().await.unwrap();
        let category = tx
            .tariffs()
            .create_category(&NewCategory {
                name: "Основные".into(),
                sort: 0,
                visible: true,
            })
            .await
            .unwrap();
        let tariff = tx
            .tariffs()
            .create(&NewTariff {
                category_id: category.id,
                name: "Стандарт".into(),
                description: None,
                on_sale: true,
                sort: 0,
                devices: DevicePolicy {
                    included: 3,
                    extra: None,
                },
                traffic: TrafficPolicy::UNLIMITED,
                squads: vec![SquadId(Uuid::from_u128(1))],
                external_squad: None,
                periods: vec![NewPeriod {
                    days: 90,
                    prices: CurrencyPrices {
                        usd: Some(699),
                        rub: Some(49_900),
                    },
                    stars: Some(Stars(350)),
                    sort: 0,
                    active: true,
                }],
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        tariff
    }

    async fn pending_payment(
        &self,
        client_id: ClientId,
        tariff: &Tariff,
    ) -> sn_domain::payment::Payment {
        let period = &tariff.periods[0];
        let q = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff,
            period_id: period.id,
            extra_devices: 0,
            sell_extra_devices: true,
            adjustments: &[],
        })
        .unwrap();
        let mut tx = self.services().store().begin().await.unwrap();
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id,
                subscription_id: None,
                kind: OrderKind::New,
                intent: PurchaseIntent::Subscription {
                    tariff_id: tariff.id,
                    period_id: period.id,
                    extra_devices: 0,
                    target: None,
                },
                quote: q.clone(),
                snapshot: Some(tariff.snapshot(period)),
            })
            .await
            .unwrap();
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id,
                provider: ProviderId::from(providers::CRYPTOPAY),
                method: None,
                amount: q.total.into(),
                expires_at: None,
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        payment
    }
}

fn totp_code(secret: &str, at: DateTime<Utc>) -> String {
    let key = sn_services::totp::base32_decode(secret).unwrap();
    sn_services::totp::code_at(
        &key,
        (at.timestamp() / sn_services::totp::STEP_SECONDS) as u64,
        sn_services::totp::DIGITS,
    )
    .unwrap()
}

#[tokio::test]
async fn версия_отвечает_без_базы() {
    let response = router(offline_state())
        .oneshot(Request::get("/api/version").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["remnawave"], "3.4.11");
}

#[tokio::test]
async fn каждый_ответ_несёт_идентификатор_запроса() {
    let response = router(offline_state())
        .oneshot(Request::get("/api/version").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let generated = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert_eq!(generated.len(), 36);
    let response = router(offline_state())
        .oneshot(
            Request::get("/api/version")
                .header("x-request-id", "req-42")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["x-request-id"], "req-42");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn здоровье_честно_говорит_что_базы_нет() {
    let response = router(offline_state())
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn вебхук_remnawave_без_секрета_и_с_чужой_подписью() {
    let state = offline_state();
    let body = r#"{"scope":"service","event":"service.panel_started","timestamp":"2026-01-07T11:57:29.426Z","data":{}}"#;
    let request = |signature: Option<&str>| {
        let mut builder = Request::post("/api/remnawave/webhook")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(signature) = signature {
            builder = builder.header("x-remnawave-signature", signature);
        }
        builder.body(Body::from(body)).unwrap()
    };
    let response = router(state.clone()).oneshot(request(None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    state.services.set_remna_webhook_secret(Some(SECRET.into()));
    let response = router(state.clone())
        .oneshot(request(Some("00ff")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "remnawave.bad_signature"
    );
}

#[tokio::test]
async fn вебхук_telegram_выключен_бот_работает_через_long_polling() {
    let response = router(offline_state())
        .oneshot(
            Request::post("/api/telegram/webhook")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "telegram.webhook_disabled"
    );
}

#[tokio::test]
async fn админка_без_входа_закрыта() {
    let state = offline_state();
    for path in [
        "/api/admin/me",
        "/api/admin/modules",
        "/api/admin/dashboard",
        "/api/admin/payments",
        "/api/admin/settings/core",
        "/api/admin/m/gifts/anything",
    ] {
        let response = router(state.clone())
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let response = router(state)
        .oneshot(Request::get("/api/nothing").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"]["code"], "not_found");
}

#[tokio::test]
async fn вход_администратора_и_переключение_модуля() {
    let Some(db) = sn_store::testing::test_store().await else {
        return;
    };
    let services = services(db.store.clone());
    services.reload().await.unwrap();
    services
        .auth()
        .upsert_owner("owner@example.com", PASSWORD)
        .await
        .unwrap();
    let state = AppState::new(ServerConfig::default(), services);
    let login = router(state.clone())
        .oneshot(
            Request::post("/api/admin/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"owner@example.com","password":"очень-надёжный-пароль"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    assert!(login.headers().get(header::SET_COOKIE).is_some());
    let token = body_json(login).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let toggle = router(state.clone())
        .oneshot(
            Request::patch("/api/admin/modules/gifts")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(toggle.status(), StatusCode::OK);
    assert_eq!(body_json(toggle).await["active"], true);
    let nalog = router(state.clone())
        .oneshot(
            Request::patch("/api/admin/modules/nalog")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(nalog.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let menu = router(state)
        .oneshot(
            Request::get("/api/admin/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let labels: Vec<String> = body_json(menu).await["menu"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["label"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(labels, ["Рассылки", "Промокоды", "Рефералы", "Подарки"]);
    db.cleanup().await;
}

#[tokio::test]
async fn вход_по_коду_totp_и_смена_пароля_через_http() {
    let Some(h) = http().await else { return };
    let token = h.owner().await;
    let (status, setup) = h
        .write(Method::POST, "/api/admin/auth/2fa/setup", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap().to_string();
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/auth/2fa/enable",
            &token,
            json!({ "secret": secret, "code": totp_code(&secret, start()), "password": PASSWORD }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/admin/auth/login",
            None,
            Some(json!({ "email": "owner@example.com", "password": PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "auth.totp_required");
    h.clock.set(start() + Duration::seconds(30));
    let (status, headers, body) = h
        .send(
            Method::POST,
            "/api/admin/auth/login",
            None,
            Some(json!({
                "email": "owner@example.com",
                "password": PASSWORD,
                "totp": totp_code(&secret, start() + Duration::seconds(30)),
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cookie = headers[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(cookie.starts_with("sn_admin="));
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    let second = body["token"].as_str().unwrap().to_string();
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/auth/password",
            &second,
            json!({ "current": PASSWORD, "new": "новый-надёжный-пароль" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let renewed = body["token"].as_str().unwrap().to_string();
    for old in [&token, &second] {
        assert_eq!(
            h.get("/api/admin/me", old).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, me) = h.get("/api/admin/me", &renewed).await;
    assert_eq!(status, StatusCode::OK);
    assert!(me["permissions"].as_array().unwrap().len() >= 11);
    let (status, security) = h.get("/api/admin/auth/security", &renewed).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(security["totp_enabled"], true);
    let (status, _, _) = h
        .send(Method::POST, "/api/admin/auth/logout", Some(&renewed), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.get("/api/admin/me", &renewed).await.0,
        StatusCode::UNAUTHORIZED
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn поддержка_не_видит_деньги_а_наблюдатель_не_пишет() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let readonly = h.admin("readonly@example.com", AdminRole::ReadOnly).await;
    let client = h.client(5_093_118_422).await;
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/admin/clients/{}/balance", client.0),
            &owner,
            json!({ "delta": { "minor": 420, "currency": "USD" }, "comment": "подарок", "idempotency_key": "r1" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let card = format!("/api/admin/clients/{}", client.0);
    let (status, body) = h.get(&card, &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["balance"]["minor"], 420);
    let (status, body) = h.get(&card, &support).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("balance").is_none() && body.get("referral_balance").is_none());
    let (status, history) = h.get(&format!("{card}/history"), &support).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        history
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"] != "balance_changed")
    );
    let (status, history) = h.get(&format!("{card}/history"), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        history
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "balance_changed")
    );
    for path in [
        "/api/admin/payments",
        "/api/admin/settings/core",
        "/api/admin/tariffs",
    ] {
        assert_eq!(
            h.get(path, &support).await.0,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    let (status, _) = h
        .write(
            Method::POST,
            &format!("{card}/balance"),
            &support,
            json!({ "delta": { "minor": 100, "currency": "USD" }, "comment": "x", "idempotency_key": "r2" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, dashboard) = h.get("/api/admin/dashboard", &support).await;
    assert_eq!(status, StatusCode::OK);
    assert!(dashboard["revenue"].is_null());
    assert!(
        !h.get("/api/admin/support/tickets", &support)
            .await
            .0
            .is_client_error()
    );
    let (status, _) = h
        .write(
            Method::PUT,
            &format!("{card}/note"),
            &support,
            json!({ "note": "звонил" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(h.get(&card, &readonly).await.0, StatusCode::OK);
    let (status, _) = h
        .write(
            Method::PUT,
            &format!("{card}/note"),
            &readonly,
            json!({ "note": "x" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .write(Method::POST, &format!("{card}/block"), &readonly, json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for path in [
        "/api/admin/infra/nodes",
        "/api/admin/support/tickets",
        "/api/admin/modules",
        "/api/admin/settings/remnawave",
    ] {
        assert_eq!(
            h.get(path, &readonly).await.0,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    let (status, me) = h.get("/api/admin/me", &readonly).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["permissions"], json!(["clients_read"]));
    h.db.cleanup().await;
}

#[tokio::test]
async fn настройки_ядра_оплаты_и_remnawave_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "currency": "RUB" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["settings"]["currency"], "RUB");
    let (_, body) = h.get("/api/admin/settings/payments", &owner).await;
    assert_eq!(body["currency"], "RUB");
    assert_eq!(body["order"][0], "platega");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/payments",
            &owner,
            json!({ "order": ["yoomoney", "platega", "balance"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["order"], json!(["yoomoney", "platega"]));
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/payments",
            &owner,
            json!({ "order": ["cryptopay"] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "settings.invalid_field");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/trial",
            &owner,
            json!({ "enabled": true, "days": 3, "squads": [], "devices": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "settings.invalid_field");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/trial",
            &owner,
            json!({ "enabled": true, "days": 3, "squads": [Uuid::from_u128(1)], "devices": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(h.services().settings().trial.days, 3);
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/yoomoney",
            &owner,
            json!({ "config": { "unknown_field": "x" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/cryptopay",
            &owner,
            json!({ "enabled": true, "config": { "api_token": "4321:SECRET" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"]["settings"]["api_token"]["filled"], true);
    assert!(!body.to_string().contains("4321:SECRET"));
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/remnawave/webhook-secret",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let secret = body["secret"].as_str().unwrap().to_string();
    assert!(body["env"].as_str().unwrap().contains(&secret));
    let (status, body) = h.get("/api/admin/settings/remnawave", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["webhook_secret_set"], true);
    assert!(!body.to_string().contains(&secret));
    h.services().set_remna(Some(h.remna.clone()));
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/remnawave/check",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["version"], "3.4.11");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/bot",
            &owner,
            json!({ "username": "@stealth_vpn_bot" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["username"], "stealth_vpn_bot");
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/unknown",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "currency": "EUR" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.invalid_body");
    let (status, body) = h.get("/api/admin/modules/gifts/config", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["schema"], json!([]));
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/modules/gifts/config",
            &owner,
            json!({ "config": { "anything": 1 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn тарифы_и_категории_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    h.remna.set_squads(vec![InternalSquad {
        uuid: Uuid::from_u128(1),
        name: "Основной".into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    let (status, category) = h
        .write(
            Method::POST,
            "/api/admin/categories",
            &owner,
            json!({ "name": "Основные" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{category}");
    let tariff = json!({
        "category_id": category["id"],
        "name": "Стандарт",
        "devices": { "included": 3, "extra": { "max": 2, "monthly": { "usd": 99, "rub": 4900 }, "monthly_stars": 50 } },
        "traffic": { "limit_bytes": null, "reset": "no_reset" },
        "squads": [Uuid::from_u128(1)],
        "periods": [
            { "days": 30, "prices": { "usd": 299, "rub": 19900 }, "stars": 150 },
            { "days": 90, "prices": { "usd": 699, "rub": 49900 }, "stars": 350 }
        ]
    });
    let (status, created) = h
        .write(Method::POST, "/api/admin/tariffs", &owner, tariff.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["periods"].as_array().unwrap().len(), 2);
    let id = created["id"].as_i64().unwrap();
    let (status, hidden) = h
        .write(
            Method::PATCH,
            &format!("/api/admin/tariffs/{id}"),
            &owner,
            json!({ "on_sale": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hidden["on_sale"], false);
    let (status, list) = h.get("/api/admin/tariffs", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["tariffs"].as_array().unwrap().len(), 1);
    assert_eq!(list["categories"][0]["name"], "Основные");
    let mut broken = tariff.clone();
    broken["periods"][0]["prices"]["rub"] = json!(19950);
    let (status, body) = h
        .write(Method::POST, "/api/admin/tariffs", &owner, broken)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "tariff.invalid");
    let (status, _) = h
        .write(
            Method::PUT,
            &format!("/api/admin/tariffs/{id}"),
            &owner,
            tariff,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, squads) = h.get("/api/admin/infra/squads", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(squads[0]["tariffs"][0]["name"], "Стандарт");
    h.db.cleanup().await;
}

#[tokio::test]
async fn главная_лента_и_ноды_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let node: Node = serde_json::from_value(json!({
        "uuid": Uuid::from_u128(2), "name": "NL-2", "address": "198.51.100.22",
        "isConnected": false, "isDisabled": false, "isConnecting": false,
        "lastStatusMessage": "connect ETIMEDOUT", "usersOnline": 0
    }))
    .unwrap();
    h.remna.set_nodes(vec![node]);
    h.client(5_093_118_422).await;
    let (status, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["remna"]["nodes_total"], 1);
    assert_eq!(dashboard["remna"]["nodes_online"], 0);
    assert_eq!(dashboard["currency"], "USD");
    assert!(dashboard["subscriptions"].is_object() || dashboard["stats_error"].is_string());
    let (status, feed) = h.get("/api/admin/feed?kind=clients&limit=5", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(feed["items"][0]["title"], "Новый клиент");
    let (status, body) = h.get("/api/admin/feed?kind=planets", &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.invalid_query");
    let (status, nodes) = h.get("/api/admin/infra/nodes", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(nodes["summary"]["attention"], 1);
    assert_eq!(nodes["nodes"][0]["state"], "down");
    let restart = format!(
        "/api/admin/infra/nodes/{}/actions/restart",
        Uuid::from_u128(2)
    );
    let (status, body) = h.write(Method::POST, &restart, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(h.remna.calls().contains(&"restart_node".to_string()));
    let (status, _) = h
        .write(
            Method::POST,
            &format!(
                "/api/admin/infra/nodes/{}/actions/explode",
                Uuid::from_u128(2)
            ),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = h
        .write(
            Method::POST,
            &format!(
                "/api/admin/infra/nodes/{}/actions/restart",
                Uuid::from_u128(9)
            ),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        h.get("/api/admin/infra/billing", &owner).await.0,
        StatusCode::OK
    );
    let (status, remna) = h.get("/api/admin/remnawave/status", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(remna["ok"], true);
    h.services().set_remna(None);
    let (status, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(dashboard["remna_error"].is_string());
    assert_eq!(
        h.get("/api/admin/infra/nodes", &owner).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn платежи_список_и_ручная_отметка_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let client = h.client(5_093_118_422).await;
    let tariff = h.tariff().await;
    let payment = h.pending_payment(client, &tariff).await;
    let (status, page) = h.get("/api/admin/payments?status=pending", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let (status, body) = h.get("/api/admin/payments?status=lost", &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let mark = format!("/api/admin/payments/{}/mark-paid", payment.id.0);
    let (status, body) = h
        .write(Method::POST, &mark, &owner, json!({ "comment": "пришло" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"], "accepted");
    let (status, body) = h.write(Method::POST, &mark, &owner, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "payment.manual_status");
    let (status, page) = h.get("/api/admin/payments?status=paid", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["provider_txid"], Value::Null);
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/payments/{}/refund", payment.id.0),
            &owner,
            json!({ "method": "balance", "subscription_action": "keep" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["refund"]["status"], "refunded");
    assert_eq!(body["refund"]["amount"]["minor"], payment.amount.minor);
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/payments/{}/refund", payment.id.0),
            &owner,
            json!({ "method": "balance", "subscription_action": "keep" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "payment.refund_status");
    let (status, page) = h.get("/api/admin/payments?status=refunded", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    h.db.cleanup().await;
}

#[tokio::test]
async fn кабинет_входит_через_telegram_и_выходит() {
    let Some(h) = http().await else { return };
    let (status, _, body) = h
        .send(Method::POST, "/api/client/auth/telegram", None, None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "bot.not_configured");
    h.set_bot().await;
    let (status, _, start_body) = h
        .send(Method::POST, "/api/client/auth/telegram", None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{start_body}");
    let link = start_body["link"].as_str().unwrap();
    let poll = start_body["poll_token"].as_str().unwrap().to_string();
    let code = link.rsplit('=').next().unwrap().to_string();
    let (status, _, pending) = h
        .send(
            Method::POST,
            "/api/client/auth/telegram/status",
            None,
            Some(json!({ "poll_token": poll })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pending["status"], "pending");
    h.services()
        .auth()
        .confirm_telegram_login(
            &code,
            &TelegramProfile {
                id: 5_093_118_422,
                username: Some("morozov_a".into()),
                first_name: Some("Алексей".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
        )
        .await
        .unwrap();
    let (status, headers, confirmed) = h
        .send(
            Method::POST,
            "/api/client/auth/telegram/status",
            None,
            Some(json!({ "poll_token": poll })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(confirmed["status"], "confirmed");
    let cookie = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("sn_client=") && cookie.contains("SameSite=Lax"));
    let token = confirmed["login"]["token"].as_str().unwrap().to_string();
    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["channel"], "cabinet");
    let (status, _, again) = h
        .send(
            Method::POST,
            "/api/client/auth/telegram/status",
            None,
            Some(json!({ "poll_token": poll })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["status"], "expired");
    let (status, _, _) = h
        .send(Method::POST, "/api/client/auth/logout", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.get("/api/client/me", &token).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email",
            None,
            Some(json!({ "email": "alex@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/verify",
            None,
            Some(json!({ "email": "alex@example.com", "code": "123456" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "auth.code_invalid");
    h.db.cleanup().await;
}

#[tokio::test]
async fn частые_запросы_входа_получают_429() {
    let Some(h) = http().await else { return };
    h.set_bot().await;
    let limit = sn_services::limits::rules::TELEGRAM_START_IP.limit;
    for _ in 0..limit {
        let (status, _, _) = h
            .send(Method::POST, "/api/client/auth/telegram", None, None)
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _, body) = h
        .send(Method::POST, "/api/client/auth/telegram", None, None)
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["code"], "auth.rate_limited");
    h.db.cleanup().await;
}

#[tokio::test]
async fn кабинет_видит_свой_платёж_и_обращения() {
    let Some(h) = http().await else { return };
    let client = h.client(5_093_118_422).await;
    let stranger = h.client(42).await;
    let token = h.client_token(client).await;
    let stranger_token = h.client_token(stranger).await;
    let tariff = h.tariff().await;
    let payment = h.pending_payment(client, &tariff).await;
    let path = format!("/api/client/payments/{}", payment.public_id);
    let (status, body) = h.get(&path, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["amount"]["minor"], 699);
    assert_eq!(body["order_kind"], "new");
    assert_eq!(h.get(&path, &stranger_token).await.0, StatusCode::NOT_FOUND);
    let (status, opened) = h
        .write(
            Method::POST,
            "/api/client/support/tickets",
            &token,
            json!({ "subject": "Чек", "body": "Не пришёл чек об оплате" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let (status, list) = h.get("/api/client/support/tickets", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["channel"], "cabinet");
    let ticket = list[0]["id"].as_i64().unwrap();
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/client/support/tickets/{ticket}/close"),
            &stranger_token,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &token,
            json!({ "amount": "десять", "provider": "cryptopay" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.amount");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &token,
            json!({ "amount": "0.50", "provider": "cryptopay" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "balance.topup_out_of_range");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &token,
            json!({ "amount": "10.00", "provider": "cryptopay" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "checkout.method_unavailable");
    let (status, body) = h
        .write(Method::POST, "/api/client/trial", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "trial.disabled");
    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::TRIAL,
                &serde_json::to_value(sn_domain::settings::TrialSettings {
                    enabled: true,
                    squads: vec![SquadId(Uuid::from_u128(1))],
                    ..sn_domain::settings::TrialSettings::default()
                })
                .unwrap(),
                None,
            )
            .await
            .unwrap();
    }
    h.services().reload().await.unwrap();
    h.services().set_remna(Some(h.remna.clone()));
    let (status, body) = h
        .write(Method::POST, "/api/client/trial", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["order_id"].is_i64(), "{body}");
    let (status, body) = h
        .write(Method::POST, "/api/client/trial", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn клиенты_ищутся_и_история_собирается_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let first = h.client(5_093_118_422).await;
    let second = h.client(7_000_000_001).await;
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/admin/clients/{}/balance", first.0),
            &owner,
            json!({ "delta": { "minor": 420, "currency": "USD" }, "comment": "подарок", "idempotency_key": "h1" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, page) = h.get("/api/admin/clients", &owner).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let (status, found) = h.get("/api/admin/clients?q=5093118422", &owner).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let items = found["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["client"]["id"], first.0);
    assert_eq!(items[0]["name"], "Алексей");
    assert_eq!(items[0]["balance"]["minor"], 420);
    assert_eq!(items[0]["paid_total"]["minor"], 0);
    let (status, hidden) = h.get("/api/admin/clients?q=5093118422", &support).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hidden["items"][0]["balance"], Value::Null);
    assert_eq!(hidden["items"][0]["paid_total"], Value::Null);
    let (status, none) = h.get("/api/admin/clients?q=нет-такого", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(none["items"].as_array().unwrap().is_empty());
    let (status, body) = h.get("/api/admin/clients?segment=lost", &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, history) = h
        .get(&format!("/api/admin/clients/{}/history", first.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    let changed: Vec<&Value> = history
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "balance_changed")
        .collect();
    assert_eq!(changed.len(), 1, "{history}");
    assert_eq!(changed[0]["data"]["delta"]["minor"], 420);
    let (status, _) = h
        .get(&format!("/api/admin/clients/{}/history", second.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    h.db.cleanup().await;
}

#[tokio::test]
async fn оформление_сохраняется_и_видно_без_входа() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        site["appearance"],
        json!({ "admin": "side", "admin_theme": "dark", "client": "accent", "client_theme": "client" })
    );
    assert_eq!(site["currency"], "USD");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/appearance",
            &owner,
            json!({ "admin": "top", "admin_theme": "system", "client": "mono", "client_theme": "light" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["admin"], "top");
    let (_, me) = h.get("/api/admin/me", &owner).await;
    assert_eq!(me["appearance"]["admin"], "top");
    assert_eq!(me["appearance"]["admin_theme"], "system");
    assert!(me["email"].as_str().is_some_and(|e| e.contains('@')));
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["appearance"]["client"], "mono");
    assert_eq!(site["appearance"]["client_theme"], "light");
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/appearance",
            &owner,
            json!({ "admin": "middle" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    h.db.cleanup().await;
}

async fn web_state() -> (AppState, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("sn-web-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><title>STEALTHNET</title>",
    )
    .unwrap();
    std::fs::write(dir.join("assets/main-1a2b.js"), "console.log(1)").unwrap();
    let mut state = offline_state();
    state.config = Arc::new(ServerConfig {
        web_dir: Some(dir.clone()),
        ..ServerConfig::default()
    });
    (state, dir)
}

async fn fetch(state: &AppState, method: Method, path: &str) -> (StatusCode, HeaderMap, String) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn интерфейс_раздаётся_сервером_с_возвратом_на_index() {
    let (state, dir) = web_state().await;
    let (status, headers, body) = fetch(&state, Method::GET, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("STEALTHNET"));
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    let (status, headers, body) = fetch(&state, Method::GET, "/admin/clients/42").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("STEALTHNET"));
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    let (status, headers, _) = fetch(&state, Method::GET, "/app/plans").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key(header::X_FRAME_OPTIONS));
    let (status, headers, body) = fetch(&state, Method::GET, "/assets/main-1a2b.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "console.log(1)");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    let (status, headers, body) = fetch(&state, Method::GET, "/api/nothing-here").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("json")
    );
    assert!(body.contains("not_found"));
    let (status, _, _) = fetch(&state, Method::POST, "/admin").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn без_каталога_интерфейса_любой_путь_отвечает_json_404() {
    let state = offline_state();
    let (status, headers, _) = fetch(&state, Method::GET, "/admin").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("json")
    );
}

#[tokio::test]
async fn способ_подключения_и_оформления_страницы_remnawave() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    h.remna.set_subpage_configs(vec![
        sn_remnawave::SubpageConfigRef {
            uuid: second,
            name: "Для белых списков".into(),
            view_position: 2,
        },
        sn_remnawave::SubpageConfigRef {
            uuid: first,
            name: "Основное".into(),
            view_position: 1,
        },
    ]);
    let (status, list) = h
        .get("/api/admin/settings/remnawave/page-configs", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list[0]["name"], "Основное");
    assert_eq!(list[1]["uuid"], second.to_string());
    let (_, connect) = h.get("/api/admin/settings/connect", &owner).await;
    assert_eq!(connect, json!({ "mode": "ours", "page_config": null }));
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/connect",
            &owner,
            json!({ "mode": "remnawave", "page_config": second.to_string() }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "remnawave");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["connect"]["page_config"], second.to_string());
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/connect",
            &owner,
            json!({ "mode": "remnawave", "page_config": "не-идентификатор" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}
