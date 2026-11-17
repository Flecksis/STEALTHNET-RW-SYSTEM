use super::*;

fn providers_of(quote: &Value) -> Vec<String> {
    quote["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["provider"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn галочки_каналов_у_платёжной_системы_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/cryptopay",
            &owner,
            json!({
                "enabled": true,
                "config": {"api_token": "1234:AAA"},
                "channels": {"cabinet": false, "bot": true, "miniapp": true}
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["provider"]["channels"],
        json!({"cabinet": false, "bot": true, "miniapp": true})
    );
    let (status, list) = h.get("/api/admin/payments/providers", &owner).await;
    assert_eq!(status, StatusCode::OK);
    let crypto = list["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "cryptopay")
        .unwrap()
        .clone();
    assert_eq!(crypto["channels"]["cabinet"], false);
    let balance = list["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "balance")
        .unwrap()
        .clone();
    assert_eq!(
        balance["channels"],
        json!({"cabinet": true, "bot": true, "miniapp": true})
    );

    h.services().set_remna(Some(h.remna.clone()));
    let tariff = h.tariff().await;
    let client = h.client(5_093_118_422).await;
    let token = h.client_token(client).await;
    let cart = json!({
        "tariff_id": tariff.id.0,
        "period_id": tariff.periods[0].id.0,
        "extra_devices": 0
    });
    let (status, quote) = h
        .write(Method::POST, "/api/client/quote", &token, cart.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    assert!(!providers_of(&quote).contains(&"cryptopay".to_string()));
    let (status, refused) = h
        .write(
            Method::POST,
            "/api/client/checkout",
            &token,
            json!({"cart": cart, "provider": "cryptopay"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "checkout.method_unavailable");

    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/cryptopay",
            &owner,
            json!({"channels": {"cabinet": true, "bot": false, "miniapp": false}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, quote) = h
        .write(Method::POST, "/api/client/quote", &token, cart.clone())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(providers_of(&quote).contains(&"cryptopay".to_string()));
    let (status, stars) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/stars",
            &owner,
            json!({"channels": {"cabinet": false, "bot": true, "miniapp": true}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{stars}");
    assert_eq!(stars["provider"]["channels"]["cabinet"], false);
    let (status, refused) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/stars",
            &owner,
            json!({"enabled": true, "channels": {"cabinet": true, "bot": true, "miniapp": true}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/cryptopay",
            &support,
            json!({"channels": {"cabinet": false, "bot": false, "miniapp": false}}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    h.db.cleanup().await;
}
