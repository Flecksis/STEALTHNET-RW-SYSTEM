use super::*;
use crate::admins::NewAdminRequest;
use crate::ext_modules::{
    Change, ConfigUpdate, Credentials, Deliverer, InstallRequest, InstallResult, signing,
};
use base64::Engine;
use sn_domain::access::{AdminPrincipal, AdminRole, Permission};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{OrderId, PaymentId};
use sn_domain::money::{Amount, PayCurrency};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn session(admin_id: AdminId, role: AdminRole, extra: &[Permission]) -> AdminPrincipal {
    AdminPrincipal {
        admin_id,
        role,
        permissions: extra.to_vec(),
        session_id: 1,
        api_key: None,
    }
}

fn manifest(webhook: &str, permissions: &[&str], events: &[&str]) -> String {
    serde_json::json!({
        "format": 1,
        "id": "hello",
        "name": "Привет",
        "version": "1.0.0",
        "author": "Студия Пример",
        "webhook": webhook,
        "events": events,
        "permissions": permissions,
        "page": { "url": "https://hello.example.com/admin", "title": "Журнал" },
        "settings": [
            { "key": "greeting", "type": "text", "label": "Текст приветствия", "required": true },
            { "key": "crm_token", "type": "secret", "label": "Токен CRM" },
            { "key": "old_token", "type": "secret", "label": "Старый токен" },
            { "key": "limit", "type": "number", "label": "Лимит", "default": 20 }
        ]
    })
    .to_string()
}

fn confirm(source: &str) -> InstallRequest {
    let parsed = crate::ext_modules::manifest::parse(source).unwrap();
    InstallRequest {
        manifest: source.to_string(),
        permissions: parsed.permissions,
        events: parsed.events,
        update: None,
        webhook: parsed.webhook,
        page: parsed.page.map(|page| page.url),
    }
}

async fn local_env() -> Option<Env> {
    env_with_fallbacks(
        ModuleRegistry::catalog_only(),
        EnvFallbacks {
            ext_modules_loopback: true,
            ..EnvFallbacks::default()
        },
    )
    .await
}

async fn install(services: &Services, actor: &AdminPrincipal, source: &str) -> InstallResult {
    services
        .ext_modules()
        .install(actor, &confirm(source))
        .await
        .unwrap()
}

async fn push(services: &Services, event: DomainEvent) {
    let mut tx = services.store().begin().await.unwrap();
    tx.events().push(&event).await.unwrap();
    tx.commit().await.unwrap();
}

fn registered(client: i64) -> DomainEvent {
    DomainEvent::ClientRegistered {
        client_id: ClientId(client),
        source: RegistrationSource::Bot,
        start_param: Some("ref_7".into()),
    }
}

fn paid(client: i64) -> DomainEvent {
    DomainEvent::PaymentPaid {
        payment_id: PaymentId(client),
        order_id: OrderId(client),
        client_id: ClientId(client),
        order_kind: OrderKind::New,
        amount: Amount::new(69_900, PayCurrency::Rub),
        provider: ProviderId::from("yookassa"),
    }
}

async fn run_sql(services: &Services, sql: &str, admin: AdminId) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(sql)
        .bind(admin.0)
        .execute(conn.conn())
        .await
        .unwrap();
}

async fn queued(env: &Env) -> i64 {
    let mut conn = env.services.store().conn().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM ext_module_deliveries")
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

fn verify(credentials: &Credentials, request: &wiremock::Request) -> serde_json::Value {
    let header = |name: &str| {
        request
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap()
            .to_string()
    };
    let id = header("webhook-id");
    let timestamp: i64 = header("webhook-timestamp").parse().unwrap();
    let key = signing::key(&credentials.signing_secret).unwrap();
    assert_eq!(
        header("webhook-signature"),
        signing::signature(&key, &id, timestamp, &request.body).unwrap()
    );
    let mut forged = request.body.clone();
    forged.push(b' ');
    assert_ne!(
        header("webhook-signature"),
        signing::signature(&key, &id, timestamp, &forged).unwrap()
    );
    serde_json::from_slice(&request.body).unwrap()
}

#[tokio::test]
async fn установка_выдаёт_ключ_с_подтверждёнными_правами_и_секрет() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let source = manifest(
        "https://hello.example.com/events",
        &["clients_read", "money"],
        &["client_registered", "payment_paid"],
    );
    let preview = services
        .ext_modules()
        .preview(&by_owner, &source, None)
        .await
        .unwrap();
    assert!(preview.errors.is_empty(), "{:?}", preview.errors);
    let preview = preview.preview.unwrap();
    assert!(preview.installed.is_none());
    assert!(preview.sensitive);
    assert_eq!(
        preview
            .events
            .iter()
            .map(|e| (e.title, e.change))
            .collect::<Vec<_>>(),
        [
            ("Клиент зарегистрировался", Change::Same),
            ("Оплата прошла", Change::Same)
        ]
    );
    assert_eq!(preview.permissions[1].title, "Деньги");
    let mut tampered = confirm(&source);
    tampered.permissions = vec![Permission::ClientsRead];
    assert_eq!(
        code(services.ext_modules().install(&by_owner, &tampered).await),
        "ext_modules.confirm"
    );
    let installed = install(services, &by_owner, &source).await;
    assert!(installed.created);
    let credentials = installed.credentials.unwrap();
    assert!(credentials.api_key.starts_with("snk_"));
    assert!(credentials.signing_secret.starts_with("whsec_"));
    assert_eq!(
        installed.module.key.prefix.as_deref(),
        Some(&credentials.api_key[..12])
    );
    let principal = services
        .api_keys()
        .principal(&credentials.api_key)
        .await
        .unwrap();
    assert_eq!(
        principal.permissions,
        [Permission::ClientsRead, Permission::Money]
    );
    assert!(!principal.can(Permission::ClientsWrite));
    let me = services.ext_modules().me(&principal).await.unwrap();
    assert_eq!(me.id, "hello");
    assert_eq!(me.settings["limit"], 20);
    assert_eq!(me.settings["crm_token"], serde_json::Value::Null);
    assert_eq!(
        code(services.ext_modules().me(&by_owner).await),
        "ext_modules.not_module"
    );
    assert_eq!(
        code(
            services
                .ext_modules()
                .install(&principal, &confirm(&source))
                .await
        ),
        "auth.forbidden"
    );
    let list = services.ext_modules().list(&by_owner).await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].enabled);
    assert_eq!(list[0].warning, None);
    let keys = services.api_keys().list().await.unwrap();
    assert_eq!(keys[0].name, "Модуль «Привет»");
    env.db.cleanup().await;
}

