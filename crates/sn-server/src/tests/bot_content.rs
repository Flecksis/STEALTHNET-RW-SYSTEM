use super::*;
use sn_domain::bot_content::{CATALOG, Value as Var};

async fn send_bytes(
    h: &Http,
    method: Method,
    path: &str,
    token: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, body)
}

fn slot<'a>(view: &'a Value, name: &str) -> &'a Value {
    view["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["slot"] == name)
        .unwrap()
}

#[tokio::test]
async fn тексты_и_кнопки_бота_правятся_из_админки() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, view) = h.get("/api/admin/bot/content", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["entries"].as_array().unwrap().len(), CATALOG.len());
    assert_eq!(view["menu_columns"], 1);
    assert_eq!(view["caption_max"], 1024);
    let greeting = view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "menu.greeting")
        .unwrap();
    assert_eq!(greeting["optional"], true);
    assert_eq!(greeting["vars"][1]["name"], "name");
    let (status, _) = h.get("/api/admin/bot/content", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, saved) = h
        .write(
            Method::PUT,
            "/api/admin/bot/content",
            &owner,
            json!({
                "texts": { "menu.greeting": "Привет, {name}! <b>Рады видеть</b> 👋", "menu.title": "<b>{brand}</b>" },
                "buttons": {
                    "menu.btn.buy": { "text": "🛒 Купить", "emoji_id": "5368324170671202286", "style": "success" },
                    "menu.btn.balance": { "hidden": true }
                },
                "menu_columns": 2
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["menu_columns"], 2);
    let entries = saved["entries"].as_array().unwrap();
    let find = |key: &str| entries.iter().find(|e| e["key"] == key).unwrap().clone();
    assert_eq!(
        find("menu.greeting")["text"],
        "Привет, {name}! <b>Рады видеть</b> 👋"
    );
    assert!(find("menu.title")["text"].is_null());
    assert_eq!(find("menu.btn.buy")["button"]["style"], "success");
    assert_eq!(find("menu.btn.balance")["button"]["hidden"], true);
    let content = h.services().bot_content();
    assert_eq!(
        content
            .content
            .text("menu.greeting", &[("name", Var::Text("Аня"))]),
        "Привет, Аня! <b>Рады видеть</b> 👋"
    );
    assert!(content.content.button("menu.btn.balance", &[]).is_none());

    let (status, error) = h
        .write(
            Method::PUT,
            "/api/admin/bot/content",
            &owner,
            json!({ "texts": { "menu.subscription": "{tariff} за {price}" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(error["error"]["code"], "bot_content.invalid");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.contains("Строка подписки"), "{message}");
    assert!(message.contains("{price}"), "{message}");
    let (status, error) = h
        .write(
            Method::PUT,
            "/api/admin/bot/content",
            &owner,
            json!({ "texts": { "menu.title": "<b>не закрыт" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("не закрыт")
    );
    let (status, error) = h
        .write(
            Method::PUT,
            "/api/admin/bot/content",
            &owner,
            json!({ "buttons": { "menu.btn.buy": { "hidden": true } } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    let (status, reset) = h
        .write(
            Method::PUT,
            "/api/admin/bot/content",
            &owner,
            json!({ "texts": { "menu.greeting": null }, "buttons": { "menu.btn.balance": null }, "menu_columns": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let entries = reset["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .find(|e| e["key"] == "menu.greeting")
            .unwrap()["text"]
            .is_null()
    );
    assert_eq!(reset["menu_columns"], 1);
    let mut conn = h.services().store().conn().await.unwrap();
    let actions = conn
        .audit()
        .recent_by_action("settings.bot_content", 10)
        .await
        .unwrap();
    assert_eq!(actions.len(), 2);
    drop(conn);
    h.db.cleanup().await;
}

#[tokio::test]
async fn картинки_бота_грузятся_проверяются_и_удаляются() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let png = logo_png(640, 360, 3);
    let (status, _, body) = send_bytes(
        &h,
        Method::POST,
        "/api/admin/bot/images/menu",
        &owner,
        "image/png",
        png.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let view: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(slot(&view, "menu")["mode"], "own");
    assert!(slot(&view, "menu")["version"].as_str().unwrap().len() == 16);
    assert!(slot(&view, "common")["version"].is_null());
    assert!(slot(&view, "common")["mode"].is_null());
    let (status, headers, bytes) = send_bytes(
        &h,
        Method::GET,
        "/api/admin/bot/images/menu/file",
        &owner,
        "application/json",
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(bytes, png);
    let content = h.services().bot_content();
    assert_eq!(
        content
            .image_for(sn_domain::bot_content::Section::Menu)
            .map(|(slot, _)| slot),
        Some(sn_domain::bot_content::ImageSlot::Menu)
    );

    for (content_type, bytes, expected) in [
        (
            "text/plain",
            png.clone(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "image/webp",
            png.clone(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "image/png",
            logo_png(60, 60, 1),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "image/png",
            logo_png(6000, 6000, 1),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "image/png",
            logo_png(4000, 150, 1),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "image/png",
            b"not an image".to_vec(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (status, _, body) = send_bytes(
            &h,
            Method::POST,
            "/api/admin/bot/images/common",
            &owner,
            content_type,
            bytes,
        )
        .await;
        assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
    }
    let mut big = logo_png(1920, 1080, 7);
    big.resize(3 * 1024 * 1024, 7);
    let (status, _, body) = send_bytes(
        &h,
        Method::POST,
        "/api/admin/bot/images/promo",
        &owner,
        "image/png",
        big,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let mut huge = logo_png(1920, 1080, 7);
    huge.resize(6 * 1024 * 1024, 7);
    let (status, _, _) = send_bytes(
        &h,
        Method::POST,
        "/api/admin/bot/images/promo",
        &owner,
        "image/png",
        huge,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _, _) = send_bytes(
        &h,
        Method::POST,
        "/api/admin/bot/images/nowhere",
        &owner,
        "image/png",
        png.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, view) = h
        .write(
            Method::PUT,
            "/api/admin/bot/images/buy/mode",
            &owner,
            json!({ "mode": "off" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(slot(&view, "buy")["mode"], "off");
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/bot/images/common/mode",
            &owner,
            json!({ "mode": "off" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _, body) = send_bytes(
        &h,
        Method::DELETE,
        "/api/admin/bot/images/menu",
        &owner,
        "application/json",
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let view: Value = serde_json::from_slice(&body).unwrap();
    assert!(slot(&view, "menu")["version"].is_null());
    assert_eq!(slot(&view, "menu")["mode"], "common");
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _, _) = send_bytes(
        &h,
        Method::POST,
        "/api/admin/bot/images/menu",
        &support,
        "image/png",
        png,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    h.db.cleanup().await;
}

#[tokio::test]
async fn документы_сохраняются_и_видны_на_сайте() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, view) = h.get("/api/admin/settings/documents", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["consent"], true);
    assert_eq!(view["items"], json!([]));
    let (status, saved) = h
        .write(
            Method::PUT,
            "/api/admin/settings/documents",
            &owner,
            json!({
                "consent": false,
                "items": [
                    { "title": "  Публичная   оферта ", "url": " https://example.com/offer " },
                    { "title": "Политика конфиденциальности", "url": "https://example.com/privacy" }
                ]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["consent"], false);
    assert_eq!(saved["items"][0]["title"], "Публичная оферта");
    assert_eq!(saved["items"][0]["url"], "https://example.com/offer");
    let (status, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(site["documents"].as_array().unwrap().len(), 2);
    assert_eq!(site["documents"][1]["title"], "Политика конфиденциальности");
    assert!(!h.services().bot_content().consent);

    let (status, error) = h
        .write(
            Method::PUT,
            "/api/admin/settings/documents",
            &owner,
            json!({ "consent": true, "items": [ { "title": "Оферта", "url": "http://example.com" } ] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    let many: Vec<Value> = (0..11)
        .map(|i| json!({ "title": format!("Документ {i}"), "url": format!("https://example.com/{i}") }))
        .collect();
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/documents",
            &owner,
            json!({ "consent": true, "items": many }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h.get("/api/admin/settings/documents", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    h.db.cleanup().await;
}

#[tokio::test]
async fn порядок_кнопок_бота_правится_из_админки() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, view) = h.get("/api/admin/bot/layout", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["row_max"], 3);
    assert_eq!(view["screens"][0]["key"], "menu");
    assert_eq!(view["screens"][0]["custom"], false);
    assert_eq!(view["screens"][0]["packed"], 1);
    assert_eq!(view["screens"][1]["key"], "subscription");
    let (status, _) = h.get("/api/admin/bot/layout", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/bot/layout",
            &support,
            json!({ "layouts": { "menu": [["menu.btn.support"]] } }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, saved) = h
        .write(
            Method::PUT,
            "/api/admin/bot/layout",
            &owner,
            json!({ "layouts": { "menu": [["menu.btn.support", "menu.btn.buy"], ["menu.btn.app"]] } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let menu = &saved["screens"][0];
    assert_eq!(menu["custom"], true);
    assert!(menu["packed"].is_null());
    assert_eq!(menu["rows"][0], json!(["menu.btn.support", "menu.btn.buy"]));
    assert_eq!(menu["rows"][1], json!(["menu.btn.app"]));

    let (status, error) = h
        .write(
            Method::PUT,
            "/api/admin/bot/layout",
            &owner,
            json!({ "layouts": { "subscription": [["sub.btn.extend", "sub.btn.all", "common.btn.menu", "sub.btn.devices"]] } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(error["error"]["code"], "bot_layout.invalid");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("не больше 3")
    );

    let (status, reset) = h
        .write(
            Method::PUT,
            "/api/admin/bot/layout",
            &owner,
            json!({ "layouts": { "menu": null } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["screens"][0]["custom"], false);
    assert_eq!(reset["screens"][0]["rows"][0], json!(["menu.btn.app"]));
    let (_, content) = h.get("/api/admin/bot/content", &owner).await;
    assert_eq!(content["menu_columns"], 1);
}
