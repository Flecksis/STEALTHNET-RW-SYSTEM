use super::*;
use crate::notify::NotifyHandler;
use sn_domain::ids::TicketId;
use sn_domain::support::FileKind;
use sn_services::Actor;
use sn_services::support::ReplyRequest;
use wiremock::matchers::body_partial_json;

fn jpeg_with_gps() -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    let segment = |marker: u8, payload: &[u8]| {
        let mut s = vec![0xFF, marker];
        s.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        s.extend_from_slice(payload);
        s
    };
    out.extend(segment(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
    out.extend(segment(
        0xE1,
        b"Exif\0\0MM\0\x2a\0\0\0\x08\0\0\0\0\0\0GPSLatitude 59.93",
    ));
    out.extend(segment(0xC0, &[8, 0, 10, 0, 20, 1, 1, 0x11, 0]));
    out.extend(segment(0xDA, &[1, 1, 0, 0, 0x3F, 0]));
    out.extend_from_slice(&[0x12, 0x34, 0xFF, 0xD9]);
    out
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x10\0\0\0\x08\x08\x06\0\0\0\0\0\0\0\0\0\0\0IEND\0\0\0\0";

fn file_update(
    update_id: i64,
    from: i64,
    file: serde_json::Value,
    caption: Option<&str>,
) -> Update {
    let mut message = serde_json::json!({
        "message_id": update_id, "from": user(from),
        "chat": {"id": from, "type": "private"}, "date": 1
    });
    for (key, value) in file.as_object().unwrap() {
        message[key] = value.clone();
    }
    if let Some(caption) = caption {
        message["caption"] = serde_json::json!(caption);
    }
    serde_json::from_value(serde_json::json!({ "update_id": update_id, "message": message }))
        .unwrap()
}

async fn mount_file(server: &MockServer, file_id: &str, path: &str, bytes: Vec<u8>) {
    Mock::given(method("POST"))
        .and(path_regex(r"/getFile$"))
        .and(body_partial_json(serde_json::json!({ "file_id": file_id })))
        .respond_with(ok(serde_json::json!({
            "file_id": file_id, "file_unique_id": format!("u-{file_id}"),
            "file_size": bytes.len(), "file_path": path
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!(
            r"/file/bot[^/]+/{}$",
            regex_escape(path)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(server)
        .await;
}

fn regex_escape(value: &str) -> String {
    value.replace('.', r"\.")
}

async fn calls(server: &MockServer, method: &str) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with(&format!("/{method}")))
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .collect()
}

async fn ticket_of(services: &Services, telegram_id: i64) -> TicketId {
    let client = client_of(services, telegram_id).await.unwrap();
    services
        .support()
        .telegram_ticket(client.id, None)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn фото_и_файл_из_телеграма_попадают_в_обращение() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    mount_file(&server, "photo-big", "photos/file_1.jpg", jpeg_with_gps()).await;
    mount_file(
        &server,
        "log-1",
        "documents/file_2.log",
        b"12:00 connect failed\n".to_vec(),
    )
    .await;
    mount_file(
        &server,
        "svg-1",
        "documents/file_3.svg",
        b"<svg onload=alert(1)/>".to_vec(),
    )
    .await;
    handle_update(&ctx, text_update(1, 731, "/start")).await;
    handle_update(&ctx, callback_update(2, 731, "sup")).await;
    handle_update(
        &ctx,
        file_update(
            3,
            731,
            serde_json::json!({ "photo": [
                {"file_id": "photo-small", "file_unique_id": "s", "width": 90, "height": 45, "file_size": 900},
                {"file_id": "photo-big", "file_unique_id": "b", "width": 1280, "height": 640, "file_size": 64000}
            ]}),
            Some("Вот что показывает приложение"),
        ),
    )
    .await;
    assert!(
        last_text(&server).await.contains("отправлено"),
        "{}",
        last_text(&server).await
    );
    let ticket = ticket_of(&services, 731).await;
    handle_update(
        &ctx,
        file_update(
            4,
            731,
            serde_json::json!({ "document": {
                "file_id": "log-1", "file_unique_id": "l", "file_name": "app.log",
                "mime_type": "text/plain", "file_size": 21
            }}),
            None,
        ),
    )
    .await;
    assert!(last_text(&server).await.contains("добавлено"));
    handle_update(
        &ctx,
        file_update(
            5,
            731,
            serde_json::json!({ "document": {
                "file_id": "svg-1", "file_unique_id": "v", "file_name": "evil.svg",
                "mime_type": "image/svg+xml", "file_size": 22
            }}),
            None,
        ),
    )
    .await;
    assert!(last_text(&server).await.contains("Такой файл не примем"));
    handle_update(
        &ctx,
        file_update(
            6,
            731,
            serde_json::json!({ "document": {
                "file_id": "huge-1", "file_unique_id": "h", "file_name": "video.pdf",
                "mime_type": "application/pdf", "file_size": 30 * 1024 * 1024
            }}),
            None,
        ),
    )
    .await;
    assert!(
        last_text(&server).await.contains("больше 10 МБ"),
        "{}",
        last_text(&server).await
    );
    let downloads = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().contains("/file/bot"))
        .count();
    assert_eq!(downloads, 2);
    assert!(
        calls(&server, "getFile")
            .await
            .iter()
            .all(|b| !b.contains("huge-1"))
    );

    let client = client_of(&services, 731).await.unwrap();
    let thread = services
        .support()
        .thread(ticket, Actor::Client(client.id))
        .await
        .unwrap();
    assert_eq!(thread.messages.len(), 2);
    assert_eq!(thread.messages[0].body, "Вот что показывает приложение");
    let photo = &thread.messages[0].attachments[0];
    assert_eq!(
        (photo.kind, photo.mime.as_str()),
        (FileKind::Image, "image/jpeg")
    );
    assert_eq!((photo.width, photo.height), (Some(20), Some(10)));
    assert_eq!(thread.messages[1].body, "");
    assert_eq!(thread.messages[1].attachments[0].name, "app.log");
    assert_eq!(thread.messages[1].text_with_files(), "[вложение: app.log]");
    let stored = services
        .support()
        .file(photo.id, Actor::Client(client.id))
        .await
        .unwrap();
    assert!(!stored.bytes.windows(11).any(|w| w == b"GPSLatitude"));
    db.cleanup().await;
}

#[tokio::test]
async fn с_выключенным_модулем_файлы_в_боте_не_принимаются() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    services
        .modules()
        .set_enabled(
            services.store(),
            sn_domain::module::ModuleId::TicketFiles,
            false,
            None,
        )
        .await
        .unwrap();
    mount_file(&server, "photo-1", "photos/file_9.jpg", jpeg_with_gps()).await;
    handle_update(&ctx, text_update(1, 732, "/start")).await;
    handle_update(&ctx, callback_update(2, 732, "sup")).await;
    handle_update(
        &ctx,
        file_update(
            3,
            732,
            serde_json::json!({ "photo": [{"file_id": "photo-1", "file_unique_id": "p", "width": 10, "height": 10}] }),
            None,
        ),
    )
    .await;
    assert!(last_text(&server).await.contains("не принимаются"));
    assert!(calls(&server, "getFile").await.is_empty());
    db.cleanup().await;
}

async fn admin(services: &Services) -> sn_domain::ids::AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

async fn last_notify(services: &Services) -> JobPayload {
    let mut conn = services.store().conn().await.unwrap();
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE kind = 'notify' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    serde_json::from_value(payload).unwrap()
}

async fn mount_uploads(server: &MockServer) {
    for name in ["sendPhoto", "sendDocument"] {
        Mock::given(method("POST"))
            .and(path_regex(format!("/{name}$")))
            .respond_with(ok(serde_json::json!({
                "message_id": 101, "chat": {"id": 1, "type": "private"}, "date": 0
            })))
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn ответ_поддержки_с_вложениями_доходит_в_бот() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    mount_uploads(&server).await;
    handle_update(&ctx, text_update(1, 733, "/start")).await;
    handle_update(&ctx, callback_update(2, 733, "sup")).await;
    handle_update(&ctx, text_update(3, 733, "Не работает")).await;
    let ticket = ticket_of(&services, 733).await;
    let client = client_of(&services, 733).await.unwrap();
    let admin = admin(&services).await;
    let support = services.support();
    let png = support
        .upload(Actor::Admin(admin), "Инструкция.png", PNG.to_vec())
        .await
        .unwrap();
    let pdf = support
        .upload(
            Actor::Admin(admin),
            "чек.pdf",
            b"%PDF-1.7\n1 0 obj".to_vec(),
        )
        .await
        .unwrap();
    support
        .reply_with_files(
            ticket,
            &ReplyRequest {
                body: "Сделайте как на картинке".into(),
                close: false,
            },
            &[png.id, pdf.id],
            Actor::Admin(admin),
        )
        .await
        .unwrap();
    let handler = NotifyHandler::new(&BotConfig {
        api_base: server.uri(),
        ..BotConfig::default()
    });
    let payload = last_notify(&services).await;
    assert_eq!(
        handler.handle(&services, &payload, 1).await,
        JobResult::Done
    );
    let photos = calls(&server, "sendPhoto").await;
    let documents = calls(&server, "sendDocument").await;
    assert_eq!((photos.len(), documents.len()), (1, 1));
    assert!(photos[0].contains("name=\"photo\""));
    assert!(!photos[0].contains("name=\"caption\""));
    assert!(documents[0].contains("name=\"document\""));
    assert!(documents[0].contains("application/pdf"));
    assert!(
        last_text(&server)
            .await
            .contains("Сделайте как на картинке")
    );
    assert_eq!(
        last_buttons(&server).await,
        vec![format!("sup:{}", ticket.0)]
    );
    assert_eq!(
        handler.handle(&services, &payload, 2).await,
        JobResult::Done
    );
    assert_eq!(calls(&server, "sendPhoto").await.len(), 1);
    assert_eq!(calls(&server, "sendDocument").await.len(), 1);

    let single = support
        .upload(Actor::Admin(admin), "схема.png", PNG.to_vec())
        .await
        .unwrap();
    support
        .reply_with_files(
            ticket,
            &ReplyRequest {
                body: String::new(),
                close: false,
            },
            &[single.id],
            Actor::Admin(admin),
        )
        .await
        .unwrap();
    let messages_before = sent(&server).await.len();
    let payload = last_notify(&services).await;
    assert_eq!(
        handler.handle(&services, &payload, 1).await,
        JobResult::Done
    );
    let photos = calls(&server, "sendPhoto").await;
    assert_eq!(photos.len(), 2);
    assert!(photos[1].contains("name=\"caption\""));
    assert!(photos[1].contains("Ответ поддержки"));
    assert!(photos[1].contains(&format!("sup:{}", ticket.0)));
    assert_eq!(sent(&server).await.len(), messages_before);
    let thread = support
        .thread(ticket, Actor::Client(client.id))
        .await
        .unwrap();
    let last = thread.messages.last().unwrap();
    assert_eq!(last.body, "");
    assert_eq!(last.attachments[0].name, "схема.png");
    db.cleanup().await;
}
