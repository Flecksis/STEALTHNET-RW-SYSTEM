use super::*;
use sn_domain::settings::secret_keys;
use sn_services::{BotProblem, BotState};

fn config(server: &MockServer) -> BotConfig {
    BotConfig {
        api_base: server.uri(),
        poll_timeout: Duration::from_secs(1),
        workers: 1,
        ..BotConfig::default()
    }
}

async fn idle_updates(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path_regex(r"/getUpdates$"))
        .respond_with(ok(serde_json::json!([])).set_delay(Duration::from_millis(100)))
        .mount(server)
        .await;
}

async fn wait_state(services: &Services, check: impl Fn(&BotState) -> bool) -> BotState {
    for _ in 0..80 {
        let state = services.bot_runtime().state();
        if check(&state) {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    services.bot_runtime().state()
}

async fn calls(server: &MockServer, name: &str) -> Vec<serde_json::Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path().ends_with(&format!("/{name}")))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(serde_json::Value::Null))
        .collect()
}

async fn without_token() -> Option<(TestDb, Services)> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(catalog::all()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    Some((db, services))
}

fn running(state: &BotState) -> bool {
    matches!(state, BotState::Running { .. })
}

fn failed_with(problem: BotProblem) -> impl Fn(&BotState) -> bool {
    move |state| state.problem() == Some(problem)
}

#[tokio::test]
async fn без_токена_бот_ждёт_и_сразу_стартует_после_сохранения_токена() {
    let Some((db, services)) = without_token().await else {
        return;
    };
    let server = telegram().await;
    idle_updates(&server).await;
    assert_eq!(services.bot_runtime().state(), BotState::Off);
    let cancel = CancellationToken::new();
    let running_bot = tokio::spawn(Bot::new(services.clone(), config(&server)).run(cancel.clone()));
    let idle = wait_state(&services, |s| *s == BotState::NoToken).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        sn_services::settings::write_secret(
            &mut conn,
            services.secrets(),
            secret_keys::BOT_TOKEN,
            Some(TOKEN),
            None,
        )
        .await
        .unwrap();
    }
    let saved_at = std::time::Instant::now();
    services.reload().await.unwrap();
    let started = wait_state(&services, running).await;
    let waited = saved_at.elapsed();
    cancel.cancel();
    running_bot.await.unwrap().unwrap();
    assert_eq!(idle, BotState::NoToken);
    match started {
        BotState::Running { username, .. } => {
            assert_eq!(username.as_deref(), Some("stealth_test_bot"));
        }
        other => panic!("бот не запустился после сохранения токена: {other:?}"),
    }
    assert!(
        waited < Duration::from_secs(3),
        "бот ждал проверки по таймеру: {waited:?}"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn чужой_вебхук_не_снимается_без_согласия_и_снимается_по_просьбе() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    Mock::given(method("POST"))
        .and(path_regex(r"/getWebhookInfo$"))
        .respond_with(ok(serde_json::json!({
            "url": "https://old.example.com/hook/SECRET-PATH", "pending_update_count": 3
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    idle_updates(&server).await;
    let cancel = CancellationToken::new();
    let running_bot = tokio::spawn(Bot::new(services.clone(), config(&server)).run(cancel.clone()));
    let blocked = wait_state(&services, failed_with(BotProblem::Webhook)).await;
    let deleted_before = calls(&server, "deleteWebhook").await.len();
    let polled_before = calls(&server, "getUpdates").await.len();
    services.bot_runtime().request_takeover();
    let taken = wait_state(&services, running).await;
    let deleted_after = calls(&server, "deleteWebhook").await.len();
    cancel.cancel();
    running_bot.await.unwrap().unwrap();
    match &blocked {
        BotState::Failed {
            problem: BotProblem::Webhook,
            detail,
            ..
        } => assert_eq!(detail.as_deref(), Some("old.example.com")),
        other => panic!("ожидалась остановка из-за чужого вебхука: {other:?}"),
    }
    assert!(!serde_json::to_string(&blocked).unwrap().contains("SECRET"));
    assert_eq!(deleted_before, 0, "чужой вебхук сняли без согласия");
    assert_eq!(polled_before, 0, "бот опрашивал Telegram при чужом вебхуке");
    assert!(running(&taken), "бот не забран после согласия: {taken:?}");
    assert_eq!(deleted_after, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn неверный_токен_виден_в_состоянии() {
    let Some((db, services, _server, _ctx)) = setup().await else {
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/getMe$"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "ok": false, "error_code": 401, "description": "Unauthorized"
        })))
        .mount(&server)
        .await;
    let cancel = CancellationToken::new();
    let running_bot = tokio::spawn(Bot::new(services.clone(), config(&server)).run(cancel.clone()));
    let state = wait_state(&services, failed_with(BotProblem::BadToken)).await;
    cancel.cancel();
    running_bot.await.unwrap().unwrap();
    match state {
        BotState::Failed {
            problem: BotProblem::BadToken,
            retry_at,
            at,
            ..
        } => assert!(retry_at.unwrap() - at >= chrono::Duration::minutes(5)),
        other => panic!("ожидался неверный токен: {other:?}"),
    }
    db.cleanup().await;
}

#[tokio::test]
async fn другой_опросчик_и_вебхук_при_опросе_видны_в_состоянии() {
    for (description, problem) in [
        (
            "Conflict: terminated by other getUpdates request; make sure that only one bot instance is running",
            BotProblem::Conflict,
        ),
        (
            "Conflict: can't use getUpdates method while webhook is active; use deleteWebhook to delete the webhook first",
            BotProblem::Webhook,
        ),
    ] {
        let Some((db, services, server, _ctx)) = setup().await else {
            return;
        };
        Mock::given(method("POST"))
            .and(path_regex(r"/getUpdates$"))
            .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
                "ok": false, "error_code": 409, "description": description
            })))
            .mount(&server)
            .await;
        let cancel = CancellationToken::new();
        let running_bot =
            tokio::spawn(Bot::new(services.clone(), config(&server)).run(cancel.clone()));
        let state = wait_state(&services, failed_with(problem)).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let polls = calls(&server, "getUpdates").await.len();
        cancel.cancel();
        running_bot.await.unwrap().unwrap();
        assert_eq!(state.problem(), Some(problem), "{description}: {state:?}");
        assert_eq!(polls, 1, "бот продолжил бодаться с другим опросчиком");
        db.cleanup().await;
    }
}

