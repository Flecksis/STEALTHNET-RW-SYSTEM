use super::*;
use sn_domain::module::ModuleId;

async fn clicks(services: &Services) -> Vec<(String, String, String, i32)> {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT channel, source, campaign, sum(clicks)::int FROM utm_clicks \
         GROUP BY channel, source, campaign ORDER BY channel, source, campaign",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

fn click(source: &str, campaign: &str, count: i32) -> (String, String, String, i32) {
    ("bot".into(), source.into(), campaign.into(), count)
}

#[tokio::test]
async fn переход_по_метке_в_боте_считается_для_нового_и_зарегистрированного() {
    let Some((db, services, _server, ctx)) = setup().await else {
        return;
    };
    let (known, _) = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 611,
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
    handle_update(&ctx, text_update(1, 610, "/start s_tiktok__k_winter")).await;
    assert!(client_of(&services, 610).await.is_some());
    assert_eq!(clicks(&services).await, vec![click("tiktok", "winter", 1)]);
    handle_update(&ctx, text_update(2, 610, "/start s_tiktok__k_winter")).await;
    handle_update(&ctx, text_update(3, 611, "/start ref_ABCDEF__s_vk")).await;
    handle_update(&ctx, text_update(4, 611, "/start")).await;
    handle_update(&ctx, text_update(5, 611, "/start ref_ABCDEF")).await;
    handle_update(&ctx, text_update(6, 611, "/menu")).await;
    assert_eq!(
        clicks(&services).await,
        vec![click("tiktok", "winter", 2), click("vk", "", 1)]
    );
    services.events().dispatch_pending(50).await.unwrap();
    let mut conn = db.store.conn().await.unwrap();
    assert!(
        utm::for_client(&mut conn, known.id)
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    services
        .modules()
        .set_enabled(&db.store, ModuleId::Utm, false, None)
        .await
        .unwrap();
    handle_update(&ctx, text_update(7, 612, "/start s_tiktok__k_winter")).await;
    assert!(client_of(&services, 612).await.is_some());
    assert_eq!(
        clicks(&services).await,
        vec![click("tiktok", "winter", 2), click("vk", "", 1)]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn переход_через_экран_согласия_считается_один_раз() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                consent::DOCUMENTS_KEY,
                &serde_json::json!([{"title": "Оферта", "url": "https://example.com/offer"}]),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    handle_update(&ctx, text_update(1, 620, "/start s_vk__k_osen")).await;
    assert!(client_of(&services, 620).await.is_none());
    assert_eq!(last_buttons(&server).await, vec!["terms:ok".to_string()]);
    assert_eq!(clicks(&services).await, vec![click("vk", "osen", 1)]);
    handle_update(&ctx, text_update(2, 620, "/menu")).await;
    handle_update(&ctx, callback_update(3, 620, "terms:ok")).await;
    let client = client_of(&services, 620).await.unwrap();
    assert_eq!(clicks(&services).await, vec![click("vk", "osen", 1)]);
    services.events().dispatch_pending(50).await.unwrap();
    let mut conn = db.store.conn().await.unwrap();
    let origin = utm::for_client(&mut conn, client.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(origin.utm.campaign.as_deref(), Some("osen"));
    drop(conn);
    handle_update(&ctx, text_update(4, 620, "/start s_vk__k_osen")).await;
    assert_eq!(clicks(&services).await, vec![click("vk", "osen", 2)]);
    db.cleanup().await;
}
