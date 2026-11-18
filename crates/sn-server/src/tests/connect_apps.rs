use super::*;

const ADMIN_PATH: &str = "/api/admin/settings/connect-apps";
const PUBLIC_PATH: &str = "/api/public/connect-apps";
const ICON_SVG: &str = r##"<svg width="50" height="50" viewBox="0 0 50 50" fill="none"><path d="M5 5h40v40H5z" fill="#FAFAFA"/></svg>"##;

async fn upload(h: &Http, token: &str, content_type: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/settings/connect-apps/icons")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn raw(
    h: &Http,
    path: &str,
    if_none_match: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(Method::GET).uri(path);
    if let Some(etag) = if_none_match {
        builder = builder.header(header::IF_NONE_MATCH, etag);
    }
    let response = router(h.state.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, headers, bytes.to_vec())
}

fn text(ru: &str, en: &str) -> Value {
    json!({ "ru": ru, "en": en })
}

fn app(name: &str, store: &str, link: &str) -> Value {
    json!({
        "name": name,
        "steps": [
            { "kind": "install", "text": text("Бесплатно в App Store", "Free on the App Store"),
              "buttons": [{ "label": text("App Store", "App Store"), "url": store }] },
            { "kind": "add", "text": text("Одна кнопка", "One click"), "link": link },
            { "kind": "connect", "text": text("Включите VPN", "Turn the VPN on") },
        ],
    })
}

fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|app| app["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn приложения_для_подключения_сохраняются_с_проверкой_ссылок() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h.get(ADMIN_PATH, &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .write(Method::PUT, ADMIN_PATH, &support, json!({ "apps": null }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, view) = h.get(ADMIN_PATH, &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["standard"], true);
    assert_eq!(names(&view["apps"]["ios"]), ["Happ"]);
    assert_eq!(names(&view["apps"]["linux"]), ["Hiddify"]);
    assert_eq!(view["apps"]["apple_tv"], json!([]));
    assert_eq!(view["defaults"], view["apps"]);
    assert_eq!(view["limits"]["apps"], 12);

    let (status, headers, body) = raw(&h, PUBLIC_PATH, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    let first: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        first["apps"]["ios"][0]["steps"][1]["link"],
        "happ://add/{url}"
    );
    assert!(first["apps"].get("apple_tv").is_none());
    assert_eq!(
        first["apps"]["windows"][0]["steps"][2]["text"]["en"],
        "Click connect in Hiddify, and the server is picked automatically"
    );
    let etag = headers[header::ETAG].to_str().unwrap().to_string();

    let forbidden = json!({ "apps": { "ios": [app("Happ", "https://apps.apple.com/app/x", "javascript:alert({url})")] } });
    let (status, body) = h.write(Method::PUT, ADMIN_PATH, &owner, forbidden).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["message"],
        "iPhone и iPad · Happ: ссылка добавления: такие ссылки запрещены, нужна схема приложения или https://"
    );
    let plain = json!({ "apps": { "android": [app("Happ", "http://play.example/happ", "happ://add/{url}")] } });
    let (status, body) = h.write(Method::PUT, ADMIN_PATH, &owner, plain).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["message"],
        "Android · Happ: кнопка «App Store»: нужна ссылка https://"
    );
    let no_slot =
        json!({ "apps": { "ios": [app("Happ", "https://apps.apple.com/app/x", "happ://add/")] } });
    let (status, body) = h.write(Method::PUT, ADMIN_PATH, &owner, no_slot).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let mut stash = app(
        "Stash",
        "https://apps.apple.com/app/stash",
        "stash://install-config?url={url}",
    );
    stash["enabled"] = json!(false);
    let list = json!({ "apps": {
        "ios": [
            app("Streisand", "https://apps.apple.com/app/streisand", "streisand://import/{url}"),
            stash,
            app("Happ", "https://apps.apple.com/app/happ", "happ://add/{url}"),
        ],
        "android_tv": [app("vpn4tv", "https://play.google.com/store/apps/details?id=tv", "hiddify://import/{url}")],
    } });
    let (status, saved) = h.write(Method::PUT, ADMIN_PATH, &owner, list).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["standard"], false);
    assert_eq!(names(&saved["apps"]["ios"]), ["Streisand", "Stash", "Happ"]);
    assert_eq!(saved["apps"]["ios"][1]["enabled"], false);
    assert_eq!(saved["apps"]["ios"][0]["id"], "streisand");
    assert_eq!(saved["apps"]["windows"], json!([]));

    let (status, headers, body) = raw(&h, PUBLIC_PATH, Some(&etag)).await;
    assert_eq!(status, StatusCode::OK);
    let public: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(names(&public["apps"]["ios"]), ["Streisand", "Happ"]);
    assert_eq!(names(&public["apps"]["android_tv"]), ["vpn4tv"]);
    assert!(public["apps"].get("windows").is_none());
    let fresh = headers[header::ETAG].to_str().unwrap().to_string();
    assert_ne!(fresh, etag);
    let (status, _, _) = raw(&h, PUBLIC_PATH, Some(&fresh)).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);

    let (status, back) = h
        .write(Method::PUT, ADMIN_PATH, &owner, json!({ "apps": null }))
        .await;
    assert_eq!(status, StatusCode::OK, "{back}");
    assert_eq!(back["standard"], true);
    let (_, _, body) = raw(&h, PUBLIC_PATH, None).await;
    let public: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(names(&public["apps"]["ios"]), ["Happ"]);
    let mut conn = h.services().store().conn().await.unwrap();
    let stored: Option<Value> = conn.settings().get(keys::CONNECT_APPS).await.unwrap();
    assert_eq!(stored, None);
    drop(conn);
    h.db.cleanup().await;
}

