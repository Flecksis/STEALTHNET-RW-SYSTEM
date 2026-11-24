use super::*;
use sn_domain::ids::SquadId;
use sn_domain::order::{Order, OrderKind};
use sn_domain::settings::{TrialSettings, TrialVariant, keys};
use sn_domain::traffic::TrafficReset;
use sn_remnawave::fake::FakeRemnawave;
use sn_remnawave::{InternalSquad, SquadInfo};

fn remnawave() -> Arc<FakeRemnawave> {
    let remna = Arc::new(FakeRemnawave::new());
    remna.set_squads(
        [1u128, 2]
            .into_iter()
            .map(|n| InternalSquad {
                uuid: uuid::Uuid::from_u128(n),
                name: format!("Сквад {n}"),
                view_position: 0,
                info: SquadInfo::default(),
                inbounds: Vec::new(),
            })
            .collect(),
    );
    remna
}

fn variant(id: u32, name: &str, days: u32, devices: u32, squad: u128) -> TrialVariant {
    TrialVariant {
        id,
        name: name.into(),
        enabled: true,
        days,
        squads: vec![SquadId(uuid::Uuid::from_u128(squad))],
        traffic: TrafficPolicy::UNLIMITED,
        devices,
    }
}

async fn set_trial(services: &Services, variants: Vec<TrialVariant>) {
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set_as(
                keys::TRIAL,
                &TrialSettings {
                    enabled: true,
                    variants,
                },
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    services.set_remna(Some(remnawave()));
}

async fn trial_orders(services: &Services, telegram_id: i64) -> Vec<Order> {
    let client = client_of(services, telegram_id).await.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    conn.orders()
        .for_client(
            client.id,
            sn_domain::paging::PageRequest::new(Some(20), None),
        )
        .await
        .unwrap()
        .items
        .into_iter()
        .filter(|o| o.kind == OrderKind::Trial)
        .collect()
}

async fn two_variants(services: &Services) {
    let mut europe = variant(2, "Европа", 1, 2, 2);
    europe.traffic = TrafficPolicy {
        limit_bytes: Some(10 * 1024 * 1024 * 1024),
        reset: TrafficReset::NoReset,
    };
    set_trial(services, vec![variant(1, "Россия", 3, 1, 1), europe]).await;
}

#[tokio::test]
async fn в_боте_клиент_выбирает_вариант_пробного_периода() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    two_variants(&services).await;
    handle_update(&ctx, text_update(1, 771, "/start")).await;
    assert!(last_buttons(&server).await.contains(&"trial".to_string()));
    handle_update(&ctx, callback_update(2, 771, "trial")).await;
    let choice = last_text(&server).await;
    assert!(choice.contains("Выберите пробный период"), "{choice}");
    assert!(
        choice.contains("Россия — 3\u{a0}дня, устройств: 1, трафик без лимита"),
        "{choice}"
    );
    assert!(
        choice.contains("Европа — 1\u{a0}день, устройств: 2, трафик: 10"),
        "{choice}"
    );
    let buttons = last_buttons(&server).await;
    assert!(buttons.contains(&"trial:1".to_string()), "{buttons:?}");
    assert!(buttons.contains(&"trial:2".to_string()), "{buttons:?}");
    let labels = last_labels(&server).await;
    assert!(
        labels.contains(&"Европа · 1\u{a0}день".to_string()),
        "{labels:?}"
    );
    assert!(trial_orders(&services, 771).await.is_empty());
    handle_update(&ctx, callback_update(3, 771, "trial:2")).await;
    let started = last_text(&server).await;
    assert!(started.contains("Пробный период оформляется"), "{started}");
    let orders = trial_orders(&services, 771).await;
    assert_eq!(orders.len(), 1);
    let snapshot = orders[0].snapshot.as_ref().unwrap();
    assert_eq!((snapshot.name.as_str(), snapshot.days), ("Европа", 1));
    handle_update(&ctx, callback_update(4, 771, "trial:1")).await;
    assert!(
        last_text(&server)
            .await
            .contains("Пробный период уже был использован")
    );
    assert_eq!(trial_orders(&services, 771).await.len(), 1);
    handle_update(&ctx, text_update(5, 772, "/start")).await;
    handle_update(&ctx, callback_update(6, 772, "trial:9")).await;
    let gone = last_text(&server).await;
    assert!(
        gone.contains("Этот вариант пробного периода сейчас недоступен"),
        "{gone}"
    );
    assert!(gone.contains("Выберите пробный период"), "{gone}");
    assert!(trial_orders(&services, 772).await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn один_вариант_в_боте_включается_сразу_без_выбора() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let mut off = variant(5, "Европа", 1, 1, 2);
    off.enabled = false;
    set_trial(&services, vec![variant(4, "", 3, 1, 1), off]).await;
    handle_update(&ctx, text_update(1, 781, "/start")).await;
    handle_update(&ctx, callback_update(2, 781, "trial")).await;
    let started = last_text(&server).await;
    assert!(started.contains("Пробный период оформляется"), "{started}");
    let orders = trial_orders(&services, 781).await;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].snapshot.as_ref().unwrap().name, "Пробный период");
    db.cleanup().await;
}

