use super::*;

async fn admin_id(h: &Http, owner: &str, email: &str) -> i64 {
    let (status, list) = h.get("/api/admin/admins", owner).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list.as_array()
        .unwrap()
        .iter()
        .find(|a| a["email"] == email)
        .unwrap()["id"]
        .as_i64()
        .unwrap()
}

fn permissions(me: &Value) -> Vec<String> {
    me["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn журнал_видит_владелец_и_администратор_с_правом() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let anna = h.admin("anna@example.com", AdminRole::Admin).await;
    let help = h.admin("help@example.com", AdminRole::Support).await;
    let (status, page) = h.get("/api/admin/audit", &owner).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page["items"].is_array());
    let (status, meta) = h.get("/api/admin/audit/meta", &owner).await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(meta["areas"].as_array().unwrap().len(), 12);
    assert_eq!(meta["search_min"], 3);
    assert!(
        permissions(&h.get("/api/admin/me", &owner).await.1).contains(&"audit_log".to_string())
    );
    assert!(
        !permissions(&h.get("/api/admin/me", &anna).await.1).contains(&"audit_log".to_string())
    );
    for token in [&anna, &help] {
        for path in [
            "/api/admin/audit",
            "/api/admin/audit/meta",
            "/api/admin/audit/1",
            "/api/admin/audit/clients?q=morozov",
        ] {
            assert_eq!(h.get(path, token).await.0, StatusCode::FORBIDDEN, "{path}");
        }
    }
    let client = h.client(7_400_000_311).await;
    let (status, found) = h.get("/api/admin/audit/clients?q=morozov", &owner).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found[0]["id"], client.0);
    assert_eq!(found[0]["label"], "Алексей");
    let (status, page) = h
        .get(&format!("/api/admin/audit?client={}", client.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["client"]["label"], "Алексей");
    assert_eq!(page["client"]["client_id"], client.0);
    let id = admin_id(&h, &owner, "anna@example.com").await;
    let (status, view) = h
        .write(
            Method::POST,
            &format!("/api/admin/admins/{id}/audit-log"),
            &anna,
            json!({ "on": true }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{view}");
    let (status, view) = h
        .write(
            Method::POST,
            &format!("/api/admin/admins/{id}/audit-log"),
            &owner,
            json!({ "on": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["audit_log"], true);
    let anna = h.login("anna@example.com").await;
    assert!(permissions(&h.get("/api/admin/me", &anna).await.1).contains(&"audit_log".to_string()));
    let (status, page) = h
        .get("/api/admin/audit?action=admin.permission_changed", &anna)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let item = &page["items"][0];
    assert_eq!(item["title"], "Выдано право «Журнал действий»");
    assert_eq!(item["actor"]["kind"], "admin");
    assert_eq!(item["actor"]["email"], "owner@example.com");
    assert_eq!(item["target"]["label"], "anna@example.com");
    assert_eq!(item["target"]["link"], "/admin/settings/admins");
    assert_eq!(item["changes"][0]["label"], "Журнал действий");
    assert_eq!(item["changes"][0]["after"], "есть");
    let (status, one) = h
        .get(&format!("/api/admin/audit/{}", item["id"]), &anna)
        .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["id"], item["id"]);
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/admin/admins/{id}/audit-log"),
            &owner,
            json!({ "on": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let anna = h.login("anna@example.com").await;
    assert_eq!(
        h.get("/api/admin/audit", &anna).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = h.get("/api/admin/audit?q=ab", &owner).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn ключу_api_журнал_закрыт_а_его_действия_подписаны_ключом() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, refused) = h
        .write(
            Method::POST,
            "/api/admin/api-keys",
            &owner,
            json!({ "name": "Журнал", "permissions": ["audit_log"], "password": PASSWORD }),
        )
        .await;
    assert!(status.is_client_error(), "{status} {refused}");
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/api-keys",
            &owner,
            json!({ "name": "CRM-выгрузка", "permissions": ["clients_read", "clients_write"], "password": PASSWORD }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let token = created["token"].as_str().unwrap().to_string();
    let key = created["key"]["id"].as_i64().unwrap();
    for path in ["/api/admin/audit", "/api/admin/audit/meta"] {
        assert_eq!(h.get(path, &token).await.0, StatusCode::FORBIDDEN, "{path}");
    }
    let client = h.client(7_400_000_301).await;
    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/clients/{}/note", client.0),
            &token,
            json!({ "note": "Купил годовой" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/clients/{}/note", client.0),
            &owner,
            json!({ "note": "Просил закрывающие документы" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, page) = h
        .get(
            &format!("/api/admin/audit?client={}&action=client.note", client.0),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{page}");
    assert_eq!(items[0]["actor"]["kind"], "admin");
    assert_eq!(items[1]["actor"]["kind"], "api_key");
    assert_eq!(items[1]["actor"]["name"], "CRM-выгрузка");
    assert_eq!(items[1]["actor"]["owner"], "owner@example.com");
    assert_eq!(
        items[1]["target"]["link"],
        format!("/admin/clients/{}", client.0)
    );
    assert_eq!(items[1]["title"], "Изменена заметка о клиенте");
    let (status, page) = h
        .get(&format!("/api/admin/audit?actor=key:{key}"), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["actor"]["kind"] == "api_key")
    );
    let (status, meta) = h.get("/api/admin/audit/meta", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        meta["keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["id"] == key)
    );
    let refused = sn_services::ext_modules::manifest::parse(
        r#"{
          "format": 1,
          "id": "watcher",
          "name": "Наблюдатель",
          "version": "1.0.0",
          "author": "Студия Пример",
          "webhook": "https://watcher.example.com/events",
          "events": ["client_registered"],
          "permissions": ["clients_read", "audit_log"]
        }"#,
    )
    .unwrap_err();
    assert!(
        refused.iter().any(|p| p.path == "permissions[1]"
            && p.message.contains("«Журнал действий» модулю дать нельзя")),
        "{refused:?}"
    );
    h.db.cleanup().await;
}
