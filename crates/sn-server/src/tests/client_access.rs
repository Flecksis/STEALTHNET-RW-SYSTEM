use super::*;

#[tokio::test]
async fn регистрация_по_паролю_и_настройка_доступа_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register",
            None,
            Some(json!({ "email": "olga@example.com", "password": "пароль-ольги-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "olga@example.com", "password": "пароль-ольги-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, access) = h.get("/api/admin/settings/client_access", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(access["site_login"], true);
    let (status, saved) = h
        .write(
            Method::PUT,
            "/api/admin/settings/client_access",
            &owner,
            json!({ "miniapp": false, "site_login": false, "self_devices": true, "self_reissue": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["site_login"], false);
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["email_auth"], serde_json::Value::Null);
    assert_eq!(site["client_access"]["reissue"], false);
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "olga@example.com", "password": "пароль-ольги-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.site_off");
    h.db.cleanup().await;
}

#[tokio::test]
async fn полный_экран_мини_приложения_через_http() {
    let Some(h) = http().await else { return };
    let path = "/api/admin/settings/client_access";
    let owner = h.owner().await;
    let anna = h.admin("anna@example.com", AdminRole::Admin).await;
    let help = h.admin("help@example.com", AdminRole::Support).await;
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["client_access"]["miniapp_fullscreen"], false);
    let (status, access) = h.get(path, &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(access["miniapp_fullscreen"], false);
    let with = |on: bool| json!({ "miniapp": true, "miniapp_fullscreen": on, "site_login": true, "self_devices": true, "self_reissue": true });
    assert_eq!(h.get(path, &help).await.0, StatusCode::FORBIDDEN);
    let (status, body) = h.write(Method::PUT, path, &help, with(true)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["client_access"]["miniapp_fullscreen"], false);
    let (status, saved) = h.write(Method::PUT, path, &anna, with(true)).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["miniapp_fullscreen"], true);
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["client_access"]["miniapp_fullscreen"], true);
    assert_eq!(site["client_access"]["miniapp"], true);
    let (status, page) = h
        .get("/api/admin/audit?action=settings.client_access", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let item = &page["items"][0];
    assert_eq!(item["title"], "Изменён доступ клиентов");
    assert_eq!(item["actor"]["email"], "anna@example.com");
    assert_eq!(item["target"]["link"], "/admin/settings/client");
    assert_eq!(
        item["changes"],
        json!([{ "label": "Открывать на весь экран", "before": "нет", "after": "да" }])
    );
    let (status, saved) = h
        .write(
            Method::PUT,
            path,
            &owner,
            json!({ "miniapp": true, "site_login": true, "self_devices": true, "self_reissue": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["miniapp_fullscreen"], false);
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["client_access"]["miniapp_fullscreen"], false);
    h.db.cleanup().await;
}

const BUTTONS: &str = "/api/admin/settings/bot/buttons";
const BUTTONS_MENU: &str = "/api/admin/settings/bot/buttons/menu";

async fn buttons_panel(api: &str, token: Option<&str>) -> Option<Http> {
    let h = http().await?;
    let services = Services::new(ServiceDeps {
        store: h.db.store.clone(),
        payments: Arc::new(ProviderRegistry::with_telegram_api(api)),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: h.clock.clone(),
        env: EnvFallbacks {
            public_url: Some("https://vpn.example.com".into()),
            bot_token: token.map(str::to_string),
            telegram_api: Some(api.to_string()),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    Some(Http {
        state: AppState::new(ServerConfig::default(), services),
        ..h
    })
}

async fn telegram_says(server: &wiremock::MockServer, method_name: &str, result: Value) {
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(format!(
            "/bot123456:TEST-token/{method_name}"
        )))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(json!({ "ok": true, "result": result })),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn кнопки_бота_видны_и_ставятся_только_с_правом_настроек() {
    let telegram = wiremock::MockServer::start().await;
    let Some(h) = buttons_panel(&telegram.uri(), Some("123456:TEST-token")).await else {
        return;
    };
    let foreign = json!({
        "type": "web_app", "text": "Кабинет", "web_app": { "url": "https://vpn.example.com/cabinet" }
    });
    telegram_says(&telegram, "getChatMenuButton", foreign).await;
    telegram_says(&telegram, "setChatMenuButton", json!(true)).await;
    telegram_says(
        &telegram,
        "getMe",
        json!({ "id": 123456, "is_bot": true, "username": "vpn_example_bot", "has_main_web_app": true }),
    )
    .await;
    let owner = h.owner().await;
    let help = h.admin("help@example.com", AdminRole::Support).await;
    let (status, _, _) = h.send(Method::GET, BUTTONS, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.get(BUTTONS, &help).await.0, StatusCode::FORBIDDEN);
    let (status, _) = h.write(Method::POST, BUTTONS_MENU, &help, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(telegram.received_requests().await.unwrap().is_empty());
    let (status, view) = h.get(BUTTONS, &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["username"], "vpn_example_bot");
    assert_eq!(view["main_app"], true);
    assert_eq!(view["same"], false);
    assert_eq!(view["menu"]["url"], "https://vpn.example.com/cabinet");
    assert_eq!(view["wanted"]["type"], "web_app");
    assert_eq!(view["wanted"]["url"], "https://vpn.example.com/app");
    let (status, view) = h.write(Method::POST, BUTTONS_MENU, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["applied"], true);
    let (status, page) = h
        .get("/api/admin/audit?action=settings.bot_menu", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let item = &page["items"][0];
    assert_eq!(
        item["title"],
        "Кнопка рядом с полем ввода поставлена заново"
    );
    assert_eq!(item["target"]["link"], "/admin/settings/client");
    assert_eq!(
        item["changes"],
        json!([{
            "label": "Значение",
            "before": "https://vpn.example.com/cabinet",
            "after": "https://vpn.example.com/app"
        }])
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn без_бота_и_без_связи_кнопки_отвечают_понятной_причиной() {
    let Some(h) = buttons_panel("http://127.0.0.1:9", None).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h.get(BUTTONS, &owner).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "bot.not_connected");
    h.db.cleanup().await;
    let Some(h) = buttons_panel("http://127.0.0.1:9", Some("123456:TEST-token")).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h.get(BUTTONS, &owner).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "bot.telegram_failed");
    assert_eq!(body["error"]["message"], "нет связи с Telegram");
    let (status, body) = h.write(Method::POST, BUTTONS_MENU, &owner, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn кнопка_рядом_с_полем_ввода_выключается_через_http() {
    let Some(h) = http().await else { return };
    let path = "/api/admin/settings/client_access";
    let owner = h.owner().await;
    let (_, access) = h.get(path, &owner).await;
    assert_eq!(access["menu_button"], true);
    let mut off = access.clone();
    off["menu_button"] = json!(false);
    let (status, saved) = h.write(Method::PUT, path, &owner, off).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["menu_button"], false);
    let (_, page) = h
        .get("/api/admin/audit?action=settings.client_access", &owner)
        .await;
    assert_eq!(
        page["items"][0]["changes"],
        json!([{ "label": "Кнопка рядом с полем ввода", "before": "да", "after": "нет" }])
    );
    h.db.cleanup().await;
}
