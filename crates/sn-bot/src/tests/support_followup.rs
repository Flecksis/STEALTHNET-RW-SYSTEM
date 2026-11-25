use super::*;
use sn_domain::client::{Languages, Locale};
use sn_domain::settings::{ClientAccess, SupportMode, keys};
use sn_services::Actor;
use sn_services::settings::CoreUpdate;
use sn_services::support::ReplyRequest;

const APP: &str = "https://vpn.example.com/app/support";

async fn keyboard(server: &MockServer) -> Vec<(String, String)> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .map(|b| {
            let target = b["web_app"]["url"]
                .as_str()
                .map(|url| format!("app {url}"))
                .unwrap_or_else(|| b["callback_data"].as_str().unwrap_or_default().to_string());
            (b["text"].as_str().unwrap_or_default().to_string(), target)
        })
        .collect()
}

fn row(label: &str, target: &str) -> (String, String) {
    (label.to_string(), target.to_string())
}

async fn set(services: &Services, key: &str, value: serde_json::Value) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings().set(key, &value, None).await.unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

#[tokio::test]
async fn уведомление_об_ответе_открывает_переписку_в_мини_приложении() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 713, "/start")).await;
    let client = client_of(&services, 713).await.unwrap();
    let (admin, _) = services
        .auth()
        .upsert_owner("owner@example.com", "пароль-владельца-уведомлений")
        .await
        .unwrap();
    let handler = NotifyHandler::new(&BotConfig {
        api_base: server.uri(),
        ..BotConfig::default()
    });
    let answer = async |params: serde_json::Value| {
        let done = notify(
            &services,
            &handler,
            client.id,
            notify::templates::SUPPORT_REPLY,
            params,
        )
        .await;
        assert_eq!(done, JobResult::Done);
        keyboard(&server).await
    };
    let params = serde_json::json!({"ticket_id": 41, "body": "Обновите подписку"});
    let reply = row("Ответить", "sup:41");
    let open = row("Открыть переписку", &format!("app {APP}/41"));
    let chat_only = std::slice::from_ref(&reply);
    let both = [open, reply.clone()];

    assert_eq!(answer(params.clone()).await, chat_only);

    set(
        &services,
        keys::PUBLIC_URL,
        serde_json::json!("https://vpn.example.com"),
    )
    .await;
    assert_eq!(answer(params.clone()).await, both);
    assert!(last_text(&server).await.contains("Обновите подписку"));
    assert_eq!(
        answer(serde_json::json!({"body": "Обновите подписку"})).await,
        [row("Ответить", "sup")]
    );

    let mode = async |mode: SupportMode| {
        services
            .settings_ops()
            .update_core(
                &CoreUpdate {
                    support_mode: Some(mode),
                    support_url: Some("https://t.me/help_chat".into()),
                    ..CoreUpdate::default()
                },
                admin,
            )
            .await
            .unwrap();
    };
    mode(SupportMode::Both).await;
    assert_eq!(answer(params.clone()).await, both);
    mode(SupportMode::Link).await;
    assert_eq!(answer(params.clone()).await, chat_only);
    mode(SupportMode::Tickets).await;

    for public in ["http://vpn.example.com", ""] {
        set(&services, keys::PUBLIC_URL, serde_json::json!(public)).await;
        assert_eq!(answer(params.clone()).await, chat_only, "{public}");
    }
    set(
        &services,
        keys::PUBLIC_URL,
        serde_json::json!("https://vpn.example.com"),
    )
    .await;
    let access = async |miniapp: bool| {
        services
            .settings_ops()
            .update_client_access(
                &ClientAccess {
                    miniapp,
                    ..ClientAccess::default()
                },
                admin,
            )
            .await
            .unwrap();
    };
    access(false).await;
    assert_eq!(answer(params.clone()).await, chat_only);
    access(true).await;

    let status = async |status: ClientStatus| {
        let mut conn = db.store.conn().await.unwrap();
        conn.clients()
            .set_status(client.id, status, None)
            .await
            .unwrap();
    };
    status(ClientStatus::Blocked).await;
    assert_eq!(answer(params.clone()).await, chat_only);
    status(ClientStatus::Active).await;

    set(
        &services,
        keys::LANGUAGES,
        serde_json::to_value(Languages {
            english: true,
            ..Languages::default()
        })
        .unwrap(),
    )
    .await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.clients()
            .set_locale(client.id, Locale::En)
            .await
            .unwrap();
    }
    assert_eq!(
        answer(params.clone()).await,
        [
            row("Open conversation", &format!("app {APP}/41")),
            row("Reply", "sup:41")
        ]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn кнопку_переписки_в_уведомлении_можно_скрыть() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 714, "/start")).await;
    let client = client_of(&services, 714).await.unwrap();
    set(
        &services,
        keys::PUBLIC_URL,
        serde_json::json!("https://vpn.example.com"),
    )
    .await;
    set(
        &services,
        "bot.content",
        serde_json::json!({"buttons": {"notify.btn.ticket": {"hidden": true}}}),
    )
    .await;
    let handler = NotifyHandler::new(&BotConfig {
        api_base: server.uri(),
        ..BotConfig::default()
    });
    let done = notify(
        &services,
        &handler,
        client.id,
        notify::templates::SUPPORT_REPLY,
        serde_json::json!({"ticket_id": 7, "body": "Готово"}),
    )
    .await;
    assert_eq!(done, JobResult::Done);
    assert_eq!(keyboard(&server).await, [row("Ответить", "sup:7")]);
    db.cleanup().await;
}