#[tokio::test]
async fn значки_приложений_загружаются_и_отдаются_без_скриптов() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, body) = upload(
        &h,
        &owner,
        "image/svg+xml",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["message"],
        "В SVG есть скрипты или внешние ссылки"
    );
    let (status, body) = upload(&h, &owner, "text/plain", b"hello".to_vec()).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{body}");
    let (status, body) = upload(&h, &owner, "image/png", logo_png(120, 60, 3)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["message"], "Значок должен быть квадратным");
    let (status, png) = upload(&h, &owner, "image/png", logo_png(64, 64, 3)).await;
    assert_eq!(status, StatusCode::OK, "{png}");
    assert_eq!(png["kind"], "png");

    let (status, svg) = upload(&h, &owner, "image/svg+xml", ICON_SVG.as_bytes().to_vec()).await;
    assert_eq!(status, StatusCode::OK, "{svg}");
    assert_eq!(svg["kind"], "svg");
    let url = svg["url"].as_str().unwrap().to_string();
    assert_eq!(
        url,
        format!("/api/public/app-icon/{}.svg", svg["tag"].as_str().unwrap())
    );
    let (status, headers, bytes) = raw(&h, &url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/svg+xml");
    assert!(
        headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .ends_with("sandbox")
    );
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\"")
    );
    let (status, _, _) = raw(&h, &url.replace(".svg", ".png"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = raw(&h, "/api/public/app-icon/zzzz.svg", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let mut lost = app("Happ", "https://apps.apple.com/app/x", "happ://add/{url}");
    lost["icon"] = json!({ "tag": "0123456789abcdef", "kind": "svg" });
    let (status, body) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "apps": { "ios": [lost] } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["message"],
        "iPhone и iPad · Happ: значок не найден, загрузите его заново"
    );
    let mut happ = app("Happ", "https://apps.apple.com/app/x", "happ://add/{url}");
    happ["icon"] = json!({ "tag": svg["tag"], "kind": "svg" });
    let (status, body) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "apps": { "ios": [happ] } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, _, body) = raw(&h, PUBLIC_PATH, None).await;
    let public: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(public["apps"]["ios"][0]["icon"], url);
    h.db.cleanup().await;
}

