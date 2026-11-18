use super::*;
use sn_services::backups::BackupEnv;
use sn_services::backups::crypt::KdfParams;
use sn_services::backups::format::RunKind;

fn backup_env(h: &Http) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sn-srv-backups-{}", Uuid::new_v4().simple()));
    h.services().set_backup_env(Some(BackupEnv {
        dir: Some(dir.join("copies")),
        kdf: KdfParams::QUICK,
        upload_dir: dir.join("uploads"),
        ..BackupEnv::default()
    }));
    dir
}

async fn raw(
    h: &Http,
    method: Method,
    path: &str,
    token: &str,
    body: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

#[tokio::test]
async fn копии_видит_только_владелец_или_админ_с_правом_и_скачивание_пишется_в_журнал() {
    let Some(h) = http().await else { return };
    let dir = backup_env(&h);
    let owner = h.owner().await;
    let admin = h.admin("admin@example.com", AdminRole::Admin).await;
    assert_eq!(
        h.get("/api/admin/backups", &admin).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = h.get("/api/admin/backups", &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["storage"]["configured"], true);
    assert_eq!(body["config"]["keep"], 7);
    assert_eq!(body["config"]["schedule"]["every"], "day");
    {
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query(
            "UPDATE admins SET permissions = '{backups}' WHERE email = 'admin@example.com'",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let admin = h.login("admin@example.com").await;
    assert_eq!(h.get("/api/admin/backups", &admin).await.0, StatusCode::OK);
    let made = h
        .services()
        .backups()
        .make_now(RunKind::Manual, None)
        .await
        .unwrap();
    let (status, headers, bytes) = raw(
        &h,
        Method::GET,
        &format!("/api/admin/backups/files/{}", made.name),
        &admin,
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, std::fs::read(&made.path).unwrap());
    let disposition = headers
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(disposition.contains(&made.name), "{disposition}");
    let downloads: i64 = {
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'backups.downloaded' AND details ->> 'file' = $1",
        )
        .bind(&made.name)
        .fetch_one(conn.conn())
        .await
        .unwrap()
    };
    assert_eq!(downloads, 1);
    for bad in [
        "/api/admin/backups/files/..%2F..%2Fetc%2Fpasswd",
        "/api/admin/backups/files/backup.json",
    ] {
        assert_eq!(h.get(bad, &owner).await.0, StatusCode::NOT_FOUND, "{bad}");
    }
    let _ = std::fs::remove_dir_all(dir);
    h.db.cleanup().await;
}

#[tokio::test]
async fn во_время_восстановления_панель_и_вебхуки_отвечают_что_идёт_восстановление() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let paused = h
        .services()
        .pause_gate()
        .pause(std::time::Duration::from_millis(10))
        .await;
    let (status, headers, body) = h
        .send(Method::GET, "/api/admin/me", Some(&owner), None)
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "system.restoring");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Идёт восстановление")
    );
    assert_eq!(headers.get(header::RETRY_AFTER).unwrap(), "30");
    let (status, _, _) = h
        .send(
            Method::POST,
            "/api/webhooks/cryptopay",
            None,
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _, body) = h.send(Method::GET, "/api/health", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, _) = h
        .send(
            Method::GET,
            "/api/admin/backups/restore/status?watch=abc",
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    drop(paused);
    let (status, _, _) = h
        .send(Method::GET, "/api/admin/me", Some(&owner), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    h.db.cleanup().await;
}

#[tokio::test]
async fn загруженная_копия_проверяется_перед_восстановлением_а_чужой_файл_нет() {
    let Some(h) = http().await else { return };
    let dir = backup_env(&h);
    let owner = h.owner().await;
    let made = h
        .services()
        .backups()
        .make_now(RunKind::Manual, None)
        .await
        .unwrap();
    let bytes = std::fs::read(&made.path).unwrap();
    let half = bytes.len() / 2;
    let (status, _, body) = raw(
        &h,
        Method::POST,
        "/api/admin/backups/upload?name=copy.tar.gz",
        &owner,
        bytes[..half].to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upload: Value = serde_json::from_slice(&body).unwrap();
    let id = upload["id"].as_str().unwrap().to_string();
    assert_eq!(upload["sealed"], false);
    let (status, _, body) = raw(
        &h,
        Method::POST,
        &format!("/api/admin/backups/upload/{id}?name=copy.tar.gz"),
        &owner,
        bytes[half..].to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(upload["size"], bytes.len());
    let (status, preview) = h
        .write(
            Method::POST,
            "/api/admin/backups/preview",
            &owner,
            json!({ "source": { "kind": "upload", "id": id } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["same_version"], true);
    assert_eq!(preview["keys"], "no_server_key");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/backups/restore",
            &owner,
            json!({ "source": { "kind": "upload", "id": id }, "confirm": { "password": "неверный" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.wrong_password");
    let (status, _, body) = raw(
        &h,
        Method::POST,
        "/api/admin/backups/upload?name=photo.zip",
        &owner,
        b"PK\x03\x04 not a backup".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["code"], "backups.damaged");
    let _ = std::fs::remove_dir_all(dir);
    h.db.cleanup().await;
}