#[test]
fn кнопка_ответить_под_ответом_поддержки_знает_обращение() {
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let content = sn_domain::bot_content::Content::default();
    let with_id = notify::render(
        &content,
        notify::templates::SUPPORT_REPLY,
        &serde_json::json!({"ticket_id": 41, "body": "Обновите подписку"}),
        utc,
    )
    .unwrap();
    assert_eq!(
        with_id.keyboard,
        crate::telegram::Keyboard::new().button("Ответить", "sup:41")
    );
    let without_id = notify::render(
        &content,
        notify::templates::SUPPORT_REPLY,
        &serde_json::json!({"body": "Обновите подписку"}),
        utc,
    )
    .unwrap();
    assert_eq!(
        without_id.keyboard,
        crate::telegram::Keyboard::new().button("Ответить", "sup")
    );
}

#[tokio::test]
async fn ответ_клиента_в_боте_попадает_в_то_же_обращение() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 712, "/start")).await;
    handle_update(&ctx, text_update(2, 712, "просто текст")).await;
    assert!(last_buttons(&server).await.contains(&"sup".to_string()));
    let client = client_of(&services, 712).await.unwrap();
    let support = services.support();
    assert_eq!(
        support.telegram_ticket(client.id, None).await.unwrap(),
        None
    );

    handle_update(&ctx, callback_update(3, 712, "sup")).await;
    handle_update(&ctx, text_update(4, 712, "Не подключается на iPhone")).await;
    assert!(last_text(&server).await.contains("отправлено"));
    let ticket = support
        .telegram_ticket(client.id, None)
        .await
        .unwrap()
        .unwrap();
    support
        .reply(
            ticket,
            &ReplyRequest {
                body: "Обновите подписку".into(),
                close: false,
            },
            Actor::System,
        )
        .await
        .unwrap();

    handle_update(&ctx, callback_update(5, 712, &format!("sup:{}", ticket.0))).await;
    assert!(
        last_text(&server).await.contains(&format!("№{}", ticket.0)),
        "{}",
        last_text(&server).await
    );
    handle_update(&ctx, text_update(6, 712, "Не помогло")).await;
    assert!(last_text(&server).await.contains("добавлено"));
    handle_update(&ctx, text_update(7, 712, "Вот ещё подробности")).await;
    assert!(last_text(&server).await.contains("добавлено"));
    handle_update(&ctx, callback_update(8, 712, "sup")).await;
    handle_update(&ctx, text_update(9, 712, "И последний вопрос")).await;
    assert!(last_text(&server).await.contains("добавлено"));

    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(conn.tickets().count_open(client.id).await.unwrap(), 1);
    assert_eq!(conn.tickets().messages(ticket).await.unwrap().len(), 5);
    drop(conn);

    support.close(ticket, Actor::System).await.unwrap();
    handle_update(&ctx, text_update(10, 712, "привет")).await;
    assert!(last_buttons(&server).await.contains(&"sup".to_string()));
    handle_update(&ctx, callback_update(11, 712, &format!("sup:{}", ticket.0))).await;
    handle_update(&ctx, text_update(12, 712, "Снова не работает")).await;
    assert!(last_text(&server).await.contains("отправлено"));
    let fresh = support
        .telegram_ticket(client.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(fresh, ticket);
    db.cleanup().await;
}
