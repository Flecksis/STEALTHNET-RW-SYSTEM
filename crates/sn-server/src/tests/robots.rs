use super::*;

#[tokio::test]
async fn robots_txt_отдаётся_текстом_и_пускает_сборщики_превью() {
    let Some(h) = http().await else { return };
    let response = router(h.state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/robots.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("User-agent: TelegramBot"), "{text}");
    assert!(text.contains("Disallow: /admin"), "{text}");
    h.db.cleanup().await;
}
