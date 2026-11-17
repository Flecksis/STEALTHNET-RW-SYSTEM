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
