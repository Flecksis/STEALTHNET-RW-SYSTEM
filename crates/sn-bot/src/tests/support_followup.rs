use super::*;
use sn_services::Actor;
use sn_services::support::ReplyRequest;

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
