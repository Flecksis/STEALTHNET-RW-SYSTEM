use super::*;

#[tokio::test]
async fn настройка_уведомлений_в_группу_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, view) = h.get("/api/admin/alerts", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["kinds"].as_array().map(Vec::len), Some(6));
    assert_eq!(view["chat_id"], Value::Null);
    let (status, body) = h
        .write(Method::POST, "/api/admin/alerts/link", &owner, json!({}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "alerts.no_bot");
    h.set_bot().await;
    let (status, body) = h
        .write(Method::POST, "/api/admin/alerts/link", &owner, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["link"]
            .as_str()
            .unwrap()
            .starts_with("https://t.me/stealth_vpn_bot?startgroup=alerts_")
    );
    let (status, body) = h
        .write(Method::POST, "/api/admin/alerts/test", &owner, json!({}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "alerts.no_group");
    let (status, view) = h
        .write(
            Method::PUT,
            "/api/admin/alerts",
            &owner,
            json!({ "routes": [
                { "kind": "orders", "on": true, "topic": "Деньги" },
                { "kind": "stars", "on": false, "topic": "Деньги" }
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let orders = view["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "orders")
        .unwrap();
    assert_eq!(orders["topic"], "Деньги");
    let stars = view["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "stars")
        .unwrap();
    assert_eq!(stars["on"], false);
    let (status, _) = h
        .write(Method::DELETE, "/api/admin/alerts/link", &owner, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    h.db.cleanup().await;
}
