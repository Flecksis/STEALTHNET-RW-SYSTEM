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

fn session_cookie(headers: &axum::http::HeaderMap) -> String {
    headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| cookie.strip_prefix("sn_client="))
        .and_then(|rest| rest.split(';').next())
        .expect("кука сессии кабинета")
        .to_string()
}

#[tokio::test]
async fn пароль_клиента_выдаёт_только_право_входа_за_клиента_и_он_одноразовый() {
    let Some(h) = http().await else { return };
    let support = h.admin("help@example.com", AdminRole::Support).await;
    let manager = h.admin("boss@example.com", AdminRole::Admin).await;
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register",
            None,
            Some(json!({ "email": "vera@example.com", "password": "пароль-веры-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let client = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(sn_domain::client::IdentityKind::Email, "vera@example.com")
            .await
            .unwrap()
            .unwrap()
            .id
    };
    for path in ["password", "totp/reset"] {
        let (status, body) = h
            .write(
                Method::POST,
                &format!("/api/admin/clients/{}/{path}", client.0),
                &support,
                json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
    }
    let (status, issued) = h
        .write(
            Method::POST,
            &format!("/api/admin/clients/{}/password", client.0),
            &manager,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let password = issued["password"].as_str().unwrap().to_string();
    let notified: i64 = {
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE payload->>'template' = 'password_issued' \
             AND (payload->>'client_id')::bigint = $1",
        )
        .bind(client.0)
        .fetch_one(conn.conn())
        .await
        .unwrap()
    };
    assert_eq!(notified, 1);
    let (status, headers, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "vera@example.com", "password": password })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = session_cookie(&headers);
    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["must_change_password"], true);
    let (status, blocked) = h.get("/api/client/subscriptions", &token).await;
    assert_ne!(status, StatusCode::OK, "{blocked}");
    assert_eq!(blocked["error"]["code"], "auth.password_change_required");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/security/password",
            Some(&token),
            Some(json!({ "password": password })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.same_password");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/security/password",
            Some(&token),
            Some(json!({ "password": "свой-пароль-веры" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, list) = h.get("/api/client/subscriptions", &token).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let (_, me) = h.get("/api/client/me", &token).await;
    assert_eq!(me["must_change_password"], false);
    let (status, _, _) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "vera@example.com", "password": password })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, headers, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "vera@example.com", "password": "свой-пароль-веры" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, me) = h.get("/api/client/me", &session_cookie(&headers)).await;
    assert_eq!(me["must_change_password"], false);
    h.db.cleanup().await;
}
