use super::*;

fn manifest() -> String {
    json!({
        "format": 1,
        "id": "hello",
        "name": "Привет",
        "version": "1.0.0",
        "author": "Студия Пример",
        "webhook": "https://hello.example.com/events",
        "events": ["client_registered"],
        "permissions": ["clients_read"],
        "page": { "url": "https://hello.example.com/admin" },
        "settings": [{ "key": "greeting", "type": "text", "label": "Текст", "default": "Привет" }]
    })
    .to_string()
}

#[tokio::test]
async fn внешний_модуль_ставит_только_админ_в_своей_сессии() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules/preview",
            &support,
            json!({ "manifest": manifest() }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, preview) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules/preview",
            &owner,
            json!({ "manifest": "{\"format\": 1, \"id\": \"Привет\"}" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(preview["preview"].is_null());
    assert!(preview["errors"].as_array().unwrap().len() > 1);
    let (status, preview) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules/preview",
            &owner,
            json!({ "manifest": manifest() }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(
        preview["preview"]["permissions"][0]["title"],
        "Клиенты: просмотр"
    );
    let (status, installed) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules",
            &owner,
            json!({ "manifest": manifest(), "permissions": ["clients_read"], "events": ["client_registered"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{installed}");
    let token = installed["credentials"]["api_key"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        installed["credentials"]["signing_secret"]
            .as_str()
            .unwrap()
            .starts_with("whsec_")
    );
    let (status, me) = h.get("/api/admin/ext-modules/me", &token).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["id"], "hello");
    assert_eq!(me["settings"]["greeting"], "Привет");
    let (status, body) = h.get("/api/admin/clients?limit=5", &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules",
            &token,
            json!({ "manifest": manifest(), "permissions": ["clients_read"], "events": ["client_registered"] }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .get("/api/admin/ext-modules/hello/deliveries", &token)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, list) = h.get("/api/admin/ext-modules", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list[0]["id"], "hello");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/admin/ext-modules/hello/page-token",
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["url"].as_str().unwrap().contains("sn_token="));
    let (status, _, body) = h
        .send(
            Method::DELETE,
            "/api/admin/ext-modules/hello",
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = h.get("/api/admin/clients?limit=5", &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    h.db.cleanup().await;
}

fn manifest_with_page(permission: &str) -> String {
    json!({
        "format": 1,
        "id": "hello",
        "name": "Привет",
        "version": "1.0.0",
        "author": "Студия Пример",
        "page": { "url": "https://hello.example.com/admin", "title": "Обращения модуля", "permission": permission }
    })
    .to_string()
}

#[tokio::test]
async fn страница_модуля_в_меню_у_тех_кому_она_открыта() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let reader = h.admin("reader@example.com", AdminRole::ReadOnly).await;
    let (status, installed) = h
        .write(
            Method::POST,
            "/api/admin/ext-modules",
            &owner,
            json!({ "manifest": manifest_with_page("support") }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{installed}");
    let page = json!([{ "id": "hello", "title": "Обращения модуля" }]);
    let (_, me) = h.get("/api/admin/me", &owner).await;
    assert_eq!(me["pages"], page);
    let (_, me) = h.get("/api/admin/me", &support).await;
    assert_eq!(me["pages"], page);
    let (_, me) = h.get("/api/admin/me", &reader).await;
    assert_eq!(me["pages"], json!([]));
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/admin/ext-modules/hello/page-token",
            Some(&support),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["title"], "Обращения модуля");
    assert_eq!(body["module"], "Привет");
    let (status, _) = h.get("/api/admin/ext-modules/hello", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = h
        .send(
            Method::POST,
            "/api/admin/ext-modules/hello/page-token",
            Some(&reader),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h
        .write(
            Method::PATCH,
            "/api/admin/ext-modules/hello",
            &owner,
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, me) = h.get("/api/admin/me", &support).await;
    assert_eq!(me["pages"], json!([]));
    let (_, me) = h.get("/api/admin/me", &owner).await;
    assert_eq!(me["pages"], json!([]));
    h.db.cleanup().await;
}
