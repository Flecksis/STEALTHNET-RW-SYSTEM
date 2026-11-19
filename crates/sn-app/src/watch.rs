use sn_services::Services;
use sn_services::bot_webhook::UPDATES_CHANNEL;
use sn_services::settings::SETTINGS_CHANNEL;
use sqlx::postgres::PgListener;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const RECONNECT_DELAY: Duration = Duration::from_secs(5);

async fn reload(services: &Services) {
    if let Err(err) = services.reload().await {
        tracing::warn!(error = %err, "настройки не перечитаны");
    }
}

async fn listen(services: &Services, cancel: &CancellationToken) -> Result<(), sqlx::Error> {
    let mut listener = PgListener::connect_with(services.store().pool()).await?;
    listener
        .listen_all([SETTINGS_CHANNEL, UPDATES_CHANNEL])
        .await?;
    reload(services).await;
    services.bot_updates_signal().notify_one();
    let own = services.instance_id().to_string();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            message = listener.try_recv() => match message? {
                Some(notification) if notification.channel() == UPDATES_CHANNEL => {
                    if notification.payload() != own {
                        services.bot_updates_signal().notify_one();
                    }
                }
                Some(notification) if notification.payload() == own => {}
                Some(_) => {
                    tracing::info!("настройки изменены в другом процессе, перечитываем");
                    reload(services).await;
                }
                None => {
                    tracing::warn!("связь с базой для уведомлений о настройках восстановлена");
                    reload(services).await;
                    services.bot_updates_signal().notify_one();
                }
            }
        }
    }
}

pub async fn settings_changes(services: Services, cancel: CancellationToken) {
    loop {
        match listen(&services, &cancel).await {
            Ok(()) => return,
            Err(err) => {
                tracing::warn!(error = %err, "подписка на изменения настроек прервалась");
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(RECONNECT_DELAY) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_bot::{Bot, BotConfig};
    use sn_domain::ids::AdminId;
    use sn_domain::settings::UpdatesMode;
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_services::bot_webhook::Inbound;
    use sn_services::settings::BotUpdate;
    use sn_services::{BotState, EnvFallbacks, ServiceDeps, SystemClock};
    use sn_store::SecretBox;
    use std::sync::Arc;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "123456:TEST-two-processes-token-0000";

    fn ok(result: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({ "ok": true, "result": result }))
    }

    fn process(store: &sn_store::Store, key: &str) -> Services {
        Services::new(ServiceDeps {
            store: store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::new(sn_modules::catalog::all()).unwrap()),
            secrets: Some(SecretBox::from_base64(key).unwrap()),
            clock: Arc::new(SystemClock),
            env: EnvFallbacks {
                bot_token: Some(TOKEN.into()),
                public_url: Some("https://panel.example.com".into()),
                ..EnvFallbacks::default()
            },
        })
    }

    async fn bot_api() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/getMe$"))
            .respond_with(ok(serde_json::json!({
                "id": 123_456, "is_bot": true, "first_name": "Бот", "username": "two_process_bot"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"/getWebhookInfo$"))
            .respond_with(ok(
                serde_json::json!({ "url": "", "pending_update_count": 0 }),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"/getUpdates$"))
            .respond_with(ok(serde_json::json!([])).set_delay(Duration::from_millis(100)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"/sendMessage$"))
            .respond_with(ok(serde_json::json!({
                "message_id": 10, "chat": {"id": 1, "type": "private"}, "date": 0
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(
                r"/(setWebhook|deleteWebhook|setMyCommands|deleteMyCommands|setChatMenuButton)$",
            ))
            .respond_with(ok(serde_json::json!(true)))
            .mount(&server)
            .await;
        server
    }

    async fn called(server: &MockServer, name: &str, chat: Option<i64>) -> bool {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.url.path().ends_with(&format!("/{name}")))
            .any(|r| match chat {
                Some(chat) => serde_json::from_slice::<serde_json::Value>(&r.body)
                    .is_ok_and(|body| body["chat_id"] == chat),
                None => true,
            })
    }

    async fn wait(what: &str, deadline: Duration, mut check: impl AsyncFnMut() -> bool) {
        let until = tokio::time::Instant::now() + deadline;
        while tokio::time::Instant::now() < until {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("не дождались: {what}");
    }

    #[tokio::test]
    async fn обновление_из_вебхука_доходит_до_бота_в_другом_процессе() {
        let Some(db) = sn_store::testing::test_store().await else {
            return;
        };
        let key = SecretBox::generate_key_base64();
        let web = process(&db.store, &key);
        let bot = process(&db.store, &key);
        web.reload().await.unwrap();
        bot.reload().await.unwrap();
        let api = bot_api().await;
        let cancel = CancellationToken::new();
        let watcher = tokio::spawn(settings_changes(bot.clone(), cancel.clone()));
        let config = BotConfig {
            api_base: api.uri(),
            poll_timeout: Duration::from_secs(1),
            workers: 2,
            queue_recheck: Duration::from_secs(120),
            webhook_check_every: Duration::from_secs(120),
            ..BotConfig::default()
        };
        let runner = tokio::spawn(Bot::new(bot.clone(), config).run(cancel.clone()));
        wait(
            "бот запустился опросом",
            Duration::from_secs(6),
            async || matches!(bot.bot_runtime().state(), BotState::Running { .. }),
        )
        .await;
        assert_eq!(web.bot_runtime().state(), BotState::Off);

        web.settings_ops()
            .update_bot(
                &BotUpdate {
                    updates: Some(UpdatesMode::Webhook),
                    ..BotUpdate::default()
                },
                AdminId(1),
            )
            .await
            .unwrap();
        wait(
            "процесс бота узнал о вебхуке и выставил его",
            Duration::from_secs(6),
            async || called(&api, "setWebhook", None).await,
        )
        .await;
        assert_eq!(bot.settings().bot_updates.mode, UpdatesMode::Webhook);
        assert_eq!(bot.bot_webhook_keys(), web.bot_webhook_keys());

        let keys = web.bot_webhook_keys().unwrap();
        let update = serde_json::json!({
            "update_id": 77,
            "message": {
                "message_id": 77,
                "from": {"id": 9_303, "is_bot": false, "first_name": "Ксения", "language_code": "ru"},
                "chat": {"id": 9_303, "type": "private"},
                "date": 1,
                "text": "/start"
            }
        });
        let accepted = web
            .bot_webhook()
            .accept(
                &keys.path_segment(),
                Some(&keys.header_token()),
                update.to_string().as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(accepted, Inbound::Queued(77));
        wait(
            "бот в другом процессе ответил",
            Duration::from_secs(4),
            async || called(&api, "sendMessage", Some(9_303)).await,
        )
        .await;
        cancel.cancel();
        runner.await.unwrap().unwrap();
        watcher.await.unwrap();
        db.cleanup().await;
    }
}
