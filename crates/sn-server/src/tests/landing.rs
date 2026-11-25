use super::*;

const ADMIN_PATH: &str = "/api/admin/settings/landing";
const PUBLIC_PATH: &str = "/api/public/landing";

async fn upload(h: &Http, token: &str, content_type: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/settings/landing/images")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn public(h: &Http, if_none_match: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(Method::GET).uri(PUBLIC_PATH);
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

fn kinds(blocks: &Value) -> Vec<&str> {
    blocks
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["kind"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn лендинг_правится_в_настройках_и_сразу_виден_на_сайте() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h.get(ADMIN_PATH, &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .write(Method::PUT, ADMIN_PATH, &support, json!({ "blocks": [] }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = upload(&h, &support, "image/png", logo_png(800, 600, 1)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, view) = h.get(ADMIN_PATH, &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(
        kinds(&view["blocks"]),
        [
            "hero", "features", "steps", "plans", "servers", "faq", "cta", "footer"
        ]
    );
    assert_eq!(view["blocks"][0], json!({ "kind": "hero" }));
    assert_eq!(view["blocks"][1], json!({ "kind": "features", "on": true }));
    assert_eq!(view["limits"]["title"], 120);
    assert_eq!(view["limits"]["answer"], 1500);
    assert_eq!(view["limits"]["faq"], 20);
    assert_eq!(view["image"]["max_bytes"], 1024 * 1024);
    assert_eq!(
        view["image"]["types"],
        json!(["image/png", "image/jpeg", "image/webp"])
    );

    let (status, headers, body) = public(&h, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    let first: Value = serde_json::from_slice(&body).unwrap();
    let first_version = first["version"].as_str().unwrap().to_string();
    assert_eq!(first_version.len(), 16);
    assert_eq!(
        headers[header::ETAG],
        format!("\"{first_version}\"").as_str()
    );
    assert_eq!(first["blocks"], view["blocks"]);
    let mut keys: Vec<&String> = first.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["blocks", "version"]);

    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [
                {
                    "kind": "hero",
                    "title": "Тихий VPN за минуту",
                    "note": "",
                    "buttons": [
                        { "label": "Тарифы", "to": "#plans" },
                        { "label": "Канал", "to": "t.me/quiet_vpn" }
                    ]
                },
                { "kind": "faq", "items": [
                    { "std": "devices" },
                    { "title": "Работает в поездках?", "text": "Да.\r\nВ любой стране." }
                ] },
                { "kind": "steps", "on": false },
                { "kind": "footer", "text": "© {year} Тихий VPN" }
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        kinds(&saved["blocks"]),
        [
            "hero", "features", "faq", "cta", "steps", "plans", "servers", "footer"
        ]
    );
    assert_eq!(
        saved["blocks"][0]["buttons"][1]["to"],
        "https://t.me/quiet_vpn"
    );
    assert_eq!(saved["blocks"][0]["note"], "");
    assert_eq!(
        saved["blocks"][2]["items"][1]["text"],
        "Да.\nВ любой стране."
    );
    assert_eq!(saved["blocks"][4], json!({ "kind": "steps", "on": false }));
    assert_eq!(saved["blocks"][7]["text"], "© {year} Тихий VPN");
    assert_eq!(saved["limits"]["title"], 120);

    let (status, headers, body) = public(&h, None).await;
    assert_eq!(status, StatusCode::OK);
    let shown: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(shown["blocks"], saved["blocks"]);
    assert!(shown.get("limits").is_none());
    let version = shown["version"].as_str().unwrap().to_string();
    assert_ne!(version, first_version);
    let etag = headers[header::ETAG].to_str().unwrap().to_string();
    assert_eq!(etag, format!("\"{version}\""));

    let (status, headers, body) = public(&h, Some(&etag)).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
    assert_eq!(headers[header::ETAG], etag.as_str());
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    let (status, _, _) = public(&h, Some(&format!("W/{etag}"))).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    let (status, _, _) = public(&h, Some(&format!("\"{first_version}\", {etag}"))).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    let (status, _, body) = public(&h, Some(&format!("\"{first_version}\""))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.is_empty());

    let (status, error) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [{ "kind": "hero", "title": "Ж".repeat(121) }] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(error["error"]["code"], "landing.invalid");
    assert_eq!(
        error["error"]["message"],
        "Первый экран, заголовок: не длиннее 120 символов"
    );
    let (status, error) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [{
                "kind": "cta",
                "buttons": [{ "label": "Открыть", "to": "javascript:alert(document.cookie)" }]
            }] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Призыв в конце, кнопка 1, ссылка:")
    );
    let (status, error) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [{ "kind": "video" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(
        error["error"]["message"],
        "Блок 1: неизвестный блок «video»"
    );
    let (_, _, body) = public(&h, None).await;
    let still: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(still["version"], version.as_str());

    let (status, reset) = h
        .write(Method::PUT, ADMIN_PATH, &owner, json!({ "blocks": [] }))
        .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["blocks"], view["blocks"]);
    let (_, headers, _) = public(&h, None).await;
    assert_eq!(
        headers[header::ETAG],
        format!("\"{first_version}\"").as_str()
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn картинки_лендинга_загружаются_и_отдаются_с_вечным_кешем() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, body) = upload(&h, &owner, "text/plain", logo_png(800, 600, 1)).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{body}");
    assert_eq!(body["error"]["code"], "landing_image.format");
    let (status, body) = upload(&h, &owner, "image/svg+xml", b"<svg/>".to_vec()).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{body}");
    let mut heavy = logo_png(800, 600, 1);
    heavy.resize(2 * 1024 * 1024, 0);
    let (status, body) = upload(&h, &owner, "image/png", heavy).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "landing_image.too_heavy");
    let (status, body) = upload(&h, &owner, "image/png", logo_png(50, 50, 1)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "landing_image.too_small");
    let (status, body) = upload(
        &h,
        &owner,
        "image/jpeg",
        b"<html><script>alert(1)</script></html>".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "landing_image.format");

    let (status, image) = upload(&h, &owner, "image/png", logo_png(800, 600, 1)).await;
    assert_eq!(status, StatusCode::OK, "{image}");
    let tag = image["tag"].as_str().unwrap().to_string();
    assert_eq!(tag.len(), 16);
    assert_eq!(image["kind"], "png");
    assert_eq!(image["width"], 800);
    assert_eq!(image["height"], 600);
    assert_eq!(image["size"], logo_png(800, 600, 1).len());
    let url = format!("/api/public/landing/{tag}.png");
    assert_eq!(image["url"], url.as_str());

    let (status, headers, bytes) = raw(&h.state, &url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        headers[header::CONTENT_SECURITY_POLICY],
        "default-src 'none'; style-src 'unsafe-inline'"
    );
    assert_eq!(bytes, logo_png(800, 600, 1));
    for missing in [
        format!("/api/public/landing/{tag}.webp"),
        format!("/api/public/landing/{tag}"),
        "/api/public/landing/ffffffffffffffff.png".to_string(),
        "/api/public/landing/..%2F..%2Fetc.png".to_string(),
    ] {
        let (status, _, _) = raw(&h.state, &missing).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }

    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [
                { "kind": "hero", "art": "image", "image": { "tag": tag } },
                { "kind": "features", "items": [
                    { "std": "cabinet" },
                    {
                        "title": "Своё приложение",
                        "text": "Ставится за минуту",
                        "link": { "label": "Скачать", "to": "https://example.com/app" },
                        "image": { "tag": tag }
                    }
                ] }
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["blocks"][0]["image"],
        json!({ "tag": tag, "kind": "png", "width": 800, "height": 600, "url": url })
    );
    assert_eq!(saved["blocks"][0]["art"], "image");
    assert_eq!(saved["blocks"][1]["items"][1]["image"]["url"], url.as_str());
    let (_, _, body) = public(&h, None).await;
    let shown: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(shown["blocks"][0]["image"]["url"], url.as_str());
    assert_eq!(
        shown["blocks"][1]["items"][1]["link"]["to"],
        "https://example.com/app"
    );

    let (status, error) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [
                { "kind": "hero", "art": "image", "image": { "tag": "ffffffffffffffff" } }
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(
        error["error"]["message"],
        "Первый экран, картинка: не найдена, загрузите её заново"
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn превью_лендинга_встраивается_только_в_свой_сайт() {
    let (state, dir) = web_state().await;
    let (status, headers, body) = fetch(&state, Method::GET, "/admin/landing-preview").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("STEALTHNET"));
    assert_eq!(headers[header::X_FRAME_OPTIONS], "SAMEORIGIN");
    assert!(body.contains("noindex"));
    let (_, headers, _) = fetch(&state, Method::GET, "/admin/settings/landing").await;
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    let (_, headers, _) = fetch(&state, Method::GET, "/").await;
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn свой_css_лендинга_отдаётся_отдельным_файлом_с_версией() {
    let Some(mut h) = http().await else { return };
    let owner = h.owner().await;
    let dir = std::env::temp_dir().join(format!("sn-css-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html lang=\"ru\"><head><title>S</title></head><body></body></html>",
    )
    .unwrap();
    h.state.config = Arc::new(ServerConfig {
        web_dir: Some(dir.clone()),
        csp: crate::security::CspMode::All,
        ..ServerConfig::default()
    });

    let (_, _, plain) = fetch_file(&h, "/", None).await;
    assert!(!plain.contains("preload"));
    let (status, saved) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [], "css": {
                "page": ".ld { --accent: teal }",
                "hero": "h1 { font-size: 90px }"
            } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["css"],
        json!({ "page": ".ld { --accent: teal }", "hero": "h1 { font-size: 90px }" })
    );
    assert_eq!(
        saved["styles"]["hero"],
        "[data-block=\"hero\"] h1 { font-size: 90px }\n"
    );
    assert_eq!(saved["limits"]["page_css"], 50_000);
    assert_eq!(saved["limits"]["block_css"], 10_000);

    let (_, headers, body) = public(&h, None).await;
    let shown: Value = serde_json::from_slice(&body).unwrap();
    let path = shown["css"].as_str().unwrap().to_string();
    assert!(path.starts_with("/api/public/landing/"), "{path}");
    assert!(shown.get("styles").is_none());
    let json_etag = headers[header::ETAG].to_str().unwrap().to_string();

    let (status, headers, text) = fetch_file(&h, &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        text,
        ".ld { --accent: teal }\n[data-block=\"hero\"] h1 { font-size: 90px }\n"
    );
    assert_eq!(headers[header::CONTENT_TYPE], "text/css; charset=utf-8");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        headers[header::CONTENT_SECURITY_POLICY],
        "default-src 'none'; frame-ancestors 'none'; sandbox"
    );
    let etag = headers[header::ETAG].to_str().unwrap().to_string();
    assert!(path.contains(etag.trim_matches('"')));
    let (status, headers, text) = fetch_file(&h, &path, Some(&etag)).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(text.is_empty());
    assert_eq!(headers[header::ETAG], etag.as_str());

    let (_, headers, html) = fetch_file(&h, "/", None).await;
    assert!(
        html.contains(&format!(
            "<link rel=\"preload\" as=\"style\" href=\"{path}\">"
        )),
        "{html}"
    );
    assert!(!html.contains("teal"));
    assert!(!html.contains("<style"));
    let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(policy.contains("style-src 'self';"), "{policy}");
    assert!(!policy.contains("unsafe-inline"), "{policy}");
    for other in [
        "/cabinet",
        "/app",
        "/admin",
        "/admin/settings/landing",
        "/pay/1",
    ] {
        let (_, _, html) = fetch_file(&h, other, None).await;
        assert!(!html.contains(&path), "{other}");
    }

    let (status, _) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [], "css": { "page": ".ld { --accent: navy }" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, headers, body) = public(&h, Some(&json_etag)).await;
    assert_ne!(headers[header::ETAG], json_etag.as_str());
    let shown: Value = serde_json::from_slice(&body).unwrap();
    let newer = shown["css"].as_str().unwrap().to_string();
    assert_ne!(newer, path);
    let (status, headers, text) = fetch_file(&h, &path, Some(&etag)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(text, ".ld { --accent: navy }\n");
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    for missing in [
        "/api/public/landing/style.css",
        "/api/public/landing/0123456789ABCDEF.css",
        "/api/public/landing/..%2F..%2Fetc%2Fpasswd.css",
        "/api/public/landing/.css",
    ] {
        let (status, _, _) = fetch_file(&h, missing, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }

    let landing_off = sn_store::ModuleStateRow {
        module: "landing".into(),
        enabled: false,
        config: json!({}),
        updated_at: Utc::now(),
    };
    h.services().modules().apply_rows(&[landing_off]);
    let (status, _, text) = fetch_file(&h, &newer, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(text.contains("module.disabled"));
    h.services().modules().apply_rows(&[]);

    let (status, reset) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [], "css": {} }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["css"], json!({}));
    let (status, _, _) = fetch_file(&h, &newer, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, _, html) = fetch_file(&h, "/", None).await;
    assert!(!html.contains("preload"));
    let (_, _, body) = public(&h, None).await;
    let shown: Value = serde_json::from_slice(&body).unwrap();
    assert!(shown.get("css").is_none());
    std::fs::remove_dir_all(dir).ok();
    h.db.cleanup().await;
}

#[tokio::test]
async fn опасный_и_кривой_css_не_сохраняется_а_проверка_говорит_где_ошибка() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let check_path = "/api/admin/settings/landing/css";
    let (status, _) = h
        .write(
            Method::POST,
            check_path,
            &support,
            json!({ "css": { "page": ".a {}" } }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &support,
            json!({ "blocks": [], "css": { "page": ".a {}" } }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    for (css, message) in [
        (
            ".a {\n  color: red;",
            "Общий CSS, строка 1: не закрыта скобка {",
        ),
        (
            ".a {}\n@import url(https://evil.example/x.css);",
            "Общий CSS, строка 2: @import запрещён: скопируйте правила из того файла сюда",
        ),
        (
            ".a { background: url('javascript:alert(1)') }",
            "Общий CSS, строка 1: ссылки javascript: и vbscript: в CSS запрещены",
        ),
        (
            ".a { width: expression(1) }",
            "Общий CSS, строка 1: expression() запрещён",
        ),
        (
            ".a { behavior: url(#x) }",
            "Общий CSS, строка 1: свойство behavior запрещено",
        ),
        (
            ".a { -moz-binding: none }",
            "Общий CSS, строка 1: свойство -moz-binding запрещено",
        ),
        (
            "</style><script>alert(1)</script>",
            "Общий CSS, строка 1: уберите теги вроде <style>: нужен только CSS",
        ),
        (
            ".a { background: url(https://evil.example/pixel.gif) }",
            "Общий CSS, строка 1: адрес «https://evil.example/pixel.gif» нельзя: картинки и шрифты — только со своего домена. Загрузите картинку кнопкой под полем",
        ),
    ] {
        let (status, error) = h
            .write(
                Method::PUT,
                ADMIN_PATH,
                &owner,
                json!({ "blocks": [], "css": { "page": css } }),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{css}: {error}");
        assert_eq!(error["error"]["code"], "landing.invalid");
        assert_eq!(error["error"]["message"], message);
    }
    let (status, error) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [], "css": { "page": "я".repeat(50_001) } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(
        error["error"]["message"],
        "Общий CSS, не длиннее 50000 символов"
    );
    let (_, view) = h.get(ADMIN_PATH, &owner).await;
    assert_eq!(view["css"], json!({}));
    assert_eq!(view["styles"], json!({}));

    let (status, report) = h
        .write(
            Method::POST,
            check_path,
            &owner,
            json!({ "css": {
                "page": ".a {\n}\n.b {",
                "hero": "h1 { color: red }",
                "cta": null
            } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        report,
        json!({
            "page": { "line": 3, "problem": "не закрыта скобка {" },
            "hero": { "style": "[data-block=\"hero\"] h1 { color: red }\n" },
            "cta": { "style": "" }
        })
    );
    let (_, view) = h.get(ADMIN_PATH, &owner).await;
    assert_eq!(view["css"], json!({}));

    let (status, _) = h
        .write(
            Method::PUT,
            ADMIN_PATH,
            &owner,
            json!({ "blocks": [], "css": { "hero": "h1 { color: secretcolor }" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, journal) = h.get("/api/admin/audit?limit=20", &owner).await;
    assert_eq!(status, StatusCode::OK, "{journal}");
    let text = journal.to_string();
    assert!(text.contains("Изменён лендинг"), "{text}");
    assert!(text.contains("Свой CSS"), "{text}");
    assert!(text.contains("первый экран"), "{text}");
    assert!(!text.contains("secretcolor"), "{text}");
    h.db.cleanup().await;
}
