use super::*;

#[tokio::test]
async fn ключ_api_открывает_только_выданные_права() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h.get("/api/admin/api-keys", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/api-keys",
            &owner,
            json!({ "name": "Выгрузка клиентов", "permissions": ["clients_read"], "expires_in_days": 90 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let token = created["token"].as_str().unwrap().to_string();
    let id = created["key"]["id"].as_i64().unwrap();
    assert!(token.starts_with("snk_"));
    assert_eq!(created["key"]["permissions"], json!(["clients_read"]));
    let (status, body) = h.get("/api/admin/clients?limit=5", &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = h.get("/api/admin/payments", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h.get("/api/admin/api-keys", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h.get("/api/admin/auth/security", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h.get("/api/admin/admins", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, list) = h.get("/api/admin/api-keys", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list[0]["name"], "Выгрузка клиентов");
    assert!(list[0]["last_used_at"].is_string());
    assert!(list[0].get("token").is_none());
    let (status, _, body) = h
        .send(
            Method::POST,
            &format!("/api/admin/api-keys/{id}/revoke"),
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revoked"], true);
    let (status, _) = h.get("/api/admin/clients?limit=5", &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    h.db.cleanup().await;
}
