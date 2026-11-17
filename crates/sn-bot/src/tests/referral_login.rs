use super::*;

#[tokio::test]
async fn приглашение_с_сайта_доходит_через_вход_в_кабинет() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                sn_domain::settings::keys::BOT_USERNAME,
                &serde_json::json!("stealth_test_bot"),
                None,
            )
            .await
            .unwrap();
    }
    let (referrer, _) = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 720,
                username: None,
                first_name: Some("Ксения".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap();
    let code = {
        let mut conn = db.store.conn().await.unwrap();
        referral::ensure_code(&mut conn, referrer.id).await.unwrap()
    };
    let meta = sn_services::auth::RequestMeta {
        ip: Some("203.0.113.9".into()),
        user_agent: None,
    };
    let start = services
        .auth()
        .telegram_login_start_with(&meta, Some(&code.to_lowercase()))
        .await
        .unwrap();
    let param = start.link.rsplit_once("?start=").unwrap().1.to_string();
    assert!(param.ends_with(&format!("__ref_{code}")), "{param}");
    assert!(param.len() <= 64, "{param}");
    assert_eq!(
        crate::login::token_from_param(&param).map(str::len),
        Some(32)
    );
    handle_update(&ctx, text_update(1, 721, &format!("/start {param}"))).await;
    assert!(last_text(&server).await.contains("Вход в кабинет"));
    handle_update(&ctx, callback_update(2, 721, "auth:ok")).await;
    assert!(last_text(&server).await.contains("Вход подтверждён"));
    services.events().dispatch_pending(50).await.unwrap();
    let overview = referral::overview(&services.module_context(), referrer.id)
        .await
        .unwrap();
    assert_eq!(overview.invited, 1);
    let ignored = services
        .auth()
        .telegram_login_start_with(&meta, Some("<script>"))
        .await
        .unwrap();
    assert!(!ignored.link.contains("ref_"), "{}", ignored.link);
    db.cleanup().await;
}
