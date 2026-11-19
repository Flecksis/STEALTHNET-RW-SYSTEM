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

async fn key_with(h: &Http, owner: &str, permissions: Value) -> String {
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/api-keys",
            owner,
            json!({ "name": "Интеграция", "permissions": permissions, "expires_in_days": 30 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    created["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn ключ_api_не_меняет_почту_клиента() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let token = key_with(&h, &owner, json!(["clients_read", "clients_login"])).await;
    let client = h.client(5_093_118_501).await;
    let path = format!("/api/admin/clients/{}/email", client.0);
    let (status, body) = h
        .write(
            Method::PUT,
            &path,
            &token,
            json!({ "email": "chuzhoi@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let identities = h
        .services()
        .store()
        .conn()
        .await
        .unwrap()
        .clients()
        .identities(client)
        .await
        .unwrap();
    assert!(
        identities
            .iter()
            .all(|i| i.kind != sn_domain::client::IdentityKind::Email),
        "почта не должна появиться"
    );
    let (status, body) = h
        .write(
            Method::PUT,
            &path,
            &owner,
            json!({ "email": "svoi@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn ключ_api_владельца_не_управляет_обновлениями() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let token = key_with(&h, &owner, json!(["settings"])).await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/system/settings",
            &token,
            json!({ "check_enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/admin/system/check",
            Some(&token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _, body) = h
        .send(
            Method::DELETE,
            "/api/admin/system/update",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/system/update",
            &token,
            json!({ "target": "0".repeat(40), "password": PASSWORD }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/system/settings",
            &owner,
            json!({ "check_enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn ключ_api_не_получает_секрет_вебхука_remnawave() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let token = key_with(&h, &owner, json!(["settings"])).await;
    let path = "/api/admin/settings/remnawave/webhook-env";
    let (status, body) = h.get(path, &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = h.get(path, &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.db.cleanup().await;
}
