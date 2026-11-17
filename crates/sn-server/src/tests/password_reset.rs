use super::*;

#[tokio::test]
async fn пароль_клиента_меняется_в_кабинете_и_выдаётся_из_админки() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["email_auth"], "password");
    assert_eq!(site["email_reset"], false);
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register",
            None,
            Some(json!({ "email": "lena@example.com", "password": "пароль-лены-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/reset",
            None,
            Some(json!({ "email": "lena@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.reset_unavailable");
    let client = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(sn_domain::client::IdentityKind::Email, "lena@example.com")
            .await
            .unwrap()
            .unwrap()
            .id
    };
    let token = h.client_token(client).await;
    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["email"], "lena@example.com");
    assert_eq!(me["has_password"], true);
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/security/password",
            Some(&token),
            Some(json!({ "current": "чужой-пароль-1", "password": "новый-пароль-лены" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/security/password",
            Some(&token),
            Some(json!({ "current": "пароль-лены-1", "password": "новый-пароль-лены" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, issued) = h
        .write(
            Method::POST,
            &format!("/api/admin/clients/{}/password", client.0),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let password = issued["password"].as_str().unwrap().to_string();
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "lena@example.com", "password": password })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, _) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "lena@example.com", "password": "новый-пароль-лены" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    h.db.cleanup().await;
}
