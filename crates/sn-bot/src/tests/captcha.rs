use super::*;
use sn_domain::settings::keys;

async fn trial_ready(services: &Services, url: Option<&str>) {
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::TRIAL,
                &serde_json::to_value(sn_domain::settings::TrialSettings::single(
                    true,
                    3,
                    vec![sn_domain::ids::SquadId(uuid::Uuid::from_u128(1))],
                    sn_domain::traffic::TrafficPolicy::UNLIMITED,
                    1,
                ))
                .unwrap(),
                None,
            )
            .await
            .unwrap();
        if let Some(url) = url {
            conn.settings()
                .set(keys::PUBLIC_URL, &serde_json::json!(url), None)
                .await
                .unwrap();
        }
    }
    services.reload().await.unwrap();
    services.set_remna(Some(Arc::new(sn_remnawave::fake::FakeRemnawave::new())));
    services
        .modules()
        .set_enabled(
            services.store(),
            sn_domain::module::ModuleId::Antibot,
            true,
            None,
        )
        .await
        .unwrap();
    services
        .modules()
        .set_config(
            services.store(),
            sn_domain::module::ModuleId::Antibot,
            sn_modules::ModuleConfig::from_value(&serde_json::json!({
                "mode": "confirm",
                "burst_count": 0,
                "check_username": false,
                "captcha": "turnstile",
                "captcha_site_key": "0x4AAAAAAAsite-key",
                "captcha_secret": "0x4AAAAAAAsecret-key",
            })),
            None,
        )
        .await
        .unwrap();
}

async fn last_markup(server: &MockServer) -> serde_json::Value {
    sent(server)
        .await
        .last()
        .map(|b| b["reply_markup"].clone())
        .unwrap_or_default()
}

fn verify_urls(markup: &serde_json::Value, kind: &str) -> Vec<String> {
    markup["inline_keyboard"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter_map(|b| {
            let url = if kind == "web_app" {
                b["web_app"]["url"].as_str()
            } else {
                b["url"].as_str()
            };
            url.map(str::to_string)
        })
        .filter(|url| url.contains("/verify/"))
        .collect()
}

#[tokio::test]
async fn с_капчей_пробник_в_боте_открывает_страницу_проверки() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    trial_ready(&services, Some("https://vpn.example.com/")).await;
    handle_update(&ctx, text_update(1, 931, "/start")).await;
    handle_update(&ctx, callback_update(2, 931, "trial")).await;
    let text = last_text(&server).await;
    assert!(text.contains("пройдите проверку «Я не робот»"), "{text}");
    let urls = verify_urls(&last_markup(&server).await, "web_app");
    assert_eq!(urls.len(), 1, "{urls:?}");
    let token = urls[0]
        .strip_prefix("https://vpn.example.com/verify/")
        .unwrap()
        .to_string();
    assert_eq!(token.len(), 32);
    assert!(
        !last_buttons(&server)
            .await
            .iter()
            .any(|data| data.starts_with("ab:"))
    );
    let ctx_modules = services.module_context();
    let page = sn_modules::catalog::antibot::checks::page_check(&ctx_modules, &token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.target, "trial");
    let client = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(sn_domain::client::IdentityKind::Telegram, "931")
            .await
            .unwrap()
            .unwrap()
    };
    let old = sn_modules::catalog::antibot::challenge(
        &ctx_modules,
        client.id,
        sn_modules::catalog::antibot::TRIAL,
        sn_modules::catalog::antibot::Surface::Bot,
    )
    .await
    .unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE antibot_checks SET created_at = now() - interval '5 seconds' WHERE client_id = $1",
        )
        .bind(client.id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    handle_update(&ctx, callback_update(3, 931, &format!("ab:{}", old.token))).await;
    let urls = verify_urls(&last_markup(&server).await, "web_app");
    assert_eq!(urls.len(), 1, "{urls:?}");
    db.cleanup().await;
}

#[tokio::test]
async fn с_капчей_без_https_кнопка_ведёт_ссылкой_а_без_адреса_пробник_недоступен() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    trial_ready(&services, Some("http://vpn.example.com")).await;
    handle_update(&ctx, text_update(1, 932, "/start")).await;
    handle_update(&ctx, callback_update(2, 932, "trial")).await;
    let urls = verify_urls(&last_markup(&server).await, "url");
    assert_eq!(urls.len(), 1, "{urls:?}");
    assert!(urls[0].starts_with("http://vpn.example.com/verify/"));
    db.cleanup().await;
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    trial_ready(&services, None).await;
    handle_update(&ctx, text_update(1, 933, "/start")).await;
    handle_update(&ctx, callback_update(2, 933, "trial")).await;
    let text = last_text(&server).await;
    assert!(text.contains("страница проверки не открывается"), "{text}");
    db.cleanup().await;
}
