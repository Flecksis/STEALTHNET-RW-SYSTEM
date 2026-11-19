use super::*;

#[tokio::test]
async fn администраторы_через_http_только_у_владельца() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let admin = h.admin("admin@example.com", AdminRole::Admin).await;
    let (status, _) = h.get("/api/admin/admins", &admin).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h.get("/api/admin/admins", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 2);
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/admins",
            &owner,
            json!({ "email": "olga@example.com", "role": "support" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["admin"]["role"], "support");
    assert_eq!(created["admin"]["must_change_password"], true);
    let password = created["password"].as_str().unwrap().to_string();
    assert_eq!(password.len(), 16);
    let id = created["admin"]["id"].as_i64().unwrap();
    let (status, _, login) = h
        .send(
            Method::POST,
            "/api/admin/auth/login",
            None,
            Some(json!({ "email": "olga@example.com", "password": password })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let olga = login["token"].as_str().unwrap().to_string();
    let (status, me) = h.get("/api/admin/me", &olga).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["must_change_password"], true);
    let (status, body) = h.get("/api/admin/clients", &olga).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.password_change_required");
    let (status, changed) = h
        .write(
            Method::POST,
            "/api/admin/auth/password",
            &olga,
            json!({ "current": password, "new": "свой-пароль-ольги-1" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let olga = changed["token"].as_str().unwrap().to_string();
    let (status, body) = h.get("/api/admin/clients", &olga).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/admins/{id}/role"),
            &owner,
            json!({ "role": "read_only" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["role"], "read_only");
    let (status, _) = h.get("/api/admin/support/tickets", &olga).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, body) = h
        .send(
            Method::POST,
            &format!("/api/admin/admins/{id}/disable"),
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["disabled"], true);
    let (status, _) = h.get("/api/admin/me", &olga).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, me) = h.get("/api/admin/me", &owner).await;
    let own = me["admin"]["admin_id"].as_i64().unwrap();
    let (status, _, body) = h
        .send(
            Method::POST,
            &format!("/api/admin/admins/{own}/disable"),
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "admins.self");
    let (status, _, body) = h
        .send(
            Method::POST,
            &format!("/api/admin/admins/{id}/password"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    h.db.cleanup().await;
}
