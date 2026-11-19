use super::*;
use serde_json::{Value, json};
use sn_domain::ids::AdminId;
use sn_domain::settings::{FallbackReason, UpdatesMode, secret_keys};
use sn_services::BotState;
use sn_services::bot_webhook::{ALLOWED_UPDATES, Inbound};
use sn_services::settings::BotUpdate;
use std::sync::Mutex;

const PANEL: &str = "https://panel.example.com";

#[derive(Default)]
struct HookState {
    url: String,
    secret: String,
    pending: i64,
    error: Option<(i64, String)>,
    reject: Option<String>,
}

#[derive(Clone, Default)]
struct FakeHook(Arc<Mutex<HookState>>);

impl FakeHook {
    fn url(&self) -> String {
        self.0.lock().unwrap().url.clone()
    }

    fn set(&self, change: impl FnOnce(&mut HookState)) {
        change(&mut self.0.lock().unwrap());
    }
}

impl wiremock::Respond for FakeHook {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        let path = request.url.path();
        if path.ends_with("/setWebhook") {
            if let Some(reason) = &state.reject {
                return ResponseTemplate::new(400).set_body_json(json!({
                    "ok": false, "error_code": 400, "description": reason
                }));
            }
            let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
            state.url = body["url"].as_str().unwrap_or_default().to_string();
            state.secret = body["secret_token"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            return ok(json!(true));
        }
        if path.ends_with("/deleteWebhook") {
            state.url.clear();
            return ok(json!(true));
        }
        let mut info = json!({ "url": state.url, "pending_update_count": state.pending });
        if !state.url.is_empty() {
            info["max_connections"] = json!(40);
            info["allowed_updates"] = json!(ALLOWED_UPDATES);
        }
        if let Some((at, message)) = &state.error {
            info["last_error_date"] = json!(at);
            info["last_error_message"] = json!(message);
        }
        ok(info)
    }
}

async fn bot_api(updates: Vec<Value>) -> (MockServer, FakeHook) {
    let server = telegram().await;
    let hook = FakeHook::default();
    Mock::given(method("POST"))
        .and(path_regex(r"/(setWebhook|deleteWebhook|getWebhookInfo)$"))
        .respond_with(hook.clone())
        .with_priority(1)
        .mount(&server)
        .await;
    if !updates.is_empty() {
        Mock::given(method("POST"))
            .and(path_regex(r"/getUpdates$"))
            .respond_with(ok(json!(updates)))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path_regex(r"/getUpdates$"))
        .respond_with(ok(json!([])).set_delay(Duration::from_millis(100)))
        .mount(&server)
        .await;
    (server, hook)
}

async fn panel(public_url: &str) -> Option<(TestDb, Services)> {
    let db = test_store().await?;
    let services = panel_on(&db, public_url).await;
    Some((db, services))
}

async fn panel_on(db: &TestDb, public_url: &str) -> Services {
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(catalog::all()).unwrap()),
        secrets: Some(SecretBox::from_base64(SECRET_KEY).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks {
            bot_token: Some(TOKEN.into()),
            public_url: Some(public_url.into()),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    services
}

const SECRET_KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

fn quick(server: &MockServer) -> BotConfig {
    BotConfig {
        api_base: server.uri(),
        poll_timeout: Duration::from_secs(1),
        workers: 2,
        webhook_check_every: Duration::from_millis(150),
        webhook_alert_after: Duration::from_millis(300),
        webhook_fallback_after: Duration::from_millis(900),
        queue_recheck: Duration::from_millis(100),
        ..BotConfig::default()
    }
}

async fn switch(services: &Services, mode: UpdatesMode) {
    services
        .settings_ops()
        .update_bot(
            &BotUpdate {
                updates: Some(mode),
                ..BotUpdate::default()
            },
            AdminId(1),
        )
        .await
        .unwrap();
}

async fn calls(server: &MockServer, name: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with(&format!("/{name}")))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

async fn calls_by(server: &MockServer, name: &str, token: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path() == format!("/bot{token}/{name}"))
        .count()
}

async fn eventually<F, Fut>(what: &str, check: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..120 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("не дождались: {what}");
}

async fn replies_to(server: &MockServer, chat: i64) -> Vec<Value> {
    sent(server)
        .await
        .into_iter()
        .filter(|b| b["chat_id"] == chat)
        .collect()
}

fn start_update(update_id: i64, from: i64) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id, "from": user(from),
            "chat": {"id": from, "type": "private"}, "date": 1, "text": "/start"
        }
    })
}