#[tokio::test]
async fn права_модуля_не_шире_прав_установившего_админа() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let member = services
        .admins()
        .create(
            owner,
            &NewAdminRequest {
                email: "support@example.com".into(),
                role: AdminRole::Support,
            },
        )
        .await
        .unwrap()
        .admin
        .id;
    let source = manifest(
        "https://hello.example.com/events",
        &["clients_read", "money"],
        &["payment_paid"],
    );
    let plain = session(member, AdminRole::Support, &[]);
    assert_eq!(
        code(services.ext_modules().preview(&plain, &source, None).await),
        "auth.forbidden"
    );
    let only_modules = session(member, AdminRole::Support, &[Permission::Modules]);
    assert_eq!(
        code(
            services
                .ext_modules()
                .install(&only_modules, &confirm(&source))
                .await
        ),
        "auth.forbidden"
    );
    let installer = session(
        member,
        AdminRole::Support,
        &[Permission::Modules, Permission::Settings],
    );
    let result = services
        .ext_modules()
        .preview(&installer, &source, None)
        .await
        .unwrap();
    assert!(result.preview.is_none());
    assert!(
        result
            .errors
            .iter()
            .any(|p| p.message.contains("у вас нет права «Деньги»")),
        "{:?}",
        result.errors
    );
    assert_eq!(
        code(
            services
                .ext_modules()
                .install(&installer, &confirm(&source))
                .await
        ),
        "ext_modules.permissions"
    );
    let modest = manifest(
        "https://hello.example.com/events",
        &["clients_read"],
        &["client_registered"],
    );
    let installed = install(services, &installer, &modest).await;
    let token = installed.credentials.unwrap().api_key;
    let principal = services.api_keys().principal(&token).await.unwrap();
    assert_eq!(principal.admin_id, member);
    assert_eq!(principal.permissions, [Permission::ClientsRead]);
    env.db.cleanup().await;
}

