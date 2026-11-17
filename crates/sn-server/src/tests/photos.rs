use super::*;
use sn_store::NewPhoto;

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];

async fn raw(h: &Http, path: &str, token: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
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
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

#[tokio::test]
async fn фото_клиента_видит_админка_и_сам_клиент() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let alexey = h.client(5_093_118_422).await;
    let other = h.client(7_000_000_002).await;
    let list = format!("/api/admin/clients/photos?ids={},{},abc", alexey.0, other.0);
    let (status, body) = h.get(&list, &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({}));
    {
        let mut conn = h.services().store().conn().await.unwrap();
        assert_eq!(conn.client_photos().due(10).await.unwrap().len(), 2);
        conn.client_photos()
            .save(
                alexey,
                Some(NewPhoto {
                    version: "AQADmid",
                    content: JPEG,
                    content_type: "image/jpeg",
                }),
            )
            .await
            .unwrap();
    }
    let (status, body) = h.get(&list, &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_object().unwrap().len(), 1);
    assert_eq!(body[alexey.0.to_string()], "AQADmid");

    let photo = format!("/api/admin/clients/{}/photo", alexey.0);
    let (status, headers, bytes) = raw(&h, &format!("{photo}?v=AQADmid"), Some(&owner)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, JPEG);
    assert_eq!(headers[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "private, max-age=31536000, immutable"
    );
    let (status, headers, _) = raw(&h, &format!("{photo}?v=old"), Some(&owner)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "private, no-cache");
    let (status, headers, bytes) = raw(&h, &photo, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, JPEG);
    assert_eq!(headers[header::CACHE_CONTROL], "private, no-cache");
    assert_eq!(headers[header::ETAG], "\"AQADmid\"");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(
        headers[header::CONTENT_SECURITY_POLICY],
        "default-src 'none'"
    );
    let (status, _, _) = raw(
        &h,
        &format!("/api/admin/clients/{}/photo", other.0),
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = raw(&h, &photo, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mine = h.client_token(alexey).await;
    let (status, headers, bytes) = raw(&h, "/api/client/photo", Some(&mine)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, JPEG);
    assert_eq!(headers[header::CACHE_CONTROL], "private, max-age=3600");
    let theirs = h.client_token(other).await;
    let (status, _, _) = raw(&h, "/api/client/photo", Some(&theirs)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = raw(&h, "/api/client/photo", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    h.db.cleanup().await;
}