async fn first_offset(
    stored: Option<serde_json::Value>,
    remembered: Option<&str>,
) -> Option<serde_json::Value> {
    let (db, services, server, _ctx) = setup().await?;
    {
        let mut conn = db.store.conn().await.unwrap();
        if let Some(value) = stored {
            conn.bot_state().set(OFFSET_KEY, &value).await.unwrap();
        }
        if let Some(username) = remembered {
            conn.settings()
                .set(
                    sn_domain::settings::keys::BOT_USERNAME,
                    &serde_json::json!(username),
                    None,
                )
                .await
                .unwrap();
        }
    }
    idle_updates(&server).await;
    let cancel = CancellationToken::new();
    let running_bot = tokio::spawn(Bot::new(services.clone(), config(&server)).run(cancel.clone()));
    let mut first = None;
    for _ in 0..60 {
        if let Some(body) = calls(&server, "getUpdates").await.into_iter().next() {
            first = Some(body["offset"].clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cancel.cancel();
    running_bot.await.unwrap().unwrap();
    db.cleanup().await;
    first
}

#[tokio::test]
async fn смещение_апдейтов_не_переносится_на_другого_бота() {
    let Some(probe) = test_store().await else {
        return;
    };
    probe.cleanup().await;
    assert_eq!(
        first_offset(Some(serde_json::json!({"bot": 999, "offset": 500})), None).await,
        Some(serde_json::Value::Null)
    );
    assert_eq!(
        first_offset(
            Some(serde_json::json!({"bot": 999, "offset": 500})),
            Some("stealth_test_bot")
        )
        .await,
        Some(serde_json::Value::Null),
        "смещение другого бота не берётся, даже если имя совпало"
    );
    assert_eq!(
        first_offset(Some(serde_json::json!({"bot": 1, "offset": 88})), None).await,
        Some(serde_json::json!(88))
    );
    assert_eq!(
        first_offset(None, None).await,
        Some(serde_json::Value::Null)
    );
}

#[tokio::test]
async fn старое_смещение_без_бота_берётся_только_для_того_же_бота() {
    let Some(probe) = test_store().await else {
        return;
    };
    probe.cleanup().await;
    assert_eq!(
        first_offset(Some(serde_json::json!(77)), Some("stealth_test_bot")).await,
        Some(serde_json::json!(77)),
        "тот же бот продолжает со своего смещения"
    );
    assert_eq!(
        first_offset(Some(serde_json::json!(77)), Some("old_vpn_bot")).await,
        Some(serde_json::Value::Null),
        "после замены бота счёт начинается заново"
    );
    assert_eq!(
        first_offset(Some(serde_json::json!(77)), None).await,
        Some(serde_json::Value::Null),
        "неизвестно, чей это счётчик: начинаем заново"
    );
}
