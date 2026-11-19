use super::*;
use sn_modules::Module;
use sn_modules::catalog::ticket_files::TicketFilesModule;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x10\0\0\0\x08\x08\x06\0\0\0\0\0\0\0\0\0\0\0IEND\0\0\0\0";

async fn files_http() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(vec![Arc::new(TicketFilesModule)]).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

async fn upload(
    h: &Http,
    path: &str,
    token: &str,
    name: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("{path}?name={}", urlencoding(name)))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn download(h: &Http, path: &str, token: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(Method::GET).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = router(h.state.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

fn header_text(headers: &HeaderMap, name: header::HeaderName) -> &str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

#[tokio::test]
async fn файлы_обращения_видят_только_его_клиент_и_поддержка() {
    let Some(h) = files_http().await else { return };
    let owner = h.owner().await;
    let watcher = h.admin("watcher@example.com", AdminRole::ReadOnly).await;
    let helper = h.admin("helper@example.com", AdminRole::Support).await;
    let alice = h.client(5_200_001).await;
    let alice_token = h.client_token(alice).await;
    let bob = h.client(5_200_002).await;
    let bob_token = h.client_token(bob).await;
    let files = "/api/client/support/files";

    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(
        site["support_files"],
        json!({ "max_bytes": 10 * 1024 * 1024, "max_files": 5 })
    );

    let (status, png) = upload(
        &h,
        files,
        &alice_token,
        "экран.PNG",
        "image/png",
        PNG.to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{png}");
    assert_eq!(png["kind"], "image");
    assert_eq!(png["name"], "экран.png");
    assert_eq!(
        (png["width"].as_i64(), png["height"].as_i64()),
        (Some(16), Some(8))
    );
    let (status, pdf) = upload(
        &h,
        files,
        &alice_token,
        "чек.pdf",
        "application/pdf",
        b"%PDF-1.7 test".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pdf}");
    assert_eq!(pdf["kind"], "file");

    for (name, bytes) in [
        (
            "x.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
        ),
        ("page.html", b"<html></html>".to_vec()),
        (
            "notes.txt",
            b"<!DOCTYPE html><html><script>alert(1)</script></html>".to_vec(),
        ),
        ("app.exe", b"MZ\x90\x00\x03\x00".to_vec()),
    ] {
        let (status, body) = upload(&h, files, &alice_token, name, "text/plain", bytes).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name}: {body}");
        assert_eq!(body["error"]["code"], "support.file_type", "{name}");
    }
    let (status, body) = upload(
        &h,
        files,
        &alice_token,
        "big.txt",
        "text/plain",
        vec![b'a'; 10 * 1024 * 1024 + 1],
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.file_big");

    let png_id = png["id"].as_i64().unwrap();
    let pdf_id = pdf["id"].as_i64().unwrap();
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/support/tickets",
            Some(&bob_token),
            Some(json!({ "body": "Чужой файл", "files": [png_id] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.file_missing");
    let (status, _, _) = download(&h, &format!("{files}/{png_id}"), Some(&bob_token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, opened) = h
        .send(
            Method::POST,
            "/api/client/support/tickets",
            Some(&alice_token),
            Some(json!({ "body": "", "files": [png_id, pdf_id] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let ticket = opened["ticket_id"].as_i64().unwrap();
    let (status, thread) = h
        .get(
            &format!("/api/client/support/tickets/{ticket}"),
            &alice_token,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let attached = thread["messages"][0]["attachments"].as_array().unwrap();
    assert_eq!(attached.len(), 2);
    assert_eq!(attached[0]["name"], "экран.png");

    let (status, headers, bytes) =
        download(&h, &format!("{files}/{png_id}"), Some(&alice_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, PNG);
    assert_eq!(header_text(&headers, header::CONTENT_TYPE), "image/png");
    assert!(header_text(&headers, header::CONTENT_DISPOSITION).starts_with("inline;"));
    assert_eq!(
        header_text(&headers, header::X_CONTENT_TYPE_OPTIONS),
        "nosniff"
    );
    assert!(header_text(&headers, header::CONTENT_SECURITY_POLICY).contains("sandbox"));
    let (status, headers, _) = download(&h, &format!("{files}/{pdf_id}"), Some(&alice_token)).await;
    assert_eq!(status, StatusCode::OK);
    let disposition = header_text(&headers, header::CONTENT_DISPOSITION);
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    assert!(
        disposition.contains("filename*=UTF-8''%D1%87%D0%B5%D0%BA.pdf"),
        "{disposition}"
    );

    let (status, _, _) = download(&h, &format!("{files}/{pdf_id}"), Some(&bob_token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = download(&h, &format!("{files}/{pdf_id}"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = download(
        &h,
        &format!("/api/admin/support/files/{pdf_id}"),
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = download(
        &h,
        &format!("/api/admin/support/files/{pdf_id}"),
        Some(&helper),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = download(
        &h,
        &format!("/api/admin/support/files/{pdf_id}"),
        Some(&watcher),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, body) = h
        .send(
            Method::POST,
            &format!("{files}/{pdf_id}/link"),
            Some(&bob_token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _, link) = h
        .send(
            Method::POST,
            &format!("{files}/{pdf_id}/link"),
            Some(&alice_token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{link}");
    let url = link["url"].as_str().unwrap();
    assert!(url.starts_with("/api/public/support-files/"));
    let (status, headers, bytes) = download(&h, url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"%PDF-1.7 test");
    assert!(header_text(&headers, header::CONTENT_DISPOSITION).starts_with("attachment;"));
    let (status, _, _) = download(&h, "/api/public/support-files/0123", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, body) = h
        .send(
            Method::POST,
            &format!("/api/client/support/tickets/{ticket}"),
            Some(&alice_token),
            Some(json!({ "body": "Ещё раз", "files": [png_id] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.file_missing");

    let (status, reply) = upload(
        &h,
        "/api/admin/support/files",
        &helper,
        "ответ.png",
        "image/png",
        PNG.to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let (status, thread) = h
        .write(
            Method::POST,
            &format!("/api/admin/support/tickets/{ticket}"),
            &helper,
            json!({ "body": "", "files": [reply["id"]] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    assert_eq!(thread["messages"][1]["attachments"][0]["name"], "ответ.png");
    let (status, list) = h
        .get("/api/admin/support/tickets?filter=answered", &helper)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"][0]["preview"], "Файл: ответ.png");
    assert_eq!(list["files"]["max_files"], 5);
    let (status, found) = h
        .get(
            &format!(
                "/api/admin/support/tickets?filter=answered&q={}",
                urlencoding("чек")
            ),
            &helper,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["items"].as_array().map(Vec::len), Some(1));
    let reply_id = reply["id"].as_i64().unwrap();
    let (status, _, _) = download(&h, &format!("{files}/{reply_id}"), Some(&alice_token)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = download(&h, &format!("{files}/{reply_id}"), Some(&bob_token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = h
        .write(
            Method::DELETE,
            &format!("/api/admin/support/tickets/{ticket}"),
            &watcher,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = h
        .write(
            Method::DELETE,
            &format!("/api/admin/support/tickets/{ticket}"),
            &helper,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for id in [png_id, pdf_id, reply_id] {
        let (status, _, _) =
            download(&h, &format!("/api/admin/support/files/{id}"), Some(&owner)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, _, _) = download(&h, url, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mut conn = h.services().store().conn().await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM ticket_files")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(left, 0);
    let audit: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'ticket.deleted'")
            .fetch_one(conn.conn())
            .await
            .unwrap();
    assert_eq!(audit, 1);
    drop(conn);
    h.db.cleanup().await;
}

#[tokio::test]
async fn лимиты_и_срок_хранения_файлов_берутся_из_настроек_модуля() {
    let Some(h) = files_http().await else { return };
    let owner = h.owner().await;
    let alice = h.client(5_200_011).await;
    let token = h.client_token(alice).await;
    let files = "/api/client/support/files";
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/modules/ticket_files/config",
            &owner,
            json!({ "config": { "max_mb": 21 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/modules/ticket_files/config",
            &owner,
            json!({ "config": { "max_mb": 1, "max_files": 1, "keep_days": 30 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(
        site["support_files"],
        json!({ "max_bytes": 1024 * 1024, "max_files": 1 })
    );

    let (status, body) = upload(
        &h,
        files,
        &token,
        "big.log",
        "text/plain",
        vec![b'x'; 1024 * 1024 + 10],
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.file_big");
    assert!(body["error"]["message"].as_str().unwrap().contains("1 МБ"));
    let (_, first) = upload(&h, files, &token, "a.txt", "text/plain", b"one".to_vec()).await;
    let (_, second) = upload(&h, files, &token, "b.txt", "text/plain", b"two".to_vec()).await;
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/support/tickets",
            Some(&token),
            Some(json!({ "body": "два файла", "files": [first["id"], second["id"]] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.files_many");
    let (status, _, body) = h
        .send(
            Method::DELETE,
            &format!("{files}/{}", second["id"]),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, opened) = h
        .send(
            Method::POST,
            "/api/client/support/tickets",
            Some(&token),
            Some(json!({ "body": "один файл", "files": [first["id"]] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let ticket = opened["ticket_id"].as_i64().unwrap();
    let (status, _, _) = h
        .send(
            Method::POST,
            &format!("/api/client/support/tickets/{ticket}/close"),
            Some(&token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, stale) = upload(&h, files, &token, "c.txt", "text/plain", b"three".to_vec()).await;
    {
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query("UPDATE tickets SET closed_at = now() - interval '31 days' WHERE id = $1")
            .bind(ticket)
            .execute(conn.conn())
            .await
            .unwrap();
        sqlx::query("UPDATE ticket_files SET created_at = now() - interval '25 hours' WHERE message_id IS NULL")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let (_, fresh) = upload(&h, files, &token, "d.txt", "text/plain", b"four".to_vec()).await;
    let module = TicketFilesModule;
    module
        .run_task(&h.services().module_context(), "purge")
        .await
        .unwrap();
    let (status, thread) = h
        .get(&format!("/api/client/support/tickets/{ticket}"), &token)
        .await;
    assert_eq!(status, StatusCode::OK);
    let file = &thread["messages"][0]["attachments"][0];
    assert_eq!(file["name"], "a.txt");
    assert_eq!(file["removed"], true);
    let (status, _, body) = download(&h, &format!("{files}/{}", first["id"]), Some(&token)).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let (status, _, _) = download(&h, &format!("{files}/{}", stale["id"]), Some(&token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = download(&h, &format!("{files}/{}", fresh["id"]), Some(&token)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = h
        .write(
            Method::PATCH,
            "/api/admin/modules/ticket_files",
            &owner,
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["support_files"], Value::Null);
    let (status, body) = upload(&h, files, &token, "e.txt", "text/plain", b"five".to_vec()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.files_off");
    h.db.cleanup().await;
}

#[tokio::test]
async fn место_для_файлов_ограничено_на_клиента_и_на_всю_установку() {
    let Some(h) = files_http().await else { return };
    let owner = h.owner().await;
    let alice = h.client(5_200_031).await;
    let bob = h.client(5_200_032).await;
    let alice_token = h.client_token(alice).await;
    let bob_token = h.client_token(bob).await;
    let files = "/api/client/support/files";
    let text = |kb: usize| vec![b'x'; kb * 1024];
    let config = "/api/admin/modules/ticket_files/config";
    let (status, body) = h
        .write(
            Method::PUT,
            config,
            &owner,
            json!({ "config": { "client_mb": 0 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            config,
            &owner,
            json!({ "config": { "client_mb": 1, "total_gb": 1 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = upload(&h, files, &alice_token, "a.txt", "text/plain", text(700)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = upload(&h, files, &alice_token, "b.txt", "text/plain", text(400)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.files_quota");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ваших файлов")
    );
    let (status, body) = upload(&h, files, &bob_token, "c.txt", "text/plain", text(400)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    {
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO ticket_files (author, name, mime, size, data) \
             VALUES ('support', 'archive.pdf', 'application/pdf', $1, $2)",
        )
        .bind(1_073_741_824_i32 - 500 * 1024)
        .bind(vec![0_u8])
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let (status, body) = upload(&h, files, &bob_token, "d.txt", "text/plain", text(400)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.files_quota");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("в поддержке")
    );
    let (status, body) = upload(
        &h,
        "/api/admin/support/files",
        &owner,
        "answer.txt",
        "text/plain",
        text(400),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    {
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query(
            "UPDATE ticket_files SET data = NULL, purged_at = now() WHERE name = 'archive.pdf'",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let (status, body) = upload(&h, files, &bob_token, "e.txt", "text/plain", text(400)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.db.cleanup().await;
}
