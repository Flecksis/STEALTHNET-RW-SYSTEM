use super::*;
use crate::alerts::Keeper;
use sn_domain::settings::keys;
use sn_domain::support::TicketChannel;
use sn_services::support::OpenTicket;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_partial_json;

fn group_update(update_id: i64, chat: i64, text: &str) -> Update {
    serde_json::from_value(serde_json::json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id, "from": user(9_001),
            "chat": {"id": chat, "type": "supergroup", "title": "Админы", "is_forum": true},
            "date": 1, "text": text
        }
    }))
    .unwrap()
}

async fn forum_api(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path_regex(r"/createForumTopic$"))
        .respond_with(ok(serde_json::json!({
            "message_thread_id": 77, "name": "Поддержка", "icon_color": 7_322_096
        })))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/getChat$"))
        .respond_with(ok(serde_json::json!({
            "id": -100_500, "type": "supergroup", "title": "Админы", "is_forum": true
        })))
        .with_priority(1)
        .mount(server)
        .await;
}

async fn open(services: &Services, telegram: i64, body: &str) {
    let client = services
        .clients()
        .ensure_telegram(
            &sn_domain::client::TelegramProfile {
                id: telegram,
                username: Some("morozov_a".into()),
                first_name: Some("Алексей".into()),
                last_name: None,
                language_code: None,
                is_premium: false,
            },
            sn_domain::client::RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap()
        .0;
    services
        .support()
        .open(
            client.id,
            &OpenTicket {
                subscription_id: None,
                channel: TicketChannel::Cabinet,
                subject: None,
                body: body.into(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn уведомления_уходят_в_группу_по_темам() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    forum_api(&server).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set_as(keys::BOT_USERNAME, &"stealth_test_bot", None)
            .await
            .unwrap();
    }
    let link = services.alerts().link().await.unwrap();
    let start = link.link.split("startgroup=").nth(1).unwrap().to_string();
    handle_update(
        &ctx,
        group_update(1, -100_500, &format!("/start@stealth_test_bot {start}")),
    )
    .await;
    let config = services.alerts().config().await.unwrap();
    assert_eq!(config.chat_id, Some(-100_500));
    assert!(config.forum);
    assert!(last_text(&server).await.contains("Готово"));
    handle_update(
        &ctx,
        group_update(2, -100_777, &format!("/start@stealth_test_bot {start}")),
    )
    .await;
    assert_eq!(
        services.alerts().config().await.unwrap().chat_id,
        Some(-100_500)
    );
    assert!(last_text(&server).await.contains("устарела"));

    let keeper = Keeper::new(
        services.clone(),
        ctx.api.clone(),
        Duration::from_millis(10),
        Duration::from_millis(10),
    );
    keeper.step(true).await.unwrap();
    open(&services, 7_401, "Не подключается iPhone").await;
    keeper.step(true).await.unwrap();
    let last = sent(&server).await.last().cloned().unwrap();
    assert_eq!(last["chat_id"], -100_500);
    assert_eq!(last["message_thread_id"], 77);
    assert!(last["text"].as_str().unwrap().contains("Новое обращение"));
    assert_eq!(
        services.alerts().topics().await.unwrap().get("Поддержка"),
        Some(&77)
    );

    services
        .alerts()
        .remember_topic("Поддержка", 99)
        .await
        .unwrap();
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_partial_json(
            serde_json::json!({"message_thread_id": 99}),
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "ok": false, "error_code": 400, "description": "Bad Request: message thread not found"
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    open(&services, 7_402, "Деньги списались, подписки нет").await;
    keeper.step(true).await.unwrap();
    assert_eq!(
        services.alerts().topics().await.unwrap().get("Поддержка"),
        Some(&77)
    );
    let last = sent(&server).await.last().cloned().unwrap();
    assert_eq!(last["message_thread_id"], 77);
    assert!(
        last["text"]
            .as_str()
            .unwrap()
            .contains("Деньги списались, подписки нет")
    );

    services.alerts().request_test().await.unwrap();
    keeper.step(false).await.unwrap();
    let checks: Vec<String> = sent(&server)
        .await
        .into_iter()
        .filter_map(|b| b["text"].as_str().map(str::to_string))
        .filter(|t| t.contains("Проверка уведомлений"))
        .collect();
    assert_eq!(checks.len(), 13);
    assert!(
        checks
            .iter()
            .any(|t| t.contains("Сюда будут приходить: перенос в Remnawave и обратно."))
    );
    assert!(
        checks
            .iter()
            .any(|t| t.contains("Сюда будут приходить: сбои вебхука и флуд в боте."))
    );
    assert!(checks.iter().all(|t| !t.contains("remnawave")));
    db.cleanup().await;
}

#[tokio::test]
async fn нода_которая_лежит_уходит_в_группу_без_общего_обхода() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    forum_api(&server).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set_as(keys::BOT_USERNAME, &"stealth_test_bot", None)
            .await
            .unwrap();
    }
    let link = services.alerts().link().await.unwrap();
    let start = link.link.split("startgroup=").nth(1).unwrap().to_string();
    handle_update(
        &ctx,
        group_update(1, -100_500, &format!("/start@stealth_test_bot {start}")),
    )
    .await;
    let since = chrono::Utc::now() - chrono::Duration::minutes(5);
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO remna_node_states \
             (node_uuid, name, state, since, event, message, seen_at, down_at, alerted_at) \
             VALUES ($1, 'Germany', 'down', $2, 'node.connection_lost', 'connect ETIMEDOUT', $2, $2, NULL)",
        )
        .bind(uuid::Uuid::from_u128(0xde))
        .bind(since)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let keeper = Keeper::new(
        services.clone(),
        ctx.api.clone(),
        Duration::from_millis(10),
        Duration::from_millis(10),
    );
    keeper.nodes_step().await.unwrap();
    let texts: Vec<String> = sent(&server)
        .await
        .into_iter()
        .filter_map(|b| b["text"].as_str().map(str::to_string))
        .filter(|t| t.contains("Нода Germany не отвечает"))
        .collect();
    assert_eq!(texts.len(), 1, "{texts:?}");
    keeper.nodes_step().await.unwrap();
    let again = sent(&server)
        .await
        .into_iter()
        .filter(|b| {
            b["text"]
                .as_str()
                .is_some_and(|t| t.contains("Нода Germany не отвечает"))
        })
        .count();
    assert_eq!(again, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn ошибка_отправки_не_теряет_остаток_пачки() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    forum_api(&server).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set_as(keys::BOT_USERNAME, &"stealth_test_bot", None)
            .await
            .unwrap();
    }
    let link = services.alerts().link().await.unwrap();
    let start = link.link.split("startgroup=").nth(1).unwrap().to_string();
    handle_update(
        &ctx,
        group_update(1, -100_500, &format!("/start@stealth_test_bot {start}")),
    )
    .await;
    let keeper = Keeper::new(
        services.clone(),
        ctx.api.clone(),
        Duration::from_millis(10),
        Duration::from_millis(10),
    );
    keeper.step(true).await.unwrap();
    open(&services, 7_411, "Не подключается iPhone").await;
    open(&services, 7_412, "Медленно работает ночью").await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "ok": false, "error_code": 429, "description": "Too Many Requests: retry after 1",
            "parameters": {"retry_after": 1}
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    assert!(keeper.step(true).await.is_err());
    keeper.step(true).await.unwrap();
    keeper.step(true).await.unwrap();
    let texts = || async {
        sent(&server)
            .await
            .into_iter()
            .filter_map(|b| b["text"].as_str().map(str::to_string))
            .collect::<Vec<String>>()
    };
    let count = |all: &[String], needle: &str| all.iter().filter(|t| t.contains(needle)).count();
    let all = texts().await;
    assert_eq!(count(&all, "Не подключается iPhone"), 2);
    assert_eq!(count(&all, "Медленно работает ночью"), 1);
    assert!(services.alerts().pending().await.unwrap().is_empty());
    open(&services, 7_413, "Сломанное <b> сообщение").await;
    open(&services, 7_414, "Не открывается ссылка").await;
    Mock::given(method("POST"))
        .and(path_regex(r"/sendMessage$"))
        .and(body_partial_json(serde_json::json!({"chat_id": -100_500})))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "ok": false, "error_code": 400, "description": "Bad Request: can't parse entities"
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    keeper.step(true).await.unwrap();
    let all = texts().await;
    assert_eq!(count(&all, "Не открывается ссылка"), 1);
    assert!(services.alerts().pending().await.unwrap().is_empty());
    db.cleanup().await;
}
