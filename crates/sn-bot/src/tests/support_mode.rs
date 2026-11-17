use super::*;
use sn_domain::settings::SupportMode;
use sn_services::settings::CoreUpdate;

async fn url_buttons(server: &MockServer) -> Vec<String> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["url"].as_str().map(str::to_string))
        .collect()
}

async fn support(services: &Services, mode: SupportMode, url: Option<&str>) {
    let (owner, _) = services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    services
        .settings_ops()
        .update_core(
            &CoreUpdate {
                support_url: url.map(str::to_string),
                support_mode: Some(mode),
                ..CoreUpdate::default()
            },
            owner,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn кнопка_поддержки_следует_режиму_поддержки() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 931, "/start")).await;
    assert!(last_buttons(&server).await.contains(&"sup".to_string()));
    assert!(url_buttons(&server).await.is_empty());
    handle_update(&ctx, text_update(2, 931, "/start support")).await;
    assert!(last_text(&server).await.contains("Опишите вопрос"));
    assert!(url_buttons(&server).await.is_empty());

    support(&services, SupportMode::Both, Some("https://t.me/help_chat")).await;
    handle_update(&ctx, text_update(3, 931, "/start")).await;
    assert!(last_buttons(&server).await.contains(&"sup".to_string()));
    handle_update(&ctx, callback_update(4, 931, "sup")).await;
    assert!(last_text(&server).await.contains("Опишите вопрос"));
    assert_eq!(url_buttons(&server).await, ["https://t.me/help_chat"]);

    support(&services, SupportMode::Link, None).await;
    handle_update(&ctx, text_update(5, 931, "/start")).await;
    assert!(!last_buttons(&server).await.contains(&"sup".to_string()));
    assert_eq!(url_buttons(&server).await, ["https://t.me/help_chat"]);
    handle_update(&ctx, callback_update(6, 931, "sup")).await;
    assert!(last_text(&server).await.contains("откроет чат поддержки"));
    assert_eq!(url_buttons(&server).await, ["https://t.me/help_chat"]);
    handle_update(&ctx, text_update(7, 931, "не работает")).await;
    let client = client_of(&services, 931).await.unwrap();
    assert_eq!(
        services
            .support()
            .telegram_ticket(client.id, None)
            .await
            .unwrap(),
        None
    );
    db.cleanup().await;
}
