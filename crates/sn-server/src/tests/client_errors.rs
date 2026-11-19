use super::*;
use sn_domain::ids::RemnaUserId;
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_remnawave::fake::{Fault, sample_user};
use sn_store::NewSubscription;

#[tokio::test]
async fn клиент_не_видит_подробностей_ошибки_remnawave() {
    let Some(h) = http().await else { return };
    let client = h.client(5_093_118_771).await;
    let token = h.client_token(client).await;
    let user = sample_user(771, "morozov_a");
    h.remna.insert_user(user.clone());
    let subscription = {
        let mut conn = h.services().store().conn().await.unwrap();
        let subscription = conn
            .subscriptions()
            .create(&NewSubscription {
                client_id: client,
                tariff_id: None,
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
                    user_id: RemnaUserId(771),
                    username: "morozov_a".into(),
                    short_uuid: Some(user.short_uuid.clone()),
                },
            )
            .await
            .unwrap();
        conn.subscriptions()
            .apply_mirror(
                subscription.id,
                &RemnaMirror {
                    status: Some(SubscriptionStatus::Active),
                    expire_at: Some(user.expire_at),
                    ..RemnaMirror::default()
                },
            )
            .await
            .unwrap();
        subscription.id
    };
    let path = format!("/api/client/subscriptions/{}/devices", subscription.0);
    let (status, body) = h.get(&path, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for fault in [Fault::Redirected, Fault::ServerError] {
        h.remna.fail_on("user_devices", fault);
        let (status, body) = h.get(&path, &token).await;
        assert!(status.is_server_error(), "{status} {body}");
        let text = body.to_string();
        assert!(!text.contains("panel.internal.example"), "{text}");
        assert!(!text.contains("fake"), "{text}");
        assert_eq!(
            body["error"]["message"],
            "Сервис VPN сейчас недоступен. Попробуйте через несколько минут.",
            "{text}"
        );
    }
    h.db.cleanup().await;
}
