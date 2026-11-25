use super::*;
use sn_domain::settings::keys;

async fn limited(services: &Services, config: serde_json::Value) {
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::BOT_USERNAME,
                &serde_json::json!("stealth_test_bot"),
                None,
            )
            .await
            .unwrap();
        conn.settings()
            .set(
                keys::LANGUAGES,
                &serde_json::json!({ "english": true, "default": "ru" }),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    let antibot = sn_domain::module::ModuleId::Antibot;
    services
        .modules()
        .set_enabled(services.store(), antibot, true, None)
        .await
        .unwrap();
    services
        .modules()
        .set_config(
            services.store(),
            antibot,
            sn_modules::ModuleConfig::from_value(&config),
            None,
        )
        .await
        .unwrap();
}

async fn link(services: &Services, ip: &str) -> String {
    let start = services
        .auth()
        .telegram_login_start(&sn_services::auth::RequestMeta {
            ip: Some(ip.into()),
            user_agent: None,
        })
        .await
        .unwrap();
    start.link.rsplit_once("?start=").unwrap().1.to_string()
}

async fn known(services: &Services, id: i64) -> bool {
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .find_by_identity(sn_domain::client::IdentityKind::Telegram, &id.to_string())
        .await
        .unwrap()
        .is_some()
}

fn english(update_id: i64, from: i64, text: &str) -> Update {
    serde_json::from_value(serde_json::json!({
        "update_id": update_id,
        "message": {
            "message_id": 10,
            "from": { "id": from, "is_bot": false, "first_name": "Alex", "language_code": "en" },
            "chat": { "id": from, "type": "private" },
            "date": 1,
            "text": text,
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn вход_с_сайта_сверх_предела_не_создаёт_аккаунт_а_бот_работает_как_обычно() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    limited(&services, serde_json::json!({ "site_address_day": 1 })).await;
    let ip = "203.0.113.50";
    let first = link(&services, ip).await;
    handle_update(&ctx, text_update(1, 8_001, &format!("/start {first}"))).await;
    assert!(known(&services, 8_001).await);
    let second = link(&services, ip).await;
    handle_update(&ctx, text_update(2, 8_002, &format!("/start {second}"))).await;
    assert!(!known(&services, 8_002).await);
    let text = last_text(&server).await;
    assert!(text.contains("создать новый с вашего адреса"), "{text}");
    assert!(text.contains("/start"), "{text}");
    assert!(text.chars().all(|c| !c.is_ascii_digit()), "{text}");
    let third = link(&services, ip).await;
    handle_update(&ctx, english(3, 8_003, &format!("/start {third}"))).await;
    assert!(!known(&services, 8_003).await);
    let text = last_text(&server).await;
    assert!(text.contains("from your address"), "{text}");
    handle_update(&ctx, text_update(4, 8_002, "/start")).await;
    assert!(known(&services, 8_002).await);
    let again = link(&services, ip).await;
    handle_update(&ctx, text_update(5, 8_002, &format!("/start {again}"))).await;
    assert!(last_text(&server).await.contains("Вход в кабинет"));
    handle_update(&ctx, callback_update(6, 8_002, "auth:ok")).await;
    assert!(last_text(&server).await.contains("Вход подтверждён"));
    let other = link(&services, "203.0.113.51").await;
    handle_update(&ctx, text_update(7, 8_004, &format!("/start {other}"))).await;
    assert!(known(&services, 8_004).await);
    db.cleanup().await;
}

#[tokio::test]
async fn сверх_минутного_предела_вход_с_сайта_ждёт_следующей_минуты() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    limited(&services, serde_json::json!({ "site_minute": 1 })).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO site_signups (via, created_at) VALUES ('code', now()), ('code', now() + interval '1 minute')",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let param = link(&services, "203.0.113.60").await;
    handle_update(&ctx, text_update(1, 8_101, &format!("/start {param}"))).await;
    assert!(!known(&services, 8_101).await);
    let text = last_text(&server).await;
    assert!(text.contains("через минуту"), "{text}");
    handle_update(&ctx, english(2, 8_102, &format!("/start {param}"))).await;
    let text = last_text(&server).await;
    assert!(text.contains("in a minute"), "{text}");
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("DELETE FROM site_signups")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    handle_update(&ctx, text_update(3, 8_101, &format!("/start {param}"))).await;
    assert!(known(&services, 8_101).await);
    assert!(last_text(&server).await.contains("Вход в кабинет"));
    db.cleanup().await;
}

#[tokio::test]
async fn просроченная_ссылка_входа_заводит_клиента_как_обычный_старт() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    limited(&services, serde_json::json!({ "site_minute": 1 })).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO site_signups (via, created_at) VALUES ('code', now()), ('code', now() + interval '1 minute')",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let param = link(&services, "203.0.113.70").await;
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE client_login_requests SET expires_at = now() - interval '1 minute'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    handle_update(&ctx, text_update(1, 8_201, &format!("/start {param}"))).await;
    assert!(known(&services, 8_201).await);
    assert!(last_text(&server).await.contains("устарел"));
    db.cleanup().await;
}
