use super::*;

fn dump(clients: usize) -> Vec<u8> {
    let mut sql = String::from(
        "--\n-- PostgreSQL database dump\n--\n\nCOPY public.system_settings (id, key, value) FROM stdin;\ns1\tdefault_currency\tusd\n\\.\n\n\
         COPY public.clients (id, email, role, balance, preferred_lang, telegram_id, telegram_username, is_blocked, block_reason, created_at) FROM stdin;\n",
    );
    let note = "заметка ".repeat(20);
    for i in 0..clients {
        sql.push_str(&format!(
            "c{i}\tclient{i}@example.com\tCLIENT\t{}.25\tru\t{}\tuser{i}\tf\t{note}\t2026-01-01 10:00:00\n",
            i % 7,
            500_000 + i
        ));
    }
    sql.push_str("\\.\n\nCOPY public.subscriptions (id, owner_id, remnawave_uuid, subscription_index, expire_at) FROM stdin;\n");
    sql.push_str("s0\tc0\t801\t0\t2026-12-01 00:00:00\ns1\tc1\t802\t0\t2026-12-01 00:00:00\n\\.\n");
    sql.into_bytes()
}

async fn upload(h: &Http, token: &str, path: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn wait(h: &Http, token: &str, id: &str, states: &[&str]) -> Value {
    for _ in 0..1200 {
        let (status, run) = h.get(&format!("/api/admin/import/{id}"), token).await;
        assert_eq!(status, StatusCode::OK, "{run}");
        if states.iter().any(|s| run["state"] == *s) {
            return run;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("перенос {id} не дошёл до {states:?}");
}

#[tokio::test]
async fn перенос_загружается_большим_файлом_проверяется_и_переносится() {
    let Some(h) = http().await else { return };
    h.remna
        .insert_user(sn_remnawave::fake::sample_user(801, "user0"));
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h.get("/api/admin/import", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, view) = h.get("/api/admin/import", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["remna"], true);
    assert_eq!(view["run"], Value::Null);

    let (status, body) = upload(
        &h,
        &owner,
        "/api/admin/import/upload?source=nobody",
        dump(10),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = upload(
        &h,
        &owner,
        "/api/admin/import/upload?source=stealthnet",
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let big = dump(6_000);
    assert!(
        big.len() > 1024 * 1024 + 100_000,
        "файл больше общего лимита тела"
    );
    let (status, run) = upload(
        &h,
        &owner,
        "/api/admin/import/upload?source=stealthnet&name=stealthnet-backup.sql",
        big,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["state"], "checking");
    assert_eq!(run["file"]["name"], "stealthnet-backup.sql");
    let id = run["id"].as_str().unwrap().to_string();
    let ready = wait(&h, &owner, &id, &["ready", "failed"]).await;
    assert_eq!(ready["state"], "ready", "{ready}");
    assert_eq!(ready["preview"]["counts"]["clients"], 6_000);
    assert_eq!(ready["preview"]["remna"]["found"], 1);
    assert_eq!(ready["preview"]["remna"]["missing"], 1);
    assert_eq!(ready["preview"]["currency"]["switch"], false);

    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/admin/import/{id}/start"),
            &support,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, started) = h
        .write(
            Method::POST,
            &format!("/api/admin/import/{id}/start"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let done = wait(&h, &owner, &id, &["done", "failed"]).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["outcome"]["counts"]["clients"], 6_000);
    assert_eq!(done["outcome"]["counts"]["subscriptions"], 1);

    let request = Request::builder()
        .uri(format!("/api/admin/import/{id}/report"))
        .header(header::AUTHORIZATION, format!("Bearer {owner}"))
        .body(Body::empty())
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains(".csv")
    );
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.starts_with('\u{feff}'));
    assert!(text.contains("не найден"), "{text}");

    let (status, view) = h.get("/api/admin/import", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["history"].as_array().unwrap().len(), 1);
    assert_eq!(view["clients"], 6_000);
    let (status, again) = h
        .write(
            Method::POST,
            &format!("/api/admin/import/{id}/start"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{again}");
}

#[tokio::test]
async fn без_remnawave_перенос_не_запускается_а_файл_можно_убрать() {
    let Some(h) = http().await else { return };
    h.services().set_remna(None);
    let owner = h.owner().await;
    let (status, run) = upload(
        &h,
        &owner,
        "/api/admin/import/upload?source=stealthnet",
        dump(3),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    let id = run["id"].as_str().unwrap().to_string();
    let ready = wait(&h, &owner, &id, &["ready", "failed"]).await;
    assert_eq!(ready["preview"]["remna"]["connected"], false);
    let (status, refused) = h
        .write(
            Method::POST,
            &format!("/api/admin/import/{id}/start"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "import.remna_required");
    let (status, _) = h
        .write(
            Method::DELETE,
            &format!("/api/admin/import/{id}"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, view) = h.get("/api/admin/import", &owner).await;
    assert_eq!(view["run"], Value::Null);
}
