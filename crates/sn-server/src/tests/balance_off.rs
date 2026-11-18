use super::*;

#[tokio::test]
async fn выключенный_баланс_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let client = h.client(5_093_118_777).await;
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/admin/clients/{}/balance", client.0),
            &owner,
            json!({ "delta": { "minor": 300, "currency": "USD" }, "comment": "остаток", "idempotency_key": "b1" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/balance",
            &owner,
            json!({"enabled": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"]["enabled"], false);
    let (status, payments) = h.get("/api/admin/settings/payments", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payments["balances"]["clients"], 1);
    assert_eq!(payments["balances"]["total"]["minor"], 300);
    let (status, list) = h.get("/api/admin/payments/providers", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["balances"]["clients"], 1);

    h.services().set_remna(Some(h.remna.clone()));
    let tariff = h.tariff().await;
    let token = h.client_token(client).await;
    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["balance_enabled"], false);
    assert_eq!(me["autorenew_card"], false);
    assert_eq!(me["balance"]["minor"], 300);
    assert_eq!(me["balance_held"], json!({ "minor": 0, "currency": "USD" }));
    for amount in ["10", "0.01"] {
        let (status, refused) = h
            .write(
                Method::POST,
                "/api/client/topup",
                &token,
                json!({"amount": amount, "provider": "cryptopay"}),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
        assert_eq!(refused["error"]["code"], "balance.disabled");
    }
    let cart = json!({
        "tariff_id": tariff.id.0,
        "period_id": tariff.periods[0].id.0,
        "extra_devices": 0
    });
    let (status, quote) = h
        .write(Method::POST, "/api/client/quote", &token, cart.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    assert!(
        quote["methods"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["provider"] != "balance")
    );
    let (status, refused) = h
        .write(
            Method::POST,
            "/api/client/checkout",
            &token,
            json!({"cart": cart, "provider": "balance"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "balance.disabled");

    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/balance",
            &owner,
            json!({"enabled": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, me) = h.get("/api/client/me", &token).await;
    assert_eq!(me["balance_enabled"], true);
    let (_, refused) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &token,
            json!({"amount": "0.01", "provider": "cryptopay"}),
        )
        .await;
    assert_eq!(refused["error"]["code"], "balance.topup_out_of_range");
    h.db.cleanup().await;
}
