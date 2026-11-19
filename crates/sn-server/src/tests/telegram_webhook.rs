use super::*;
use sn_services::bot_webhook::{MAX_BODY, SECRET_HEADER, WebhookKeys};

const BOT_TOKEN: &str = "123456:TEST-webhook-token-000000000";

async fn hook_http(public_url: Option<&str>, telegram_api: Option<&str>) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks {
            bot_token: Some(BOT_TOKEN.into()),
            telegram_api: telegram_api.map(str::to_string),
            ..EnvFallbacks::default()
        },
    });
    if let Some(url) = public_url {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &json!(url), None)
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

async fn deliver(h: &Http, segment: &str, token: Option<&str>, body: Vec<u8>) -> StatusCode {
    let mut builder = Request::post(format!("/api/webhooks/telegram/{segment}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(SECRET_HEADER, token);
    }
    router(h.state.clone())
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap()
        .status()
}

async fn queued(h: &Http) -> Vec<(i64, i64, bool)> {
    let mut conn = h.services().store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT bot_id, update_id, payload IS NOT NULL FROM bot_updates ORDER BY update_id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

fn update(id: i64) -> Vec<u8> {
    json!({
        "update_id": id,
        "message": {
            "message_id": id,
            "from": {"id": 9001, "is_bot": false, "first_name": "Ксения"},
            "chat": {"id": 9001, "type": "private"},
            "date": 1,
            "text": "/start"
        }
    })
    .to_string()
    .into_bytes()
}

#[tokio::test]
async fn вебхук_telegram_принимает_обновление_только_с_верными_секретами() {
    let Some(h) = hook_http(Some("https://panel.example.com/"), None).await else {
        return;
    };
    let owner = h.owner().await;
    let early = WebhookKeys::generate();
    assert_eq!(
        deliver(
            &h,
            &early.path_segment(),
            Some(&early.header_token()),
            update(1)
        )
        .await,
        StatusCode::NOT_FOUND,
        "при опросе адрес вебхука закрыт"
    );
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/bot",
            &owner,
            json!({ "updates": "webhook" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["updates"]["mode"], "webhook");
    assert_eq!(
        body["updates"]["address"],
        "https://panel.example.com/api/webhooks/telegram/…"
    );
    assert!(body["updates"]["blocker"].is_null());
    let keys = h.services().bot_webhook_keys().expect("ключи вебхука");
    let path = keys.path_segment();
    let token = keys.header_token();
    assert!(!body.to_string().contains(&path));
    assert!(!body.to_string().contains(&token));

    assert_eq!(
        deliver(&h, &path, Some(&token), update(1001)).await,
        StatusCode::OK
    );
    assert_eq!(
        deliver(&h, &path, Some(&token), update(1001)).await,
        StatusCode::OK,
        "повтор от Telegram тоже получает 200"
    );
    let stranger = WebhookKeys::generate();
    assert_eq!(
        deliver(&h, &path, Some(&stranger.header_token()), update(1002)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        deliver(&h, &path, None, update(1003)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        deliver(&h, &path, Some("короткий"), update(1003)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        deliver(&h, &stranger.path_segment(), Some(&token), update(1004)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        deliver(&h, &token, Some(&path), update(1004)).await,
        StatusCode::NOT_FOUND,
        "путь и заголовок не взаимозаменяемы"
    );
    assert_eq!(
        deliver(&h, &path, Some(&token), b"not json".to_vec()).await,
        StatusCode::OK
    );
    assert_eq!(
        deliver(
            &h,
            &path,
            Some(&token),
            json!({"message": {"text": "без номера"}})
                .to_string()
                .into_bytes()
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        deliver(&h, &path, Some(&token), vec![b' '; MAX_BODY + 1]).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(queued(&h).await, vec![(123_456, 1001, true)]);
    assert_eq!(
        crate::routes::shown_path(&format!("/api/webhooks/telegram/{path}")),
        "/api/webhooks/telegram/…"
    );
    assert_eq!(crate::routes::shown_path("/api/health"), "/api/health");

    let (status, body) = h.get("/api/admin/settings/bot", &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["updates"]["queue"]["pending"], 1);
    assert!(body["updates"]["queue"]["last_received_at"].is_string());

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/bot",
            &owner,
            json!({ "updates": "polling" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["updates"]["mode"], "polling");
    assert_eq!(
        deliver(&h, &path, Some(&token), update(1005)).await,
        StatusCode::NOT_FOUND
    );
    let mut conn = h.services().store().conn().await.unwrap();
    let actions: Vec<(String, Value)> = sqlx::query_as(
        "SELECT action, details FROM audit_log WHERE action = 'settings.bot_updates' ORDER BY id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(
        actions,
        vec![
            (
                "settings.bot_updates".to_string(),
                json!({"from": "опрос", "to": "вебхук"})
            ),
            (
                "settings.bot_updates".to_string(),
                json!({"from": "вебхук", "to": "опрос"})
            ),
        ]
    );
    let sealed: Vec<u8> =
        sqlx::query_scalar("SELECT ciphertext FROM secrets WHERE key = 'bot.webhook_secret'")
            .fetch_one(conn.conn())
            .await
            .unwrap();
    let stored = String::from_utf8_lossy(&sealed);
    assert!(!stored.contains(&keys.to_secret()));
    drop(conn);
    h.db.cleanup().await;
}

#[tokio::test]
async fn вебхук_не_включается_без_адреса_https_и_с_чужим_портом() {
    for (url, api, code) in [
        (None, None, "bot.webhook_needs_https"),
        (
            Some("http://panel.example.com"),
            None,
            "bot.webhook_needs_https",
        ),
        (
            Some("https://panel.example.com:9443"),
            None,
            "bot.webhook_port",
        ),
    ] {
        let Some(h) = hook_http(url, api).await else {
            return;
        };
        let owner = h.owner().await;
        let (status, body) = h.get("/api/admin/settings/bot", &owner).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["updates"]["blocker"], code, "{url:?}");
        let (status, body) = h
            .write(
                Method::PUT,
                "/api/admin/settings/bot",
                &owner,
                json!({ "updates": "webhook" }),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{url:?}: {body}");
        assert_eq!(body["error"]["code"], code, "{url:?}");
        assert!(body["error"]["message"].as_str().unwrap().contains(
            if code == "bot.webhook_port" {
                "443"
            } else {
                "https"
            }
        ));
        assert_eq!(
            h.services().settings().bot_updates.mode,
            sn_domain::settings::UpdatesMode::Polling
        );
        assert!(h.services().bot_webhook_keys().is_none());
        h.db.cleanup().await;
    }
    let Some(h) = hook_http(
        Some("https://panel.example.com:9443"),
        Some("http://telegram-bot-api:8081"),
    )
    .await
    else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/bot",
            &owner,
            json!({ "updates": "webhook" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["updates"]["custom_api"], true);
    assert_eq!(
        body["updates"]["address"],
        "https://panel.example.com:9443/api/webhooks/telegram/…"
    );
    h.db.cleanup().await;
}
