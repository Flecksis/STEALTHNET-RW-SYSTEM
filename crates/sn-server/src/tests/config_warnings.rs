use super::*;

#[tokio::test]
async fn доверие_всем_адресам_видно_в_админке_тем_кто_правит_настройки() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, body) = h.get("/api/admin/system/warnings", &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["warnings"], json!([]));

    let config = ServerConfig {
        trusted_proxies: ["0.0.0.0/0", "10.0.0.2", "::/0"]
            .iter()
            .map(|net| net.parse().unwrap())
            .collect(),
        ..ServerConfig::default()
    };
    let h = Http {
        state: AppState::new(config, h.services().clone()),
        ..h
    };
    let (status, body) = h.get("/api/admin/system/warnings", &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["warnings"],
        json!([{ "kind": "trusted_proxies_everyone", "networks": ["0.0.0.0/0", "::/0"] }])
    );
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h.get("/api/admin/system/warnings", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    h.db.cleanup().await;
}
