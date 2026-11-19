use super::*;
use sn_domain::subscription::{AutoRenewState, SubscriptionKind};
use sn_remnawave::fake::sample_user;
use sn_store::NewSubscription;

#[tokio::test]
async fn привязать_к_пользователю_remnawave_можно_только_из_сессии_с_правом_изменений() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let viewer = h.admin("viewer@example.com", AdminRole::ReadOnly).await;
    let client = h.client(5_093_118_802).await;
    let subscription = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.subscriptions()
            .create(&NewSubscription {
                client_id: client,
                tariff_id: None,
                kind: SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: AutoRenewState::default(),
            })
            .await
            .unwrap()
    };
    let mut user = sample_user(701, "morozov_vpn");
    user.telegram_id = Some(5_093_118_802);
    h.remna.insert_user(user);
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/api-keys",
            &owner,
            json!({ "name": "Скрипт", "permissions": ["clients_read", "clients_write"], "expires_in_days": 30, "password": PASSWORD }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let key = created["token"].as_str().unwrap().to_string();
    let search = format!(
        "/api/admin/subscriptions/{}/remna/search",
        subscription.id.0
    );
    let link = format!("/api/admin/subscriptions/{}/remna/link", subscription.id.0);
    let query = json!({ "q": "5093118802" });
    let pick = json!({ "user_id": 701, "username": "morozov_vpn" });

    for token in [&key, &viewer] {
        assert_eq!(
            h.write(Method::POST, &search, token, query.clone()).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            h.write(Method::POST, &link, token, pick.clone()).await.0,
            StatusCode::FORBIDDEN
        );
    }

    let (status, found) = h.write(Method::POST, &search, &support, query).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["state"], "unlinked");
    assert_eq!(found["users"][0]["id"], 701);
    assert_eq!(found["users"][0]["found_by"], json!(["telegram"]));
    assert!(found["users"][0]["holder"].is_null());

    let (status, wrong) = h
        .write(
            Method::POST,
            &link,
            &owner,
            json!({ "user_id": 701, "username": "someone_else" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(wrong["error"]["code"], "remna_link.user_changed");

    let (status, linked) = h.write(Method::POST, &link, &owner, pick).await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    assert_eq!(linked["username"], "morozov_vpn");
    assert_eq!(linked["refreshed"], true);
    let (status, view) = h
        .get(
            &format!("/api/admin/subscriptions/{}", subscription.id.0),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["subscription"]["remna"]["user_id"], 701);
    assert!(view["remna_missing_at"].is_null());
    assert_eq!(view["subscription_url"], "https://sub.example.com/short701");
    h.db.cleanup().await;
}

#[tokio::test]
async fn добавить_подписку_из_remnawave_можно_только_из_сессии_с_правом_изменений() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let viewer = h.admin("viewer@example.com", AdminRole::ReadOnly).await;
    let client = h.client(5_093_118_803).await;
    let mut user = sample_user(702, "petrov_vpn");
    user.telegram_id = Some(5_093_118_803);
    h.remna.insert_user(user);
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/api-keys",
            &owner,
            json!({ "name": "Скрипт", "permissions": ["clients_read", "clients_write"], "expires_in_days": 30, "password": PASSWORD }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let key = created["token"].as_str().unwrap().to_string();
    let search = format!("/api/admin/clients/{}/remna/search", client.0);
    let add = format!("/api/admin/clients/{}/remna/add", client.0);
    let query = json!({ "q": "petrov_vpn" });
    let pick = json!({ "user_id": 702, "username": "petrov_vpn" });
    for token in [&key, &viewer] {
        assert_eq!(
            h.write(Method::POST, &search, token, query.clone()).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            h.write(Method::POST, &add, token, pick.clone()).await.0,
            StatusCode::FORBIDDEN
        );
    }
    let (status, found) = h.write(Method::POST, &search, &owner, query).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["users"][0]["id"], 702);
    assert_eq!(found["users"][0]["tariff"]["kind"], "none");
    let (status, added) = h.write(Method::POST, &add, &owner, pick.clone()).await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert!(added["tariff"].is_null());
    let (status, card) = h
        .get(&format!("/api/admin/clients/{}", client.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{card}");
    assert_eq!(card["multi_subscriptions"], false);
    assert_eq!(card["subscriptions"][0]["remna"]["user_id"], 702);
    let (status, again) = h.write(Method::POST, &add, &owner, pick).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(again["error"]["code"], "remna_link.single");
    h.db.cleanup().await;
}
