use super::*;
use sn_domain::ids::RemnaUserId;
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_store::NewSubscription;

#[tokio::test]
async fn мультиподписки_выключены_по_умолчанию_и_меняются_в_настройках() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let readonly = h.admin("viewer@example.com", AdminRole::Support).await;
    let (status, body) = h
        .get("/api/admin/settings/multi_subscriptions", &owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "enabled": false }));
    assert_eq!(
        h.get("/api/admin/settings/multi_subscriptions", &readonly)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    let tariff = h.tariff().await;
    let client = h.client(5_093_118_801).await;
    let subscription = {
        let mut conn = h.services().store().conn().await.unwrap();
        let subscription = conn
            .subscriptions()
            .create(&NewSubscription {
                client_id: client,
                tariff_id: Some(tariff.id),
                kind: SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: AutoRenewState::default(),
            })
            .await
            .unwrap();
        conn.subscriptions()
            .link_remna(
                subscription.id,
                &RemnaLink {
                    user_id: RemnaUserId(91),
                    username: "morozov_a".into(),
                    short_uuid: None,
                },
            )
            .await
            .unwrap();
        conn.subscriptions()
            .apply_mirror(
                subscription.id,
                &RemnaMirror {
                    status: Some(SubscriptionStatus::Active),
                    expire_at: Some(start() + Duration::days(20)),
                    ..RemnaMirror::default()
                },
            )
            .await
            .unwrap();
        subscription
    };
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
    assert_eq!(quote["kind"], "renewal");
    assert_eq!(quote["current"]["subscription_id"], subscription.id.0);
    assert_eq!(quote["current"]["tariff"], "Стандарт");
    assert_eq!(quote["current"]["trial"], false);

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/multi_subscriptions",
            &owner,
            json!({ "enabled": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "enabled": true }));
    assert!(h.services().settings().multi_subscriptions);
    let (status, quote) = h
        .write(Method::POST, "/api/client/quote", &token, cart)
        .await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    assert_eq!(quote["kind"], "new");
    assert_eq!(quote["current"], Value::Null);
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/multi_subscriptions",
            &readonly,
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(h.services().settings().multi_subscriptions);
    h.db.cleanup().await;
}