fn remna_config() -> Value {
    let localized = |ru: &str, en: &str| json!({ "ru": ru, "en": en, "fa": "متن" });
    let block = |title: &str, buttons: Value| {
        json!({
            "title": localized(title, title),
            "description": localized(&format!("{title}: что сделать"), "What to do"),
            "svgIconKey": "Gear", "svgIconColor": "cyan", "buttons": buttons,
        })
    };
    let external = |label: &str, link: &str| json!({ "type": "external", "link": link, "text": localized(label, label), "svgIconKey": "Gear" });
    let add = |link: &str| json!({ "type": "subscriptionLink", "link": link, "text": localized("Добавить", "Add"), "svgIconKey": "Gear" });
    json!({
        "version": "1",
        "locales": ["ru", "en", "fa"],
        "svgLibrary": { "Happ": ICON_SVG, "Gear": ICON_SVG },
        "platforms": {
            "ios": { "displayName": localized("iOS", "iOS"), "svgIconKey": "Gear", "apps": [
                { "name": "Streisand", "featured": false, "blocks": [
                    block("Установка", json!([external("App Store", "https://apps.apple.com/app/streisand")])),
                    block("Добавление", json!([add("streisand://import/{{SUBSCRIPTION_LINK}}")])),
                    block("Подключение", json!([])),
                ]},
                { "name": "Happ", "featured": true, "svgIconKey": "Happ", "blocks": [
                    block("Установка", json!([external("App Store", "https://apps.apple.com/app/happ")])),
                    block("Добавление", json!([add("happ://add/{{SUBSCRIPTION_LINK}}")])),
                    block("Если подписка не добавилась", json!([])),
                    block("Подключение", json!([])),
                ]},
            ]},
            "appleTV": { "displayName": localized("Apple TV", "Apple TV"), "svgIconKey": "Gear", "apps": [
                { "name": "Happ", "featured": true, "svgIconKey": "Happ", "blocks": [
                    block("Установка", json!([external("App Store", "https://apps.apple.com/app/happ-tv")])),
                    block("Добавление", json!([add("happ://add/{{SUBSCRIPTION_LINK}}")])),
                    block("Подключение", json!([])),
                ]},
            ]},
        },
    })
}

#[tokio::test]
async fn приложения_переносятся_из_оформления_remnawave_с_предпросмотром() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let page = Uuid::new_v4();
    h.remna
        .set_subpage_configs(vec![sn_remnawave::SubpageConfigRef {
            uuid: page,
            name: "Основное".into(),
            view_position: 0,
        }]);
    h.remna.set_subpage_content(page, remna_config());
    let path = "/api/admin/settings/connect-apps/import";
    let (status, _) = h
        .write(
            Method::POST,
            path,
            &support,
            json!({ "page_config": page.to_string() }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h
        .write(
            Method::POST,
            path,
            &owner,
            json!({ "page_config": Uuid::new_v4().to_string() }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "connect_apps.not_found");

    let (status, preview) = h
        .write(
            Method::POST,
            path,
            &owner,
            json!({ "page_config": page.to_string() }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["name"], "Основное");
    assert_eq!(preview["total"], 3);
    assert_eq!(
        preview["platforms"],
        json!([
            { "platform": "ios", "title": "iPhone и iPad", "apps": ["Happ", "Streisand"] },
            { "platform": "apple_tv", "title": "Apple TV", "apps": ["Happ"] },
        ])
    );
    let happ = &preview["apps"]["ios"][0];
    assert_eq!(happ["steps"][1]["link"], "happ://add/{url}");
    assert_eq!(happ["steps"][2]["kind"], "note");
    assert_eq!(
        happ["steps"][2]["title"]["ru"],
        "Если подписка не добавилась"
    );
    assert_eq!(preview["apps"]["ios"][1]["icon"], Value::Null);
    let icon = happ["icon"].clone();
    assert_eq!(icon["kind"], "svg");
    assert_eq!(preview["apps"]["apple_tv"][0]["icon"], icon);
    let notes = preview["notes"].to_string();
    assert!(
        notes.contains("Тексты переносятся на русском и английском"),
        "{notes}"
    );
    assert!(notes.contains("Дополнительных шагов: 1"), "{notes}");

    let (_, _, body) = raw(&h, PUBLIC_PATH, None).await;
    let public: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(names(&public["apps"]["ios"]), ["Happ"]);
    assert!(public["apps"].get("apple_tv").is_none());

    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "apps": preview["apps"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (_, _, body) = raw(&h, PUBLIC_PATH, None).await;
    let public: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(names(&public["apps"]["ios"]), ["Happ", "Streisand"]);
    assert_eq!(names(&public["apps"]["apple_tv"]), ["Happ"]);
    assert!(public["apps"].get("android").is_none());
    let icon_url = public["apps"]["ios"][0]["icon"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _, _) = raw(&h, &icon_url, None).await;
    assert_eq!(status, StatusCode::OK);

    h.services().set_remna(Some(h.remna.clone()));
    h.remna
        .set_subpage_content(page, json!({ "version": "1", "platforms": {} }));
    let (status, body) = h
        .write(
            Method::POST,
            path,
            &owner,
            json!({ "page_config": page.to_string() }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["message"],
        "В оформлении нет приложений: переносить нечего"
    );
    h.db.cleanup().await;
}