#[tokio::test]
async fn обновление_описания_с_новыми_правами_меняет_права_ключа() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let first = manifest(
        "https://hello.example.com/events",
        &["clients_read"],
        &["client_registered"],
    );
    let credentials = install(services, &by_owner, &first)
        .await
        .credentials
        .unwrap();
    services
        .ext_modules()
        .update_config(
            &by_owner,
            "hello",
            &ConfigUpdate {
                config: serde_json::json!({
                    "greeting": "Здравствуйте!",
                    "crm_token": "crm-1",
                    "old_token": "old-1"
                })
                .as_object()
                .cloned()
                .unwrap(),
                clear: Vec::new(),
            },
        )
        .await
        .unwrap();
    let mut second: serde_json::Value = serde_json::from_str(&manifest(
        "https://hello.example.com/events",
        &["clients_read", "money"],
        &["client_registered", "payment_paid"],
    ))
    .unwrap();
    second["version"] = "1.1.0".into();
    second["settings"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["key"] != "old_token");
    let mut other = second.clone();
    other["id"] = "other".into();
    let second = second.to_string();
    let other = other.to_string();
    let preview = services
        .ext_modules()
        .preview(&by_owner, &second, Some("hello"))
        .await
        .unwrap()
        .preview
        .unwrap();
    assert_eq!(preview.installed.as_ref().unwrap().version, "1.0.0");
    let money = preview
        .permissions
        .iter()
        .find(|p| p.key == Permission::Money)
        .unwrap();
    assert_eq!(money.change, Change::Added);
    assert!(
        preview
            .events
            .iter()
            .any(|e| e.kind == "payment_paid" && e.change == Change::Added)
    );
    let mut wrong = confirm(&other);
    wrong.update = Some("hello".into());
    assert_eq!(
        code(services.ext_modules().install(&by_owner, &wrong).await),
        "ext_modules.other_module"
    );
    let mut request = confirm(&second);
    request.update = Some("hello".into());
    let updated = services
        .ext_modules()
        .install(&by_owner, &request)
        .await
        .unwrap();
    assert!(!updated.created);
    assert!(updated.credentials.is_none());
    assert_eq!(updated.module.version, "1.1.0");
    let principal = services
        .api_keys()
        .principal(&credentials.api_key)
        .await
        .unwrap();
    assert_eq!(
        principal.permissions,
        [Permission::ClientsRead, Permission::Money]
    );
    let me = services.ext_modules().me(&principal).await.unwrap();
    assert_eq!(me.settings["greeting"], "Здравствуйте!");
    assert_eq!(me.settings["crm_token"], "crm-1");
    assert!(me.settings.get("old_token").is_none());
    let mut conn = services.store().conn().await.unwrap();
    assert!(
        conn.secrets()
            .get("ext_module.hello.setting.old_token")
            .await
            .unwrap()
            .is_none()
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn смена_адреса_модуля_требует_подтверждения_и_пишется_в_журнал() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let first = manifest(
        "https://hello.example.com/events",
        &["clients_read"],
        &["client_registered"],
    );
    install(services, &by_owner, &first).await;
    let mut same: serde_json::Value = serde_json::from_str(&first).unwrap();
    same["version"] = "1.0.1".into();
    let mut quiet = confirm(&same.to_string());
    quiet.update = Some("hello".into());
    quiet.webhook = None;
    quiet.page = None;
    services
        .ext_modules()
        .install(&by_owner, &quiet)
        .await
        .unwrap();
    let mut moved: serde_json::Value = serde_json::from_str(&first).unwrap();
    moved["version"] = "1.1.0".into();
    moved["webhook"] = "https://collector.example.net/events".into();
    moved["page"]["url"] = "https://admin.example.net/hello".into();
    let moved = moved.to_string();
    let preview = services
        .ext_modules()
        .preview(&by_owner, &moved, Some("hello"))
        .await
        .unwrap()
        .preview
        .unwrap();
    assert_eq!(
        serde_json::to_value(&preview.moved).unwrap(),
        serde_json::json!([
            {
                "address": "webhook",
                "before": "https://hello.example.com/events",
                "after": "https://collector.example.net/events"
            },
            {
                "address": "page",
                "before": "https://hello.example.com/admin",
                "after": "https://admin.example.net/hello"
            }
        ])
    );
    let mut request = confirm(&moved);
    request.update = Some("hello".into());
    let mut stale = request.clone();
    stale.webhook = Some("https://hello.example.com/events".into());
    let mut silent = request.clone();
    silent.webhook = None;
    silent.page = None;
    let mut half = request.clone();
    half.page = Some("https://hello.example.com/admin".into());
    for refused in [&stale, &silent, &half] {
        assert_eq!(
            code(services.ext_modules().install(&by_owner, refused).await),
            "ext_modules.confirm"
        );
    }
    assert_eq!(
        services
            .ext_modules()
            .card("hello")
            .await
            .unwrap()
            .webhook
            .as_deref(),
        Some("https://hello.example.com/events")
    );
    services
        .ext_modules()
        .install(&by_owner, &request)
        .await
        .unwrap();
    let card = services.ext_modules().card("hello").await.unwrap();
    assert_eq!(
        card.webhook.as_deref(),
        Some("https://collector.example.net/events")
    );
    assert_eq!(card.page.unwrap().url, "https://admin.example.net/hello");
    let mut conn = services.store().conn().await.unwrap();
    let journal = conn
        .audit()
        .recent_by_action("ext_module.updated", 1)
        .await
        .unwrap();
    assert_eq!(journal[0].actor_id, Some(owner.0));
    assert_eq!(journal[0].details["version"], "1.1.0");
    assert_eq!(
        journal[0].details["moved"],
        serde_json::to_value(&preview.moved).unwrap()
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn удаление_отзывает_ключ_и_стирает_секреты() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let source = manifest(
        "https://hello.example.com/events",
        &["clients_read"],
        &["client_registered"],
    );
    let credentials = install(services, &by_owner, &source)
        .await
        .credentials
        .unwrap();
    push(services, registered(5)).await;
    assert_eq!(queued(&env).await, 1);
    let support = session(owner, AdminRole::Support, &[Permission::Modules]);
    assert_eq!(
        code(
            services
                .ext_modules()
                .remove(&support, "hello", &Default::default())
                .await
        ),
        "auth.forbidden"
    );
    services
        .ext_modules()
        .remove(&by_owner, "hello", &Default::default())
        .await
        .unwrap();
    assert!(
        services
            .api_keys()
            .principal(&credentials.api_key)
            .await
            .is_err()
    );
    assert!(services.api_keys().list().await.unwrap()[0].revoked);
    assert_eq!(queued(&env).await, 0);
    let mut conn = services.store().conn().await.unwrap();
    assert!(
        conn.secrets()
            .keys("ext_module.hello.")
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    assert_eq!(
        code(services.ext_modules().card_for(&by_owner, "hello").await),
        "not_found"
    );
    push(services, registered(6)).await;
    assert_eq!(queued(&env).await, 0);
    env.db.cleanup().await;
}

#[tokio::test]
async fn выключенный_модуль_не_получает_событий_и_его_ключ_не_работает() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let source = manifest(
        "https://hello.example.com/events",
        &["clients_read"],
        &["client_registered"],
    );
    let credentials = install(services, &by_owner, &source)
        .await
        .credentials
        .unwrap();
    push(services, registered(5)).await;
    push(
        services,
        DomainEvent::ClientUnblocked {
            client_id: ClientId(5),
        },
    )
    .await;
    assert_eq!(queued(&env).await, 1);
    services
        .ext_modules()
        .set_enabled(&by_owner, "hello", false)
        .await
        .unwrap();
    assert!(
        services
            .api_keys()
            .principal(&credentials.api_key)
            .await
            .is_err()
    );
    push(services, registered(6)).await;
    assert_eq!(queued(&env).await, 1);
    let mut conn = services.store().conn().await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM ext_module_deliveries")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    drop(conn);
    assert_eq!(status, "skipped");
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "INSERT INTO ext_module_deliveries (module_id, kind, payload, attempts) \
         VALUES ('hello', 'client_registered', '{}', 1)",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert_eq!(Deliverer::new(services.clone()).round().await.unwrap(), 0);
    let mut conn = services.store().conn().await.unwrap();
    let waiting: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ext_module_deliveries WHERE status = 'pending'")
            .fetch_one(conn.conn())
            .await
            .unwrap();
    drop(conn);
    assert_eq!(waiting, 0);
    assert_eq!(
        code(services.ext_modules().page_token(&by_owner, "hello").await),
        "ext_modules.disabled"
    );
    services
        .ext_modules()
        .set_enabled(&by_owner, "hello", true)
        .await
        .unwrap();
    assert!(
        services
            .api_keys()
            .principal(&credentials.api_key)
            .await
            .is_ok()
    );
    push(services, registered(7)).await;
    assert_eq!(queued(&env).await, 3);
    env.db.cleanup().await;
}

#[tokio::test]
async fn события_уходят_модулю_с_подписью_и_журналом() {
    let Some(env) = local_env().await else { return };
    let services = &env.services;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/events"))
        .respond_with(ResponseTemplate::new(200).set_body_string("принято"))
        .mount(&server)
        .await;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let source = manifest(
        &format!("{}/events", server.uri()),
        &["clients_read"],
        &["client_registered"],
    );
    let credentials = install(services, &by_owner, &source)
        .await
        .credentials
        .unwrap();
    push(services, registered(5)).await;
    let deliverer = Deliverer::new(services.clone());
    assert_eq!(deliverer.round().await.unwrap(), 1);
    assert_eq!(deliverer.round().await.unwrap(), 0);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = verify(&credentials, &requests[0]);
    assert_eq!(body["type"], "client_registered");
    assert_eq!(body["module"], "hello");
    assert_eq!(body["data"]["client_id"], 5);
    assert_eq!(body["data"]["source"], "bot");
    assert_eq!(body["data"]["start_param"], "ref_7");
    assert!(body["id"].as_str().unwrap().starts_with("evt_"));
    assert_eq!(
        requests[0]
            .headers
            .get("webhook-id")
            .unwrap()
            .to_str()
            .unwrap(),
        body["id"].as_str().unwrap()
    );
    let journal = services
        .ext_modules()
        .journal(&by_owner, "hello")
        .await
        .unwrap();
    assert_eq!(journal.attempts.len(), 1);
    let attempt = &journal.attempts[0];
    assert_eq!(attempt.title, "Клиент зарегистрировался");
    assert_eq!(attempt.http_status, Some(200));
    assert_eq!(attempt.status, "done");
    assert_eq!(attempt.response.as_deref(), Some("принято"));
    assert!(!attempt.can_retry);
    let ping = services
        .ext_modules()
        .ping(&by_owner, "hello")
        .await
        .unwrap();
    assert!(ping.ok);
    let requests = server.received_requests().await.unwrap();
    let body = verify(&credentials, &requests[1]);
    assert_eq!(body["type"], "ping");
    assert!(body["id"].as_str().unwrap().starts_with("msg_"));
    let api_key = services
        .api_keys()
        .principal(&credentials.api_key)
        .await
        .unwrap();
    assert_eq!(
        code(services.ext_modules().journal(&api_key, "hello").await),
        "auth.forbidden"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn повторы_с_паузами_до_предела_и_ручной_повтор() {
    let Some(env) = local_env().await else { return };
    let services = &env.services;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/events"))
        .respond_with(ResponseTemplate::new(500).set_body_string("упал"))
        .up_to_n_times(8)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/events"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let source = manifest(
        &format!("{}/events", server.uri()),
        &["clients_read"],
        &["client_registered"],
    );
    install(services, &by_owner, &source).await;
    push(services, registered(5)).await;
    let deliverer = Deliverer::new(services.clone());
    let mut moment = start();
    for attempt in 1..=crate::ext_modules::delivery::MAX_ATTEMPTS {
        assert_eq!(deliverer.round().await.unwrap(), 1, "попытка {attempt}");
        assert_eq!(deliverer.round().await.unwrap(), 0, "пауза после {attempt}");
        if let Some(pause) = crate::ext_modules::delivery::retry_delay(attempt) {
            moment += pause;
            env.clock.set(moment);
        }
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 8);
    env.clock.set(moment + Duration::days(1));
    assert_eq!(deliverer.round().await.unwrap(), 0);
    let journal = services
        .ext_modules()
        .journal(&by_owner, "hello")
        .await
        .unwrap();
    assert_eq!(journal.attempts.len(), 8);
    assert_eq!(journal.stats.failed, 1);
    let last = &journal.attempts[0];
    assert_eq!(last.status, "failed");
    assert_eq!(last.attempt, 8);
    assert_eq!(last.error.as_deref(), Some("модуль ответил кодом 500"));
    assert!(last.can_retry);
    assert!(!journal.attempts[1].can_retry);
    let card = services.ext_modules().card("hello").await.unwrap();
    assert!(card.warning.unwrap().contains("Не доставлено событий: 1"));
    assert!(
        services
            .ext_modules()
            .retry(&by_owner, "hello", last.delivery_id)
            .await
            .unwrap()
    );
    assert_eq!(deliverer.round().await.unwrap(), 1);
    let journal = services
        .ext_modules()
        .journal(&by_owner, "hello")
        .await
        .unwrap();
    assert_eq!(journal.attempts[0].status, "done");
    assert_eq!(journal.attempts[0].http_status, Some(204));
    assert_eq!(journal.stats.failed, 0);
    env.db.cleanup().await;
}

#[tokio::test]
async fn перенаправления_и_внутренние_адреса_не_выполняются() {
    let Some(env) = local_env().await else { return };
    let services = &env.services;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/events"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/inside", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(path("/inside"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let source = manifest(
        &format!("{}/events", server.uri()),
        &["clients_read"],
        &["client_registered"],
    );
    install(services, &by_owner, &source).await;
    push(services, registered(5)).await;
    let deliverer = Deliverer::new(services.clone());
    deliverer.round().await.unwrap();
    let journal = services
        .ext_modules()
        .journal(&by_owner, "hello")
        .await
        .unwrap();
    assert_eq!(journal.attempts[0].http_status, Some(302));
    assert!(
        journal.attempts[0]
            .error
            .as_deref()
            .unwrap()
            .contains("перенаправления не выполняются")
    );
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "UPDATE ext_modules SET manifest = jsonb_set(manifest, '{webhook}', '\"https://10.0.0.5/events\"')",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    push(services, registered(6)).await;
    deliverer.round().await.unwrap();
    let journal = services
        .ext_modules()
        .journal(&by_owner, "hello")
        .await
        .unwrap();
    assert_eq!(journal.attempts[0].http_status, None);
    assert!(
        journal.attempts[0]
            .error
            .as_deref()
            .unwrap()
            .contains("внутренней сети")
    );
    let ping = services
        .ext_modules()
        .ping(&by_owner, "hello")
        .await
        .unwrap();
    assert!(!ping.ok);
    assert!(ping.error.unwrap().contains("внутренней сети"));
    env.db.cleanup().await;
}

#[tokio::test]
async fn адрес_на_этом_же_сервере_закрыт_без_разрешения() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("принято"))
        .mount(&server)
        .await;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let local = manifest(
        &format!("{}/events", server.uri()),
        &["clients_read"],
        &["client_registered"],
    );
    let preview = services
        .ext_modules()
        .preview(&by_owner, &local, None)
        .await
        .unwrap();
    assert!(preview.preview.is_none());
    assert!(
        preview
            .errors
            .iter()
            .any(|p| p.path == "webhook" && p.message.contains("SN_EXT_MODULES_LOOPBACK")),
        "{:?}",
        preview.errors
    );
    let refused = services
        .ext_modules()
        .install(&by_owner, &confirm(&local))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "ext_modules.manifest");
    assert!(refused.to_string().contains("SN_EXT_MODULES_LOOPBACK"));
    let mut page = serde_json::from_str::<serde_json::Value>(&manifest(
        "https://hello.example.com/events",
        &["clients_read"],
        &["client_registered"],
    ))
    .unwrap();
    page["page"]["url"] = serde_json::json!("http://localhost:8090/admin");
    let preview = services
        .ext_modules()
        .preview(&by_owner, &page.to_string(), None)
        .await
        .unwrap();
    assert!(
        preview.errors.iter().any(|p| p.path == "page.url"),
        "{:?}",
        preview.errors
    );
    install(
        services,
        &by_owner,
        &manifest(
            "https://hello.example.com/events",
            &["clients_read"],
            &["client_registered"],
        ),
    )
    .await;
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "UPDATE ext_modules SET manifest = jsonb_set(jsonb_set(manifest, '{webhook}', to_jsonb($1::text)), \
         '{page,url}', to_jsonb($2::text))",
    )
    .bind(format!("{}/events", server.uri()))
    .bind(format!("{}/admin", server.uri()))
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    push(services, registered(5)).await;
    let deliverer = Deliverer::new(services.clone());
    assert_eq!(deliverer.round().await.unwrap(), 1);
    env.clock.set(start() + Duration::days(1));
    assert_eq!(deliverer.round().await.unwrap(), 0);
    let journal = services
        .ext_modules()
        .journal(&by_owner, "hello")
        .await
        .unwrap();
    assert_eq!(journal.attempts.len(), 1);
    let attempt = &journal.attempts[0];
    assert_eq!(attempt.status, "failed");
    assert_eq!(attempt.http_status, None);
    assert!(
        attempt
            .error
            .as_deref()
            .unwrap()
            .contains("SN_EXT_MODULES_LOOPBACK")
    );
    assert!(attempt.can_retry);
    let card = services.ext_modules().card("hello").await.unwrap();
    assert!(card.warning.unwrap().contains("SN_EXT_MODULES_LOOPBACK"));
    let ping = services
        .ext_modules()
        .ping(&by_owner, "hello")
        .await
        .unwrap();
    assert!(!ping.ok);
    assert!(ping.error.unwrap().contains("SN_EXT_MODULES_LOOPBACK"));
    assert_eq!(
        code(services.ext_modules().page_token(&by_owner, "hello").await),
        "ext_modules.page"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    env.db.cleanup().await;
    let Some(open) = local_env().await else {
        return;
    };
    let services = &open.services;
    let by_owner = session(super::owner(services).await, AdminRole::Owner, &[]);
    install(services, &by_owner, &local).await;
    push(services, registered(5)).await;
    assert_eq!(Deliverer::new(services.clone()).round().await.unwrap(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    let card = services.ext_modules().card("hello").await.unwrap();
    assert!(card.warning.is_none(), "{:?}", card.warning);
    open.db.cleanup().await;
}

#[tokio::test]
async fn события_не_уходят_модулю_без_действующего_ключа_и_права() {
    let Some(env) = local_env().await else { return };
    let services = &env.services;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let member = services
        .admins()
        .create(
            owner,
            &NewAdminRequest {
                email: "money@example.com".into(),
                role: AdminRole::Support,
            },
        )
        .await
        .unwrap()
        .admin
        .id;
    run_sql(
        services,
        "UPDATE admins SET permissions = '{money}' WHERE id = $1",
        member,
    )
    .await;
    let installer = session(
        member,
        AdminRole::Support,
        &[Permission::Modules, Permission::Settings, Permission::Money],
    );
    let source = manifest(
        &format!("{}/events", server.uri()),
        &["clients_read", "money"],
        &["client_registered", "payment_paid"],
    );
    let credentials = install(services, &installer, &source)
        .await
        .credentials
        .unwrap();
    push(services, paid(5)).await;
    assert_eq!(queued(&env).await, 1);
    run_sql(
        services,
        "UPDATE admins SET permissions = '{}' WHERE id = $1",
        member,
    )
    .await;
    push(services, paid(6)).await;
    push(services, registered(6)).await;
    assert_eq!(queued(&env).await, 2);
    let deliverer = Deliverer::new(services.clone());
    assert_eq!(deliverer.round().await.unwrap(), 2);
    let kinds = |requests: Vec<wiremock::Request>| -> Vec<String> {
        requests
            .iter()
            .map(|r| {
                verify(&credentials, r)["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    assert_eq!(
        kinds(server.received_requests().await.unwrap()),
        ["client_registered"]
    );
    let journal = services
        .ext_modules()
        .journal(&by_owner, "hello")
        .await
        .unwrap();
    let held = journal
        .attempts
        .iter()
        .find(|a| a.kind == "payment_paid")
        .unwrap();
    assert_eq!(held.status, "skipped");
    assert_eq!(held.http_status, None);
    assert!(held.error.as_deref().unwrap().contains("«Деньги»"));
    for (client, (stop, back, reason)) in (10..).zip([
        (
            "UPDATE api_keys SET revoked_at = now() WHERE created_by = $1",
            "UPDATE api_keys SET revoked_at = NULL WHERE created_by = $1",
            "отозван",
        ),
        (
            "UPDATE api_keys SET expires_at = '2026-11-15T00:00:00Z' WHERE created_by = $1",
            "UPDATE api_keys SET expires_at = NULL WHERE created_by = $1",
            "истёк",
        ),
        (
            "UPDATE admins SET disabled_at = now() WHERE id = $1",
            "UPDATE admins SET disabled_at = NULL WHERE id = $1",
            "отключён",
        ),
    ]) {
        push(services, registered(client)).await;
        let before = queued(&env).await;
        run_sql(services, stop, member).await;
        push(services, registered(client + 100)).await;
        assert_eq!(queued(&env).await, before, "{reason}");
        assert_eq!(deliverer.round().await.unwrap(), 1, "{reason}");
        let journal = services
            .ext_modules()
            .journal(&by_owner, "hello")
            .await
            .unwrap();
        let last = &journal.attempts[0];
        assert_eq!(last.status, "skipped", "{reason}");
        assert!(
            last.error.as_deref().unwrap().contains(reason),
            "{reason}: {:?}",
            last.error
        );
        run_sql(services, back, member).await;
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    push(services, registered(50)).await;
    assert_eq!(deliverer.round().await.unwrap(), 1);
    assert_eq!(
        kinds(server.received_requests().await.unwrap()),
        ["client_registered", "client_registered"]
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn токен_страницы_подписан_секретом_модуля_и_короткий() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let source = manifest(
        "https://hello.example.com/events",
        &["clients_read"],
        &["client_registered"],
    );
    let credentials = install(services, &by_owner, &source)
        .await
        .credentials
        .unwrap();
    let page = services
        .ext_modules()
        .page_token(&by_owner, "hello")
        .await
        .unwrap();
    assert_eq!(page.origin, "https://hello.example.com");
    assert!(
        page.url
            .starts_with("https://hello.example.com/admin?sn_token=")
    );
    assert_eq!(page.expires_at, start() + Duration::seconds(300));
    let key = signing::key(&credentials.signing_secret).unwrap();
    let claims = signing::read_page_token(&key, &page.token, "hello", start().timestamp()).unwrap();
    assert_eq!(claims.email, "owner@example.com");
    assert_eq!(claims.admin_id, owner.0);
    assert_eq!(claims.aud, "module:hello");
    assert_eq!(claims.role, AdminRole::Owner);
    assert!(
        signing::read_page_token(&key, &page.token, "hello", start().timestamp() + 301).is_none()
    );
    let token_part = page.url.split("sn_token=").nth(1).unwrap();
    assert_eq!(token_part, page.token);
    let principal = services
        .api_keys()
        .principal(&credentials.api_key)
        .await
        .unwrap();
    assert_eq!(
        code(services.ext_modules().page_token(&principal, "hello").await),
        "auth.forbidden"
    );
    let reader = session(owner, AdminRole::ReadOnly, &[]);
    assert_eq!(
        code(services.ext_modules().page_token(&reader, "hello").await),
        "auth.forbidden"
    );
    let parts: Vec<&str> = page.token.split('.').collect();
    let header: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(header["alg"], "HS256");
    env.db.cleanup().await;
}

#[tokio::test]
async fn настройки_модуля_сохраняются_и_доходят_до_модуля() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let source = manifest(
        "https://hello.example.com/events",
        &["clients_read"],
        &["client_registered"],
    );
    let credentials = install(services, &by_owner, &source)
        .await
        .credentials
        .unwrap();
    let ops = services.ext_modules();
    let update = |config: serde_json::Value, clear: &[&str]| ConfigUpdate {
        config: config.as_object().cloned().unwrap(),
        clear: clear.iter().map(|s| s.to_string()).collect(),
    };
    assert_eq!(
        code(
            ops.update_config(
                &by_owner,
                "hello",
                &update(serde_json::json!({ "limit": 5 }), &[])
            )
            .await
        ),
        "ext_modules.settings"
    );
    assert_eq!(
        code(
            ops.update_config(
                &by_owner,
                "hello",
                &update(
                    serde_json::json!({ "greeting": "Привет", "limit": "много" }),
                    &[]
                )
            )
            .await
        ),
        "ext_modules.settings"
    );
    assert_eq!(
        code(
            ops.update_config(
                &by_owner,
                "hello",
                &update(serde_json::json!({ "color": "red" }), &[])
            )
            .await
        ),
        "ext_modules.settings"
    );
    let view = ops
        .update_config(
            &by_owner,
            "hello",
            &update(
                serde_json::json!({ "greeting": " Привет! ", "limit": "7", "crm_token": "tok-1" }),
                &[],
            ),
        )
        .await
        .unwrap();
    assert_eq!(view.config["greeting"], "Привет!");
    assert_eq!(view.config["limit"], 7);
    assert_eq!(view.config["crm_token"]["filled"], true);
    let principal = services
        .api_keys()
        .principal(&credentials.api_key)
        .await
        .unwrap();
    let me = ops.me(&principal).await.unwrap();
    assert_eq!(me.settings["greeting"], "Привет!");
    assert_eq!(me.settings["limit"], 7);
    assert_eq!(me.settings["crm_token"], "tok-1");
    let mut conn = services.store().conn().await.unwrap();
    let (kind, payload): (String, serde_json::Value) =
        sqlx::query_as("SELECT kind, payload FROM ext_module_deliveries ORDER BY id DESC LIMIT 1")
            .fetch_one(conn.conn())
            .await
            .unwrap();
    drop(conn);
    assert_eq!(kind, "settings_changed");
    assert_eq!(
        payload["keys"],
        serde_json::json!(["crm_token", "greeting", "limit"])
    );
    ops.update_config(
        &by_owner,
        "hello",
        &update(serde_json::json!({}), &["crm_token"]),
    )
    .await
    .unwrap();
    assert_eq!(
        ops.me(&principal).await.unwrap().settings["crm_token"],
        serde_json::Value::Null
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn недоставленные_события_отправляются_снова_все_сразу() {
    let Some(env) = local_env().await else { return };
    let services = &env.services;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/events"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let source = manifest(
        &format!("{}/events", server.uri()),
        &["clients_read"],
        &["client_registered"],
    );
    install(services, &by_owner, &source).await;
    push(services, registered(5)).await;
    push(services, registered(6)).await;
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "INSERT INTO ext_module_deliveries (module_id, kind, payload, status, attempts) \
         VALUES ('hello', 'ping', '{}', 'failed', 1)",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE ext_module_deliveries SET status = 'failed', attempts = 8 WHERE kind <> 'ping'",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    let deliverer = Deliverer::new(services.clone());
    assert_eq!(deliverer.round().await.unwrap(), 0);
    let card = services.ext_modules().card("hello").await.unwrap();
    assert_eq!(card.stats.failed, 2);
    assert!(card.warning.unwrap().contains("Не доставлено событий: 2"));
    services
        .ext_modules()
        .set_enabled(&by_owner, "hello", false)
        .await
        .unwrap();
    assert_eq!(
        code(
            services
                .ext_modules()
                .retry_failed(&by_owner, "hello")
                .await
        ),
        "ext_modules.disabled"
    );
    services
        .ext_modules()
        .set_enabled(&by_owner, "hello", true)
        .await
        .unwrap();
    assert_eq!(
        services
            .ext_modules()
            .retry_failed(&by_owner, "hello")
            .await
            .unwrap(),
        2
    );
    assert_eq!(deliverer.round().await.unwrap(), 2);
    let journal = services
        .ext_modules()
        .journal(&by_owner, "hello")
        .await
        .unwrap();
    assert_eq!(journal.stats.failed, 0);
    assert!(journal.attempts.iter().all(|a| a.status == "done"));
    let order: Vec<i64> = journal
        .attempts
        .iter()
        .map(|a| a.event_id.unwrap())
        .collect();
    assert!(order.windows(2).all(|w| w[0] > w[1]), "{order:?}");
    let card = services.ext_modules().card("hello").await.unwrap();
    assert_eq!(card.warning, None);
    env.db.cleanup().await;
}

fn renamed(source: &str, id: &str, events: &[&str]) -> String {
    let mut value: serde_json::Value = serde_json::from_str(source).unwrap();
    value["id"] = id.into();
    value["events"] = serde_json::json!(events);
    value.to_string()
}

#[tokio::test]
async fn медленный_модуль_не_задерживает_события_других_модулей() {
    let Some(env) = local_env().await else { return };
    let services = &env.services;
    let slow = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(6)))
        .mount(&slow)
        .await;
    let fast = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&fast)
        .await;
    let owner = owner(services).await;
    let by_owner = session(owner, AdminRole::Owner, &[]);
    let base = |uri: String| manifest(&format!("{uri}/events"), &["clients_read"], &[]);
    install(
        services,
        &by_owner,
        &renamed(&base(slow.uri()), "slowpoke", &["client_registered"]),
    )
    .await;
    install(
        services,
        &by_owner,
        &renamed(&base(fast.uri()), "quick", &["client_blocked"]),
    )
    .await;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let worker = tokio::spawn(Deliverer::new(services.clone()).run(async move {
        let _ = stopped.await;
    }));
    push(services, registered(5)).await;
    let waited = std::time::Instant::now();
    while slow.received_requests().await.unwrap().is_empty() {
        assert!(waited.elapsed() < std::time::Duration::from_secs(5));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    push(
        services,
        DomainEvent::ClientBlocked {
            client_id: ClientId(6),
            reason: None,
        },
    )
    .await;
    let blocked_at = std::time::Instant::now();
    while fast.received_requests().await.unwrap().is_empty() {
        assert!(
            blocked_at.elapsed() < std::time::Duration::from_secs(4),
            "событие быстрому модулю ждёт медленный"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let journal = services
        .ext_modules()
        .journal(&by_owner, "slowpoke")
        .await
        .unwrap();
    assert!(journal.attempts.is_empty());
    let _ = stop.send(());
    worker.await.unwrap();
    env.db.cleanup().await;
}
