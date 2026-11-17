use super::*;
use crate::photos::{PhotoKeeper, sniff};
use wiremock::matchers::body_partial_json;

const JPEG: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01,
];

async fn mount_photos(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path_regex(r"/getUserProfilePhotos$"))
        .and(body_partial_json(serde_json::json!({ "user_id": 520 })))
        .respond_with(ok(serde_json::json!({
            "total_count": 1,
            "photos": [[
                {"file_id": "small", "file_unique_id": "AQADsmall", "width": 160, "height": 160, "file_size": 5000},
                {"file_id": "mid", "file_unique_id": "AQADmid", "width": 320, "height": 320, "file_size": 12000},
                {"file_id": "big", "file_unique_id": "AQADbig", "width": 640, "height": 640, "file_size": 40000}
            ]]
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/getUserProfilePhotos$"))
        .and(body_partial_json(serde_json::json!({ "user_id": 521 })))
        .respond_with(ok(serde_json::json!({ "total_count": 0, "photos": [] })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/getUserProfilePhotos$"))
        .and(body_partial_json(serde_json::json!({ "user_id": 522 })))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "ok": false, "error_code": 429, "description": "Too Many Requests: retry after 40",
            "parameters": {"retry_after": 40}
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/getFile$"))
        .and(body_partial_json(serde_json::json!({ "file_id": "mid" })))
        .respond_with(ok(serde_json::json!({
            "file_id": "mid", "file_unique_id": "AQADmid", "file_size": 12000,
            "file_path": "photos/file_7.jpg"
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/file/bot[^/]+/photos/file_7\.jpg$"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(JPEG.to_vec()))
        .mount(server)
        .await;
}

async fn downloads(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().contains("/file/bot"))
        .count()
}

#[tokio::test]
async fn бот_сохраняет_фото_профиля_и_не_качает_его_повторно() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    mount_photos(&server).await;
    for (update, telegram) in [(1, 520), (2, 521), (3, 522)] {
        handle_update(&ctx, text_update(update, telegram, "/start")).await;
    }
    let with_photo = client_of(&services, 520).await.unwrap();
    let without = client_of(&services, 521).await.unwrap();
    let limited = client_of(&services, 522).await.unwrap();
    let keeper = PhotoKeeper::new(services.clone(), &server.uri(), Duration::from_millis(50));
    assert_eq!(keeper.round(TOKEN).await.unwrap(), 3);
    {
        let mut conn = db.store.conn().await.unwrap();
        let mut photos = conn.client_photos();
        let stored = photos.get(with_photo.id).await.unwrap().unwrap();
        assert_eq!(stored.content, JPEG);
        assert_eq!(stored.content_type, "image/jpeg");
        assert_eq!(stored.version, "AQADmid");
        assert!(photos.get(without.id).await.unwrap().is_none());
        assert!(photos.get(limited.id).await.unwrap().is_none());
        assert!(photos.due(10).await.unwrap().is_empty());
        assert!(
            photos
                .request_if_stale(with_photo.id, Duration::ZERO)
                .await
                .unwrap()
        );
    }
    assert_eq!(downloads(&server).await, 1);
    assert_eq!(keeper.round(TOKEN).await.unwrap(), 1);
    assert_eq!(downloads(&server).await, 1);
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.client_photos()
            .get(with_photo.id)
            .await
            .unwrap()
            .unwrap()
            .version,
        "AQADmid"
    );
    drop(conn);
    handle_update(&ctx, text_update(4, 520, "/start")).await;
    assert_eq!(keeper.round(TOKEN).await.unwrap(), 0);
    db.cleanup().await;
}

#[tokio::test]
async fn без_токена_или_при_отказе_telegram_фото_не_портятся() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 530, "/start")).await;
    let client = client_of(&services, 530).await.unwrap();
    let denied = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "ok": false, "error_code": 401, "description": "Unauthorized"
        })))
        .mount(&denied)
        .await;
    let keeper = PhotoKeeper::new(services.clone(), &denied.uri(), Duration::from_millis(50));
    assert_eq!(keeper.round(TOKEN).await.unwrap(), 0);
    let mut conn = db.store.conn().await.unwrap();
    let due = conn.client_photos().due(10).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].client_id, client.id);
    drop(conn);
    let _ = server;
    db.cleanup().await;
}

#[test]
fn картинка_узнаётся_по_содержимому() {
    assert_eq!(sniff(JPEG), Some("image/jpeg"));
    assert_eq!(
        sniff(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
        Some("image/png")
    );
    assert_eq!(sniff(b"RIFF\x10\x00\x00\x00WEBPVP8 "), Some("image/webp"));
    assert_eq!(sniff(b"<html><script>alert(1)</script>"), None);
    assert_eq!(sniff(b"GIF89a"), None);
}
