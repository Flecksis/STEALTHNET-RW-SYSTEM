use super::*;

const LIST: &str = "/api/admin/settings/documents";

const DIRTY: &str = "<p>Здесь описано, какие данные получает сервис.</p>\
<script>alert('xss')</script><img src=x onerror=\"alert(1)\">\
<h2 onclick=\"alert(2)\">1. Данные</h2>\
<p style=\"background:url(https://evil.example/t.png)\">Только нужные.</p>\
<iframe src=\"https://evil.example\"></iframe>\
<p><a href=\"javascript:alert(3)\">не ссылка</a> и <a href=\"https://t.me/support\" onmouseover=\"x()\">поддержка</a></p>";

fn web_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sn-docs-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::create_dir_all(dir.join("promo")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html>\n<html lang=\"ru\">\n<head>\n<meta name=\"robots\" content=\"noindex\" />\n<title>STEALTHNET</title>\n<script type=\"module\" src=\"/assets/index-1a2b.js\"></script>\n</head>\n<body>\n<div id=\"root\"></div>\n</body>\n</html>",
    )
    .unwrap();
    dir
}

async fn with_web() -> Option<(Http, std::path::PathBuf)> {
    let mut h = http().await?;
    let dir = web_dir();
    h.state.config = Arc::new(ServerConfig {
        web_dir: Some(dir.clone()),
        ..ServerConfig::default()
    });
    Some((h, dir))
}

