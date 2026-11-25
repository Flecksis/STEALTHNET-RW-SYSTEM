use super::*;
use sn_domain::ids::{TicketFileId, TicketId};
use sn_domain::support::TicketChannel;
use sn_modules::catalog;
use sn_services::Actor;
use sn_services::support::ReplyRequest;
use sn_store::{NewSession, SessionChannel, SessionScope};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x10\0\0\0\x08\x08\x06\0\0\0\0\0\0\0\0\0\0\0IEND\0\0\0\0";
const TICKETS: &str = "/api/client/support/tickets";
const FILES: &str = "/api/client/support/files";
const UNREAD: &str = "/api/client/support/unread";

async fn support_http() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(catalog::all()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    services.support().start_unread().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

async fn mini_token(h: &Http, client: ClientId) -> String {
    h.services()
        .auth()
        .client_session_for(client, SessionChannel::Miniapp, &Default::default(), false)
        .await
        .unwrap()
        .token
}

async fn upload(h: &Http, token: &str, name: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("{FILES}?name={name}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn open(h: &Http, token: &str, body: Value) -> i64 {
    let (status, _, opened) = h.send(Method::POST, TICKETS, Some(token), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    opened["ticket_id"].as_i64().unwrap()
}

async fn answer(h: &Http, ticket: i64, body: &str, files: &[i64], close: bool) {
    let admin = h
        .services()
        .auth()
        .upsert_owner("owner@example.com", PASSWORD)
        .await
        .unwrap()
        .0;
    let files: Vec<TicketFileId> = files.iter().map(|id| TicketFileId(*id)).collect();
    h.services()
        .support()
        .reply_with_files(
            TicketId(ticket),
            &ReplyRequest {
                body: body.into(),
                close,
            },
            &files,
            Actor::Admin(admin),
        )
        .await
        .unwrap();
}

async fn unread(h: &Http, token: &str) -> i64 {
    let (status, body) = h.get(UNREAD, token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, me) = h.get("/api/client/me", token).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["support_unread"], body["tickets"], "{me}");
    body["tickets"].as_i64().unwrap()
}

async fn item(h: &Http, token: &str, ticket: i64) -> Value {
    let (status, list) = h.get(TICKETS, token).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list.as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == ticket)
        .cloned()
        .unwrap()
}

async fn core(h: &Http, owner: &str, body: Value) {
    let (status, saved) = h
        .write(Method::PUT, "/api/admin/settings/core", owner, body)
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
}

#[tokio::test]
async fn обращения_из_мини_приложения_принимаются_по_режиму_поддержки() {
    let Some(h) = support_http().await else {
        return;
    };
    let owner = h.owner().await;
    let alice = h.client(5_300_001).await;
    let token = mini_token(&h, alice).await;

    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["support_tickets"], true);
    assert_eq!(
        site["support_ai"],
        json!({ "cabinet": false, "miniapp": false })
    );
    let first = open(&h, &token, json!({ "body": "Не подключается iPhone" })).await;
    let thread = h
        .services()
        .support()
        .thread(TicketId(first), Actor::System)
        .await
        .unwrap();
    assert_eq!(thread.ticket.channel, TicketChannel::Miniapp);
    assert_eq!(thread.ai, None);
    let (status, body) = h.get(&format!("{TICKETS}/{first}"), &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ai"], Value::Null);
    let (status, body) = h
        .write(
            Method::POST,
            &format!("{TICKETS}/{first}/operator"),
            &token,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ai"], Value::Null);

    core(
        &h,
        &owner,
        json!({ "support_url": "https://t.me/help_chat", "support_mode": "both" }),
    )
    .await;
    let second = open(&h, &token, json!({ "body": "Нужен чек" })).await;
    assert_ne!(first, second);

    core(&h, &owner, json!({ "support_mode": "link" })).await;
    let (status, _, body) = h
        .send(
            Method::POST,
            TICKETS,
            Some(&token),
            Some(json!({ "body": "Ещё вопрос" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.tickets_off");
    let (status, list) = h.get(TICKETS, &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().map(Vec::len), Some(2));
    let (status, body) = h
        .write(
            Method::POST,
            &format!("{TICKETS}/{first}"),
            &token,
            json!({ "body": "Дополню" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["messages"].as_array().map(Vec::len), Some(2));
    h.db.cleanup().await;
}

#[tokio::test]
async fn чужое_обращение_из_мини_приложения_не_открыть_и_не_изменить() {
    let Some(h) = support_http().await else {
        return;
    };
    let alice = h.client(5_300_011).await;
    let alice_token = mini_token(&h, alice).await;
    let bob = h.client(5_300_012).await;
    let bob_token = mini_token(&h, bob).await;
    let (status, file) = upload(&h, &alice_token, "screen.png", PNG.to_vec()).await;
    assert_eq!(status, StatusCode::OK, "{file}");
    let file = file["id"].as_i64().unwrap();
    let ticket = open(
        &h,
        &alice_token,
        json!({ "body": "Мой вопрос", "files": [file] }),
    )
    .await;
    answer(&h, ticket, "Ответ только для вас", &[], false).await;

    let (status, list) = h.get(TICKETS, &bob_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([]));
    assert_eq!(unread(&h, &bob_token).await, 0);

    for id in [ticket, ticket + 1_000, -1, 0] {
        let path = format!("{TICKETS}/{id}");
        let (status, body) = h.get(&path, &bob_token).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}: {body}");
        for (path, body) in [
            (path.clone(), json!({ "body": "Чужой ответ" })),
            (format!("{path}/close"), json!({})),
            (format!("{path}/operator"), json!({})),
        ] {
            let (status, answer) = h.write(Method::POST, &path, &bob_token, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {answer}");
            assert!(!answer.to_string().contains("Ответ только"), "{answer}");
        }
    }
    let (status, body) = h.get(&format!("{TICKETS}/abc"), &bob_token).await;
    assert!(status.is_client_error(), "{status}: {body}");

    for (method, path) in [
        (Method::GET, format!("{FILES}/{file}")),
        (Method::POST, format!("{FILES}/{file}/link")),
        (Method::DELETE, format!("{FILES}/{file}")),
    ] {
        let body = (method != Method::GET).then(|| json!({}));
        let (status, _, answer) = h.send(method.clone(), &path, Some(&bob_token), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}: {answer}");
    }
    let (status, _, body) = h
        .send(
            Method::POST,
            TICKETS,
            Some(&bob_token),
            Some(json!({ "body": "Прикреплю чужое", "files": [file] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.file_missing");

    for path in [
        TICKETS.to_string(),
        UNREAD.to_string(),
        format!("{TICKETS}/{ticket}"),
        format!("{FILES}/{file}"),
    ] {
        let (status, _, _) = h.send(Method::GET, &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }

    assert_eq!(item(&h, &alice_token, ticket).await["unread"], 1);
    let (status, own) = h.get(&format!("{TICKETS}/{ticket}"), &alice_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(own["ticket"]["status"], "open");
    assert_eq!(own["messages"].as_array().map(Vec::len), Some(2));
    h.db.cleanup().await;
}

#[tokio::test]
async fn непрочитанный_ответ_пропадает_когда_клиент_открыл_переписку() {
    let Some(h) = support_http().await else {
        return;
    };
    let helper = h.admin("helper@example.com", AdminRole::Support).await;
    let alice = h.client(5_300_021).await;
    let token = mini_token(&h, alice).await;
    let cabinet = h.client_token(alice).await;
    let ticket = open(&h, &token, json!({ "body": "Не подключается iPhone" })).await;
    let path = format!("{TICKETS}/{ticket}");
    let own = item(&h, &token, ticket).await;
    assert_eq!(own["preview"]["author"], "client");
    assert_eq!(own["preview"]["text"], "Не подключается iPhone");
    assert_eq!(
        (own["unread"].as_i64(), unread(&h, &token).await),
        (Some(0), 0)
    );

    answer(&h, ticket, "Обновите подписку", &[], false).await;
    answer(&h, ticket, "И перезайдите", &[], false).await;
    let own = item(&h, &token, ticket).await;
    assert_eq!(own["preview"]["author"], "support");
    assert_eq!(own["preview"]["text"], "И перезайдите");
    assert_eq!(own["unread"], 2);
    assert_eq!(own["status"], "open");
    assert_eq!(unread(&h, &token).await, 1);
    assert_eq!(unread(&h, &cabinet).await, 1);

    let (status, seen) = h
        .get(&format!("/api/admin/support/tickets/{ticket}"), &helper)
        .await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(unread(&h, &token).await, 1);

    let other = open(&h, &token, json!({ "body": "Нужен чек" })).await;
    answer(&h, other, "Отправили", &[], true).await;
    assert_eq!(unread(&h, &token).await, 2);
    let closed = item(&h, &token, other).await;
    assert_eq!(
        (closed["status"].as_str(), closed["unread"].as_i64()),
        (Some("closed"), Some(1))
    );

    let (status, thread) = h.get(&path, &token).await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    assert_eq!(thread["messages"].as_array().map(Vec::len), Some(3));
    assert_eq!(thread["unread"], 1);
    assert_eq!(item(&h, &token, ticket).await["unread"], 0);
    assert_eq!(unread(&h, &token).await, 1);
    let (_, thread) = h.get(&format!("{TICKETS}/{other}"), &token).await;
    assert_eq!(thread["unread"], 0);
    assert_eq!(unread(&h, &token).await, 0);

    answer(&h, ticket, "Получилось?", &[], false).await;
    assert_eq!(unread(&h, &token).await, 1);
    let (status, thread) = h
        .write(Method::POST, &path, &token, json!({ "body": "Да" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    assert_eq!(thread["unread"], 0);
    assert_eq!(unread(&h, &token).await, 0);

    answer(&h, ticket, "Отлично", &[], false).await;
    let view = {
        let hash = sn_services::auth::token_hash("view-token");
        let mut conn = h.services().store().conn().await.unwrap();
        conn.sessions()
            .create_client(
                alice,
                SessionChannel::Cabinet,
                SessionScope::SubscriptionView,
                &NewSession {
                    token_hash: hash,
                    expires_at: Utc::now() + chrono::Duration::days(1),
                    ip: None,
                    user_agent: None,
                },
            )
            .await
            .unwrap();
        "view-token"
    };
    let (status, thread) = h.get(&path, view).await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    assert_eq!(thread["unread"], 1);
    assert_eq!(unread(&h, &token).await, 1);
    h.db.cleanup().await;
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[tokio::test]
async fn клиенту_не_уходит_номер_администратора_и_служебные_сообщения() {
    let Some(h) = support_http().await else {
        return;
    };
    let helper = h.admin("helper@example.com", AdminRole::Support).await;
    let alice = h.client(5_300_061).await;
    let cabinet = h.client_token(alice).await;
    let mini = mini_token(&h, alice).await;
    let ticket = open(&h, &cabinet, json!({ "body": "Не подключается iPhone" })).await;
    answer(&h, ticket, "Обновите подписку", &[], false).await;
    h.services()
        .support()
        .reply(
            TicketId(ticket),
            &ReplyRequest {
                body: "служебная запись".into(),
                close: false,
            },
            Actor::System,
        )
        .await
        .unwrap();
    assert_eq!(unread(&h, &cabinet).await, 1);

    let path = format!("{TICKETS}/{ticket}");
    let checked = |thread: &Value, authors: &[&str]| {
        assert_eq!(keys(thread), ["ai", "messages", "ticket", "unread"]);
        let messages = thread["messages"].as_array().unwrap();
        for message in messages {
            assert_eq!(
                keys(message),
                ["attachments", "author", "body", "created_at"]
            );
        }
        let found: Vec<&str> = messages
            .iter()
            .map(|m| m["author"].as_str().unwrap())
            .collect();
        assert_eq!(found, authors);
        let text = thread.to_string();
        assert!(!text.contains("admin"), "{text}");
        assert!(!text.contains("служебная"), "{text}");
    };
    let (status, thread) = h.get(&path, &cabinet).await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    checked(&thread, &["client", "support"]);
    assert_eq!(thread["unread"], 0);
    assert_eq!(unread(&h, &cabinet).await, 0);
    assert_eq!(unread(&h, &mini).await, 0);
    let (status, thread) = h.get(&path, &mini).await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    checked(&thread, &["client", "support"]);
    let (status, thread) = h
        .write(
            Method::POST,
            &format!("{path}/operator"),
            &cabinet,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    checked(&thread, &["client", "support"]);
    let (status, thread) = h
        .write(Method::POST, &path, &cabinet, json!({ "body": "Помогло" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    checked(&thread, &["client", "support", "client"]);

    let (status, thread) = h
        .get(&format!("/api/admin/support/tickets/{ticket}"), &helper)
        .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    let messages = thread["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4, "{thread}");
    assert!(messages[1]["admin_id"].is_i64(), "{thread}");
    assert_eq!(messages[2]["author"], "system");
    assert!(!thread["admins"].as_array().unwrap().is_empty(), "{thread}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn ответы_до_обновления_не_считаются_непрочитанными() {
    let Some(h) = support_http().await else {
        return;
    };
    let alice = h.client(5_300_031).await;
    let token = mini_token(&h, alice).await;
    let ticket = open(&h, &token, json!({ "body": "Старый вопрос" })).await;
    answer(&h, ticket, "Старый ответ", &[], false).await;
    assert_eq!(unread(&h, &token).await, 1);
    let clear = async || {
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query("DELETE FROM bot_state WHERE key = $1")
            .bind(sn_store::UNREAD_FROM_KEY)
            .execute(conn.conn())
            .await
            .unwrap();
    };
    clear().await;
    assert_eq!(unread(&h, &token).await, 0);
    assert!(h.services().support().start_unread().await.unwrap());
    assert_eq!(unread(&h, &token).await, 0);
    assert_eq!(item(&h, &token, ticket).await["unread"], 0);
    answer(&h, ticket, "Новый ответ", &[], false).await;
    assert!(!h.services().support().start_unread().await.unwrap());
    assert_eq!(unread(&h, &token).await, 1);
    assert_eq!(item(&h, &token, ticket).await["unread"], 1);
    h.db.cleanup().await;
}

#[tokio::test]
async fn вложения_из_мини_приложения_проверяются_как_в_кабинете() {
    let Some(h) = support_http().await else {
        return;
    };
    let alice = h.client(5_300_041).await;
    let token = mini_token(&h, alice).await;
    for (name, bytes) in [
        (
            "x.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
        ),
        ("page.html", b"<html></html>".to_vec()),
        (
            "photo.png",
            b"<!DOCTYPE html><script>alert(1)</script>".to_vec(),
        ),
        ("app.exe", b"MZ\x90\x00\x03\x00".to_vec()),
    ] {
        let (status, body) = upload(&h, &token, name, bytes).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name}: {body}");
        assert_eq!(body["error"]["code"], "support.file_type", "{name}");
    }
    let (status, body) = upload(&h, &token, "big.txt", vec![b'a'; 10 * 1024 * 1024 + 1]).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.file_big");
    let mut ids = Vec::new();
    for n in 0..6 {
        let (status, file) = upload(&h, &token, &format!("{n}.png"), PNG.to_vec()).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        ids.push(file["id"].as_i64().unwrap());
    }
    let (status, _, body) = h
        .send(
            Method::POST,
            TICKETS,
            Some(&token),
            Some(json!({ "body": "", "files": ids })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.files_many");
    let ticket = open(&h, &token, json!({ "body": "", "files": &ids[..2] })).await;
    let own = item(&h, &token, ticket).await;
    assert_eq!(own["preview"]["text"], "");
    assert_eq!(own["preview"]["file"], "0.png");
    let (status, _, link) = h
        .send(
            Method::POST,
            &format!("{FILES}/{}/link", ids[0]),
            Some(&token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{link}");
    let url = link["url"].as_str().unwrap();
    assert!(url.starts_with("/api/public/support-files/"), "{url}");
    let (status, headers, _) = h.send(Method::GET, url, None, None).await;
    assert_eq!(status, StatusCode::OK);
    let disposition = headers[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(disposition.starts_with("attachment;"), "{disposition}");

    h.services()
        .modules()
        .set_enabled(
            h.services().store(),
            sn_domain::module::ModuleId::TicketFiles,
            false,
            None,
        )
        .await
        .unwrap();
    h.services().reload().await.unwrap();
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["support_files"], Value::Null);
    let (status, body) = upload(&h, &token, "late.png", PNG.to_vec()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.files_off");
    h.db.cleanup().await;
}

#[tokio::test]
async fn ии_чат_мини_приложения_отвечает_только_там_где_включён() {
    let Some(h) = support_http().await else {
        return;
    };
    let alice = h.client(5_300_051).await;
    let token = mini_token(&h, alice).await;
    let cabinet = h.client_token(alice).await;
    let ai = |miniapp: bool, cabinet: bool| {
        h.services()
            .modules()
            .apply_rows(&[sn_store::ModuleStateRow {
                module: "ai".into(),
                enabled: true,
                config: json!({ "api_key": "gsk_server_test", "miniapp": miniapp, "cabinet": cabinet }),
                updated_at: Utc::now(),
            }]);
    };
    ai(false, true);
    let plain = open(&h, &token, json!({ "body": "Вопрос оператору" })).await;
    let (_, thread) = h.get(&format!("{TICKETS}/{plain}"), &token).await;
    assert_eq!(thread["ai"], Value::Null, "{thread}");
    let site_ticket = open(&h, &cabinet, json!({ "body": "Вопрос на сайте" })).await;
    let (_, thread) = h.get(&format!("{TICKETS}/{site_ticket}"), &token).await;
    assert_eq!(thread["ai"]["state"], "active", "{thread}");

    ai(true, false);
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(
        site["support_ai"],
        json!({ "cabinet": false, "miniapp": true })
    );
    let chat = open(&h, &token, json!({ "body": "Вопрос ИИ" })).await;
    let (_, thread) = h.get(&format!("{TICKETS}/{chat}"), &token).await;
    assert_eq!(thread["ai"]["state"], "active", "{thread}");
    let (status, thread) = h
        .write(
            Method::POST,
            &format!("{TICKETS}/{chat}/operator"),
            &token,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    assert_eq!(thread["ai"]["state"], "handed_off");
    h.db.cleanup().await;
}