#[tokio::test]
async fn кнопка_я_не_робот_помнит_выбранный_вариант() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    two_variants(&services).await;
    antibot_mode(&services, "confirm").await;
    handle_update(&ctx, text_update(1, 791, "/start")).await;
    let client = client_of(&services, 791).await.unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        sn_modules::catalog::antibot::store::save_mark(
            &mut conn,
            client.id,
            &[sn_modules::catalog::antibot::signals::Signal::DisposableEmail],
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }
    handle_update(&ctx, callback_update(2, 791, "trial:2")).await;
    let asked = last_text(&server).await;
    assert!(asked.contains("подтвердите, что вы не робот"), "{asked}");
    let button = last_buttons(&server)
        .await
        .into_iter()
        .find(|b| b.starts_with("ab:"))
        .unwrap();
    assert!(button.ends_with(":2"), "{button}");
    assert!(button.len() <= 64, "{button}");
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE antibot_checks SET created_at = now() - interval '5 seconds' WHERE client_id = $1",
        )
        .bind(client.id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    handle_update(&ctx, callback_update(3, 791, &button)).await;
    let started = last_text(&server).await;
    assert!(started.contains("Пробный период оформляется"), "{started}");
    let orders = trial_orders(&services, 791).await;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].snapshot.as_ref().unwrap().name, "Европа");

    handle_update(&ctx, text_update(4, 792, "/start")).await;
    let other = client_of(&services, 792).await.unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        sn_modules::catalog::antibot::store::save_mark(
            &mut conn,
            other.id,
            &[sn_modules::catalog::antibot::signals::Signal::DisposableEmail],
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }
    handle_update(&ctx, callback_update(5, 792, "trial:1")).await;
    let token = last_buttons(&server)
        .await
        .into_iter()
        .find_map(|b| {
            b.strip_prefix("ab:")
                .and_then(|rest| rest.split_once(':'))
                .map(|(token, _)| token.to_string())
        })
        .unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE antibot_checks SET created_at = now() - interval '5 seconds' WHERE client_id = $1",
        )
        .bind(other.id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    handle_update(&ctx, callback_update(6, 792, &format!("ab:{token}"))).await;
    let choice = last_text(&server).await;
    assert!(choice.contains("Выберите пробный период"), "{choice}");
    assert!(trial_orders(&services, 792).await.is_empty());
    handle_update(&ctx, callback_update(7, 792, "trial:1")).await;
    let started = last_text(&server).await;
    assert!(started.contains("Пробный период оформляется"), "{started}");
    assert_eq!(
        trial_orders(&services, 792).await[0]
            .snapshot
            .as_ref()
            .unwrap()
            .name,
        "Россия"
    );
    db.cleanup().await;
}
