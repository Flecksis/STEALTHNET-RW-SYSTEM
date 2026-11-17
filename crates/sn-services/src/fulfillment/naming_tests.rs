use super::*;
use crate::checkout::testkit::*;
use sn_domain::client::{Locale, NewIdentity, RegistrationSource, TelegramProfile};
use sn_domain::ids::RemnaUserId;
use sn_domain::settings::{MultiSubscriptions, keys};
use sn_domain::tariff::Tariff;
use sn_remnawave::fake::{Fault, sample_user};
use sn_store::NewClient;

pub(crate) async fn set_multi(kit: &Kit, enabled: bool) {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.settings()
        .set_as(
            keys::MULTI_SUBSCRIPTIONS,
            &MultiSubscriptions { enabled },
            None,
        )
        .await
        .unwrap();
    drop(conn);
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
}

async fn email_client(kit: &Kit, email: &str) -> ClientId {
    let mut conn = kit.services.store().conn().await.unwrap();
    let client = conn
        .clients()
        .create(&NewClient {
            locale: Locale::Ru,
            registration_source: RegistrationSource::Web,
            note: None,
        })
        .await
        .unwrap();
    conn.clients()
        .add_identity(
            client.id,
            &NewIdentity::new(IdentityKind::Email, email, true).unwrap(),
        )
        .await
        .unwrap();
    client.id
}

async fn bare_client(kit: &Kit) -> ClientId {
    let mut conn = kit.services.store().conn().await.unwrap();
    conn.clients()
        .create(&NewClient {
            locale: Locale::Ru,
            registration_source: RegistrationSource::Admin,
            note: None,
        })
        .await
        .unwrap()
        .id
}

async fn name_of(kit: &Kit, client_id: ClientId, tariff: &Tariff) -> String {
    let subscription = provision(kit, client_id, tariff, 30).await;
    let user = remna_user(kit, &subscription).await;
    assert_eq!(
        subscription.remna.as_ref().map(|l| l.username.as_str()),
        Some(user.username.as_str())
    );
    user.username
}

#[tokio::test]
async fn имя_по_нику_у_второй_и_третьей_подписок_номер() {
    let Some(kit) = kit().await else { return };
    set_multi(&kit, true).await;
    let client_id = client(&kit.services, 301).await;
    let tariff = standard(&kit.services).await;
    let mut names = Vec::new();
    for _ in 0..3 {
        names.push(name_of(&kit, client_id, &tariff).await);
    }
    assert_eq!(names, ["user301", "user301_2", "user301_3"]);
    assert_eq!(kit.remna.users().len(), 3);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn без_ника_имя_из_почты() {
    let Some(kit) = kit().await else { return };
    let client_id = email_client(&kit, "Ivan.Petrov@mail.ru").await;
    let tariff = standard(&kit.services).await;
    assert_eq!(name_of(&kit, client_id, &tariff).await, "ivan_petrov");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn без_ника_и_почты_имя_по_telegram_id_или_номеру() {
    let Some(kit) = kit().await else { return };
    let tariff = standard(&kit.services).await;
    let profile = TelegramProfile {
        id: 5_000_000_303,
        username: None,
        first_name: Some("Анна".into()),
        last_name: None,
        language_code: Some("ru".into()),
        is_premium: false,
    };
    let telegram_only = kit
        .services
        .clients()
        .ensure_telegram(&profile, RegistrationSource::Bot, None)
        .await
        .unwrap()
        .0
        .id;
    assert_eq!(name_of(&kit, telegram_only, &tariff).await, "tg5000000303");
    let nobody = bare_client(&kit).await;
    assert_eq!(
        name_of(&kit, nobody, &tariff).await,
        format!("id{}", nobody.0)
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn имя_занятое_чужим_в_remnawave_заменяется_следующим() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 304).await;
    let tariff = standard(&kit.services).await;
    let mut stranger = sample_user(500, "user304");
    stranger.description = Some("чужой".into());
    kit.remna.insert_user(stranger);
    let mut old = sample_user(501, "user304_2");
    old.description = None;
    kit.remna.insert_user(old);
    assert_eq!(name_of(&kit, client_id, &tariff).await, "user304_3");
    let stranger = kit.remna.user(RemnaUserId(500)).unwrap();
    assert_eq!(stranger.description.as_deref(), Some("чужой"));
    assert_eq!(kit.remna.users().len(), 3);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn имя_занятое_другим_клиентом_в_базе_получает_номер() {
    let Some(kit) = kit().await else { return };
    let tariff = standard(&kit.services).await;
    let first = email_client(&kit, "ivan@mail.ru").await;
    let second = email_client(&kit, "ivan@gmail.com").await;
    assert_eq!(name_of(&kit, first, &tariff).await, "ivan");
    assert_eq!(name_of(&kit, second, &tariff).await, "ivan_2");
    kit.db.cleanup().await;
}

#[tokio::test]
async fn имя_занятое_во_время_выдачи_меняется_и_выдача_доходит() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 306).await;
    let tariff = standard(&kit.services).await;
    credit(&kit.services, client_id, 1_000).await;
    let order = buy_with_balance(&kit.services, client_id, cart(&tariff, 30, None)).await;
    let fulfillment = kit.services.fulfillment();
    kit.remna.fail_on("create_user", Fault::ServerError);
    let outcome = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Retry { .. }),
        "{outcome:?}"
    );
    let saved = self::order(&kit.services, order.id).await.plan.unwrap();
    assert!(
        matches!(&saved.action, PlanAction::Create { username, .. } if username == "user306"),
        "{saved:?}"
    );
    let mut stranger = sample_user(700, "user306");
    stranger.description = Some("чужой".into());
    kit.remna.insert_user(stranger);
    let outcome = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(&outcome, FulfillmentOutcome::Retry { reason, .. } if reason.contains("user306_2")),
        "{outcome:?}"
    );
    assert_eq!(self::order(&kit.services, order.id).await.plan, None);
    let outcome = fulfillment.run(order.id).await.unwrap();
    assert!(
        matches!(outcome, FulfillmentOutcome::Done { .. }),
        "{outcome:?}"
    );
    let order = self::order(&kit.services, order.id).await;
    let subscription = subscription(&kit.services, &order).await;
    let user = remna_user(&kit, &subscription).await;
    assert_eq!(user.username, "user306_2");
    assert_eq!(
        kit.remna
            .user(RemnaUserId(700))
            .unwrap()
            .description
            .as_deref(),
        Some("чужой")
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn уже_выданные_подписки_не_переименовываются() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 307).await;
    let tariff = standard(&kit.services).await;
    let subscription = provision(&kit, client_id, &tariff, 30).await;
    let legacy = remna_username(subscription.public_id);
    let relinked = set_user(&kit, &subscription, |user| user.username = legacy.clone()).await;
    let link = relinked.remna.clone().unwrap();
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.subscriptions()
            .link_remna(
                subscription.id,
                &RemnaLink {
                    username: legacy.clone(),
                    ..link
                },
            )
            .await
            .unwrap();
    }
    let renewed = provision(&kit, client_id, &tariff, 30).await;
    assert_eq!(renewed.id, subscription.id);
    assert_eq!(
        remna_user(&kit, &renewed).await.username,
        legacy,
        "продление не трогает имя"
    );
    assert_eq!(renewed.remna.map(|l| l.username), Some(legacy));
    kit.db.cleanup().await;
}