async fn page_with(
    h: &Http,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(Method::GET).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router(h.state.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn assert_clean(html: &str) {
    for bad in [
        "<script",
        "alert",
        "onerror",
        "onclick",
        "onmouseover",
        "style=",
        "url(",
        "<iframe",
        "javascript:",
        "evil.example",
        "<img",
    ] {
        assert!(!html.contains(bad), "{bad} в {html}");
    }
}

#[tokio::test]
async fn страница_документа_чистится_и_отдаётся_сайтом_в_оформлении_лендинга() {
    let Some((h, dir)) = with_web().await else {
        return;
    };
    let owner = h.owner().await;
    let (status, doc) = h
        .write(
            Method::POST,
            LIST,
            &owner,
            json!({
                "kind": "page",
                "title": "  Политика   конфиденциальности ",
                "title_en": "Privacy policy",
                "path": "/Privacy-Policy/",
                "body_ru": DIRTY,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(doc["kind"], "page");
    assert_eq!(doc["title"], "Политика конфиденциальности");
    assert_eq!(doc["path"], "/privacy-policy");
    assert!(doc["url"].is_null());
    let body = doc["body_ru"].as_str().unwrap();
    assert_clean(body);
    assert!(body.contains("<h2>1. Данные</h2>"), "{body}");
    assert!(
        body.contains("<a href=\"https://t.me/support\" rel=\"noopener noreferrer\">поддержка</a>"),
        "{body}"
    );
    assert!(doc["body_en"].is_null());
    let id = doc["id"].as_str().unwrap().to_string();

    let (status, headers, html) = page_with(&h, "/privacy-policy", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("<title>Политика конфиденциальности — STEALTHNET</title>"),
        "{html}"
    );
    assert!(html.contains(
        "<meta name=\"description\" content=\"Здесь описано, какие данные получает сервис."
    ));
    assert!(html.contains("<meta property=\"og:type\" content=\"article\">"));
    assert!(
        html.contains(
            "<noscript><article class=\"dp-noscript\"><h1>Политика конфиденциальности</h1>"
        )
    );
    assert!(html.contains("<html lang=\"ru\">"));
    assert_eq!(html.matches("<title>").count(), 1);
    assert_eq!(html.matches("<script").count(), 1);
    assert!(html.contains("<script type=\"module\" src=\"/assets/index-1a2b.js\"></script>"));
    assert_clean(&html.replace(
        "<script type=\"module\" src=\"/assets/index-1a2b.js\"></script>",
        "",
    ));
    assert!(html.find("<noscript>").unwrap() < html.find("<div id=\"root\">").unwrap());
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    let policy = headers
        .get(header::CONTENT_SECURITY_POLICY_REPORT_ONLY)
        .or_else(|| headers.get(header::CONTENT_SECURITY_POLICY))
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(policy.contains("script-src 'self'"), "{policy}");
    assert!(policy.contains("style-src 'self'"), "{policy}");
    assert!(!policy.contains("unsafe-inline"), "{policy}");

    let (status, _, page) = h
        .send(Method::GET, "/api/public/pages/privacy-policy", None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["title"], "Политика конфиденциальности");
    assert_eq!(page["lang"], "ru");
    assert_eq!(page["english"], false);
    assert_clean(page["html"].as_str().unwrap());

    let (status, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        site["documents"],
        json!([{ "title": "Политика конфиденциальности", "title_en": "Privacy policy", "url": "/privacy-policy", "page": true }])
    );

    let (status, headers, _) = page_with(&h, "/privacy-policy/?utm_source=bot", &[]).await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(headers[header::LOCATION], "/privacy-policy?utm_source=bot");

    let landing_off = sn_store::ModuleStateRow {
        module: "landing".into(),
        enabled: false,
        config: json!({}),
        updated_at: Utc::now(),
    };
    h.services().modules().apply_rows(&[landing_off]);
    let (status, _, html) = page_with(&h, "/privacy-policy", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(
            "<noscript><article class=\"dp-noscript\"><h1>Политика конфиденциальности</h1>"
        )
    );
    let (status, headers, _) = page_with(&h, "/", &[]).await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(headers[header::LOCATION], "/cabinet");

    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/languages",
            &owner,
            json!({ "english": true, "default": "ru" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, html) = page_with(&h, "/privacy-policy", &[("cookie", "sn_lang=en")]).await;
    assert!(
        html.contains("<html lang=\"ru\">"),
        "без английского текста страница русская"
    );
    let (status, saved) = h
        .write(
            Method::PUT,
            &format!("{LIST}/{id}"),
            &owner,
            json!({
                "kind": "page",
                "title": "Политика конфиденциальности",
                "title_en": "Privacy policy",
                "path": "privacy-policy",
                "body_ru": body,
                "body_en": "<p>What data the service gets.</p><script>alert(1)</script>",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["body_en"], "<p>What data the service gets.</p>");
    let (_, _, html) = page_with(&h, "/privacy-policy", &[("cookie", "sn_lang=en")]).await;
    assert!(html.contains("<html lang=\"en\">"), "{html}");
    assert!(
        html.contains("<title>Privacy policy — STEALTHNET</title>"),
        "{html}"
    );
    assert!(html.contains("<p>Updated "), "{html}");
    let (_, _, page) = h
        .send(Method::GET, "/api/public/pages/privacy-policy", None, None)
        .await;
    assert_eq!(page["lang"], "ru");
    let mut request = Request::builder()
        .method(Method::GET)
        .uri("/api/public/pages/privacy-policy")
        .header("x-sn-lang", "en");
    request = request.header(header::ACCEPT, "application/json");
    let response = router(h.state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let page = body_json(response).await;
    assert_eq!(page["lang"], "en");
    assert_eq!(page["title"], "Privacy policy");
    assert_eq!(page["english"], true);

    let (status, _, _) = h
        .send(Method::DELETE, &format!("{LIST}/{id}"), Some(&owner), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, html) = page_with(&h, "/privacy-policy", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains("dp-noscript"));
    let (status, _, _) = h
        .send(Method::GET, "/api/public/pages/privacy-policy", None, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mut conn = h.services().store().conn().await.unwrap();
    assert!(conn.document_pages().get(&id).await.unwrap().is_none());
    drop(conn);
    std::fs::remove_dir_all(dir).unwrap();
    h.db.cleanup().await;
}

#[tokio::test]
async fn адрес_страницы_не_пересекается_с_путями_панели() {
    let Some((h, dir)) = with_web().await else {
        return;
    };
    let owner = h.owner().await;
    let create = |path: &'static str, body: &'static str| json!({ "kind": "page", "title": "Документ", "path": path, "body_ru": body });
    for (path, expected) in [
        ("admin", "/admin занят панелью"),
        ("/cabinet/terms", "/cabinet занят панелью"),
        ("app", "/app занят панелью"),
        ("api/public", "/api занят панелью"),
        ("m", "/m занят панелью"),
        ("pay", "/pay занят панелью"),
        ("verify", "/verify занят панелью"),
        ("assets", "/assets занят панелью"),
        ("sw.js", "/sw.js занят панелью"),
        ("promo", "/promo занят файлами сайта"),
        ("политика", "только латинские буквы"),
        ("", "Укажите адрес страницы"),
        ("a/b/c/d", "не больше 3 частей"),
    ] {
        let (status, error) = h
            .write(Method::POST, LIST, &owner, create(path, "<p>Текст</p>"))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {error}");
        assert_eq!(error["error"]["code"], "bot_documents.invalid", "{path}");
        let message = error["error"]["message"].as_str().unwrap();
        assert!(message.contains(expected), "{path}: {message}");
    }
    let (status, error) = h
        .write(
            Method::POST,
            LIST,
            &owner,
            create("empty", "<script>alert(1)</script><p> </p>"),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Добавьте текст")
    );
    let huge = format!("<p>{}</p>", "а".repeat(sn_domain::documents::BODY_MAX));
    let (status, error) = h
        .write(
            Method::POST,
            LIST,
            &owner,
            json!({ "kind": "page", "title": "Большой", "path": "big", "body_ru": huge }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    let (status, first) = h
        .write(Method::POST, LIST, &owner, create("offer", "<p>Оферта</p>"))
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (status, error) = h
        .write(Method::POST, LIST, &owner, create("/Offer/", "<p>Ещё</p>"))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("/offer уже у документа")
    );
    let (status, error) = h
        .write(
            Method::PUT,
            LIST,
            &owner,
            json!({ "consent": true, "items": [
                { "id": first["id"], "title": "Оферта" },
                { "title": "Новая", "path": "new-page" }
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("сначала создать")
    );
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h
        .write(Method::POST, LIST, &support, create("rules", "<p>x</p>"))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .get(
            &format!("{LIST}/{}", first["id"].as_str().unwrap()),
            &support,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    std::fs::remove_dir_all(dir).unwrap();
    h.db.cleanup().await;
}

#[tokio::test]
async fn список_меняет_порядок_согласие_и_удаляет_тексты_убранных_страниц() {
    let Some((h, dir)) = with_web().await else {
        return;
    };
    let owner = h.owner().await;
    let (_, offer) = h
        .write(
            Method::POST,
            LIST,
            &owner,
            json!({ "kind": "page", "title": "Оферта", "path": "offer", "body_ru": "<p>Оферта</p>" }),
        )
        .await;
    let (_, rules) = h
        .write(
            Method::POST,
            LIST,
            &owner,
            json!({ "kind": "link", "title": "Правила", "title_en": "Rules", "url": "https://example.com/rules" }),
        )
        .await;
    let (status, view) = h.get(LIST, &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["items"][0]["kind"], "page");
    assert_eq!(view["items"][0]["english"], false);
    assert!(view["items"][0]["updated_at"].is_string());
    assert_eq!(view["items"][1]["kind"], "link");
    assert!(view["items"][1]["updated_at"].is_null());
    assert_eq!(view["max"], 10);
    let (status, view) = h
        .write(
            Method::PUT,
            LIST,
            &owner,
            json!({ "consent": false, "items": [
                { "id": rules["id"], "title": "Правила", "title_en": "Rules", "url": "https://example.com/rules" },
                { "id": offer["id"], "title": "Публичная оферта" }
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["consent"], false);
    assert_eq!(view["items"][0]["title"], "Правила");
    assert_eq!(view["items"][1]["title"], "Публичная оферта");
    assert_eq!(view["items"][1]["path"], "/offer");
    let (status, _, page) = h
        .send(Method::GET, "/api/public/pages/offer", None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["title"], "Публичная оферта");
    let (status, view) = h
        .write(
            Method::PUT,
            LIST,
            &owner,
            json!({ "consent": false, "items": [
                { "id": rules["id"], "title": "Правила", "url": "https://example.com/rules" }
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["items"].as_array().unwrap().len(), 1);
    let mut conn = h.services().store().conn().await.unwrap();
    assert!(
        conn.document_pages()
            .get(offer["id"].as_str().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT details ->> 'action' FROM audit_log WHERE action = 'settings.documents' ORDER BY id",
    )
    .fetch_all(sn_store::Db::conn(&mut conn))
    .await
    .unwrap();
    assert_eq!(actions, ["create", "create", "list", "list"]);
    drop(conn);
    let (status, _, page) = h
        .send(Method::GET, "/api/public/pages/offer", None, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{page}");
    std::fs::remove_dir_all(dir).unwrap();
    h.db.cleanup().await;
}

#[tokio::test]
async fn черновик_html_очищается_для_превью() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, view) = h
        .write(
            Method::POST,
            &format!("{LIST}/clean"),
            &owner,
            json!({ "html": DIRTY }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_clean(view["html"].as_str().unwrap());
    assert!(view["chars"].as_u64().unwrap() > 10);
    h.db.cleanup().await;
}
