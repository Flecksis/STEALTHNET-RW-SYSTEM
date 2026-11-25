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