async fn deliver(services: &Services, update: &Value) -> Inbound {
    let keys = services.bot_webhook_keys().expect("ключи вебхука");
    services
        .bot_webhook()
        .accept(
            &keys.path_segment(),
            Some(&keys.header_token()),
            update.to_string().as_bytes(),
        )
        .await
        .unwrap()
}

fn running(state: &BotState) -> bool {
    matches!(state, BotState::Running { .. })
}

async fn audit_actions(services: &Services) -> Vec<String> {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar("SELECT action FROM audit_log WHERE action LIKE 'settings.bot%' ORDER BY id")
        .fetch_all(conn.conn())
        .await
        .unwrap()
}

#[tokio::test]
async fn вебхук_включается_и_выключается_на_подставном_bot_api() {
    let (server, hook) = bot_api(Vec::new()).await;
    let Some((db, services)) = panel(PANEL).await else {
        return;
    };
    switch(&services, UpdatesMode::Webhook).await;
    let keys = services.bot_webhook_keys().unwrap();
    let cancel = CancellationToken::new();
    let bot = tokio::spawn(Bot::new(services.clone(), quick(&server)).run(cancel.clone()));
    eventually("бот выставил вебхук", || async {
        !calls(&server, "setWebhook").await.is_empty() && running(&services.bot_runtime().state())
    })
    .await;
    let set = calls(&server, "setWebhook").await;
    assert_eq!(set.len(), 1, "{set:?}");
    assert_eq!(
        set[0]["url"],
        format!("{PANEL}/api/webhooks/telegram/{}", keys.path_segment())
    );
    assert_eq!(set[0]["secret_token"], keys.header_token());
    assert_eq!(set[0]["max_connections"], 40);
    assert_eq!(set[0]["allowed_updates"], json!(ALLOWED_UPDATES));
    assert_eq!(set[0]["drop_pending_updates"], false);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        calls(&server, "getUpdates").await.is_empty(),
        "при вебхуке бот не опрашивает Telegram"
    );
    let snapshot = services.bot_webhook().snapshot().await.unwrap().unwrap();
    assert_eq!(snapshot.bot_id, Some(123_456));
    assert_eq!(snapshot.host.as_deref(), Some("panel.example.com"));
    assert!(snapshot.checked_at.is_some(), "состояние из getWebhookInfo");
    assert_eq!(snapshot.pending, Some(0));

    switch(&services, UpdatesMode::Polling).await;
    eventually(
        "бот снял вебхук и вернулся к опросу",
        || async {
            !calls(&server, "deleteWebhook").await.is_empty()
                && !calls(&server, "getUpdates").await.is_empty()
        },
    )
    .await;
    assert!(hook.url().is_empty());
    assert_eq!(calls(&server, "setWebhook").await.len(), 1);
    cancel.cancel();
    bot.await.unwrap().unwrap();
    assert_eq!(
        audit_actions(&services).await,
        ["settings.bot_updates", "settings.bot_updates"]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn обновление_из_вебхука_обрабатывается_так_же_как_из_опроса() {
    let chat = 9_101;
    let (polled_api, _) = bot_api(vec![start_update(501, chat)]).await;
    let Some((polled_db, polled)) = panel(PANEL).await else {
        return;
    };
    let cancel = CancellationToken::new();
    let bot = tokio::spawn(Bot::new(polled.clone(), quick(&polled_api)).run(cancel.clone()));
    eventually("ответ на /start из опроса", || async {
        !replies_to(&polled_api, chat).await.is_empty()
    })
    .await;
    cancel.cancel();
    bot.await.unwrap().unwrap();
    let by_polling = replies_to(&polled_api, chat).await;
    polled_db.cleanup().await;

    let (hook_api, _) = bot_api(Vec::new()).await;
    let Some((db, services)) = panel(PANEL).await else {
        return;
    };
    switch(&services, UpdatesMode::Webhook).await;
    let cancel = CancellationToken::new();
    let bot = tokio::spawn(Bot::new(services.clone(), quick(&hook_api)).run(cancel.clone()));
    eventually("бот работает от вебхука", || async {
        running(&services.bot_runtime().state()) && !calls(&hook_api, "setWebhook").await.is_empty()
    })
    .await;
    assert_eq!(
        deliver(&services, &start_update(501, chat)).await,
        Inbound::Queued(501)
    );
    eventually("ответ на /start из вебхука", || async {
        !replies_to(&hook_api, chat).await.is_empty()
    })
    .await;
    assert_eq!(
        deliver(&services, &start_update(501, chat)).await,
        Inbound::Duplicate(501),
        "повтор того же update_id не попадает в очередь"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    let by_webhook = replies_to(&hook_api, chat).await;
    assert_eq!(by_webhook.len(), by_polling.len(), "{by_webhook:?}");
    for (hooked, polled) in by_webhook.iter().zip(&by_polling) {
        assert_eq!(hooked["text"], polled["text"]);
        assert_eq!(hooked["reply_markup"], polled["reply_markup"]);
    }
    let mut conn = services.store().conn().await.unwrap();
    let row: (bool, bool) = sqlx::query_as(
        "SELECT taken_at IS NOT NULL, payload IS NULL FROM bot_updates WHERE update_id = 501",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert_eq!(row, (true, true), "обработанное обновление без текста");
    cancel.cancel();
    bot.await.unwrap().unwrap();
    assert!(calls(&hook_api, "getUpdates").await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn сломанный_вебхук_предупреждает_и_возвращает_бота_к_опросу() {
    let (server, hook) = bot_api(Vec::new()).await;
    let Some((db, services)) = panel(PANEL).await else {
        return;
    };
    assert!(services.alerts().collect().await.unwrap().is_empty());
    switch(&services, UpdatesMode::Webhook).await;
    hook.set(|state| {
        state.pending = 7;
        state.error = Some((
            chrono::Utc::now().timestamp(),
            "Connection timed out".into(),
        ));
    });
    let cancel = CancellationToken::new();
    let bot = tokio::spawn(Bot::new(services.clone(), quick(&server)).run(cancel.clone()));
    eventually("бот сам вернулся к опросу", || async {
        services.settings().bot_updates.mode == UpdatesMode::Polling
            && !calls(&server, "getUpdates").await.is_empty()
    })
    .await;
    let fallback = services.settings().bot_updates.fallback.clone().unwrap();
    assert_eq!(fallback.reason, FallbackReason::Undelivered);
    assert_eq!(fallback.error.as_deref(), Some("Connection timed out"));
    assert!(!calls(&server, "deleteWebhook").await.is_empty());
    assert!(hook.url().is_empty());
    assert_eq!(
        audit_actions(&services).await,
        [
            "settings.bot_updates",
            "settings.bot_webhook_broken",
            "settings.bot_webhook_fallback"
        ]
    );
    let alerts = services.alerts().collect().await.unwrap();
    let texts: Vec<&str> = alerts
        .iter()
        .filter(|a| a.kind == sn_domain::alerts::AlertKind::Bot)
        .map(|a| a.html.as_str())
        .collect();
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert!(texts[0].contains("Вебхук бота не получает обновления"));
    assert!(texts[0].contains("panel.example.com"));
    assert!(texts[0].contains("Connection timed out"));
    assert!(texts[0].contains("Ждут доставки: 7"));
    assert!(texts[1].contains("Бот вернулся к опросу"));
    assert!(texts[1].contains("не потерялись"));
    assert!(alerts.iter().all(|a| a.path == "/admin/settings/bot"));
    cancel.cancel();
    bot.await.unwrap().unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn вебхук_оживший_до_срока_не_возвращает_бота_к_опросу() {
    let (server, hook) = bot_api(Vec::new()).await;
    let Some((db, services)) = panel(PANEL).await else {
        return;
    };
    switch(&services, UpdatesMode::Webhook).await;
    hook.set(|state| {
        state.pending = 2;
        state.error = Some((
            chrono::Utc::now().timestamp(),
            "Read timeout expired".into(),
        ));
    });
    let config = BotConfig {
        webhook_fallback_after: Duration::from_secs(60),
        ..quick(&server)
    };
    let cancel = CancellationToken::new();
    let bot = tokio::spawn(Bot::new(services.clone(), config).run(cancel.clone()));
    eventually(
        "предупреждение о сломанном вебхуке",
        || async {
            audit_actions(&services)
                .await
                .contains(&"settings.bot_webhook_broken".to_string())
        },
    )
    .await;
    let snapshot = services.bot_webhook().snapshot().await.unwrap().unwrap();
    assert!(snapshot.broken_since.is_some());
    assert_eq!(snapshot.last_error.as_deref(), Some("Read timeout expired"));
    hook.set(|state| state.pending = 0);
    eventually("вебхук снова работает", || async {
        audit_actions(&services)
            .await
            .contains(&"settings.bot_webhook_restored".to_string())
    })
    .await;
    assert_eq!(services.settings().bot_updates.mode, UpdatesMode::Webhook);
    assert!(calls(&server, "deleteWebhook").await.is_empty());
    cancel.cancel();
    bot.await.unwrap().unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn адрес_который_telegram_не_принял_оставляет_бота_на_опросе() {
    let (server, hook) = bot_api(Vec::new()).await;
    hook.set(|state| {
        state.reject = Some(
            "Bad Request: bad webhook: Failed to resolve host: Name or service not known".into(),
        )
    });
    let Some((db, services)) = panel(PANEL).await else {
        return;
    };
    assert!(services.alerts().collect().await.unwrap().is_empty());
    switch(&services, UpdatesMode::Webhook).await;
    let cancel = CancellationToken::new();
    let bot = tokio::spawn(Bot::new(services.clone(), quick(&server)).run(cancel.clone()));
    eventually("бот остался на опросе", || async {
        services.settings().bot_updates.mode == UpdatesMode::Polling
            && !calls(&server, "getUpdates").await.is_empty()
    })
    .await;
    let fallback = services.settings().bot_updates.fallback.clone().unwrap();
    assert_eq!(fallback.reason, FallbackReason::Rejected);
    assert!(fallback.error.unwrap().contains("Failed to resolve host"));
    let alerts = services.alerts().collect().await.unwrap();
    assert!(
        alerts
            .iter()
            .any(|a| a.html.contains("Вебхук бота не включился")),
        "{alerts:?}"
    );
    cancel.cancel();
    bot.await.unwrap().unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn смена_адреса_панели_и_токена_перевыставляют_вебхук() {
    let (server, hook) = bot_api(Vec::new()).await;
    let Some((db, services)) = panel(PANEL).await else {
        return;
    };
    switch(&services, UpdatesMode::Webhook).await;
    let cancel = CancellationToken::new();
    let bot = tokio::spawn(Bot::new(services.clone(), quick(&server)).run(cancel.clone()));
    eventually("первый вебхук", || async {
        hook.url().starts_with(PANEL) && running(&services.bot_runtime().state())
    })
    .await;
    let first = calls(&server, "setWebhook").await;

    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                sn_domain::settings::keys::PUBLIC_URL,
                &json!("https://new.example.com"),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    eventually("вебхук на новом адресе", || async {
        hook.url()
            .starts_with("https://new.example.com/api/webhooks/telegram/")
    })
    .await;
    let second = calls(&server, "setWebhook").await;
    assert_eq!(second.len(), 2);
    assert_eq!(second[1]["secret_token"], first[0]["secret_token"]);

    let other = "654321:TEST-other-bot-token-00000000";
    {
        let mut conn = db.store.conn().await.unwrap();
        sn_services::settings::write_secret(
            &mut conn,
            services.secrets(),
            secret_keys::BOT_TOKEN,
            Some(other),
            None,
        )
        .await
        .unwrap();
    }
    services.reload().await.unwrap();
    eventually("вебхук нового бота", || async {
        calls_by(&server, "setWebhook", other).await == 1
            && running(&services.bot_runtime().state())
    })
    .await;
    assert_eq!(
        calls_by(&server, "deleteWebhook", TOKEN).await,
        1,
        "вебхук прежнего бота снят его токеном"
    );
    let third = calls(&server, "setWebhook").await;
    assert_ne!(third[2]["secret_token"], first[0]["secret_token"]);
    assert_ne!(third[2]["url"], second[1]["url"]);
    let keys = services.bot_webhook_keys().unwrap();
    assert_eq!(third[2]["secret_token"], keys.header_token());
    let snapshot = services.bot_webhook().snapshot().await.unwrap().unwrap();
    assert_eq!(snapshot.bot_id, Some(654_321));
    cancel.cancel();
    bot.await.unwrap().unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn второй_процесс_не_берёт_обработку_пока_работает_первый() {
    let (server, _) = bot_api(Vec::new()).await;
    let Some((db, first)) = panel(PANEL).await else {
        return;
    };
    let second = panel_on(&db, PANEL).await;
    let cancel = CancellationToken::new();
    let one = tokio::spawn(Bot::new(first.clone(), quick(&server)).run(cancel.clone()));
    eventually("первый процесс работает", || async {
        running(&first.bot_runtime().state())
    })
    .await;
    let other = CancellationToken::new();
    let two = tokio::spawn(Bot::new(second.clone(), quick(&server)).run(other.clone()));
    eventually("второй процесс ждёт", || async {
        second.bot_runtime().state().problem() == Some(sn_services::BotProblem::Duplicate)
    })
    .await;
    cancel.cancel();
    one.await.unwrap().unwrap();
    second.bot_runtime().wake();
    eventually(
        "второй процесс подхватил бота",
        || async { running(&second.bot_runtime().state()) },
    )
    .await;
    other.cancel();
    two.await.unwrap().unwrap();
    db.cleanup().await;
}
