use super::*;
use sn_domain::settings::{ClientAccess, keys};

async fn app_urls(server: &MockServer) -> Vec<String> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["web_app"]["url"].as_str().map(str::to_string))
        .collect()
}

async fn public_url(services: &Services, url: &str) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings()
        .set(keys::PUBLIC_URL, &serde_json::json!(url), None)
        .await
        .unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

#[tokio::test]
async fn кнопка_мини_приложения_только_при_https_и_включённом_приложении() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 921, "/start")).await;
    assert!(app_urls(&server).await.is_empty());
    public_url(&services, "http://vpn.example.com").await;
    handle_update(&ctx, text_update(2, 921, "/start")).await;
    assert!(app_urls(&server).await.is_empty());
    public_url(&services, "https://vpn.example.com/").await;
    handle_update(&ctx, text_update(3, 921, "/start")).await;
    assert_eq!(app_urls(&server).await, ["https://vpn.example.com/app"]);
    let (owner, _) = services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    services
        .settings_ops()
        .update_client_access(
            &ClientAccess {
                miniapp: false,
                ..ClientAccess::default()
            },
            owner,
        )
        .await
        .unwrap();
    handle_update(&ctx, text_update(4, 921, "/start")).await;
    assert!(app_urls(&server).await.is_empty());
    db.cleanup().await;
}
