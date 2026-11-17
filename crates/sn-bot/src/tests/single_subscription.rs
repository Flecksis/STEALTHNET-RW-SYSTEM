use super::*;
use chrono::{Duration, Utc};
use sn_domain::ids::{RemnaUserId, SquadId, SubscriptionId};
use sn_domain::settings::{MultiSubscriptions, keys};
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_domain::tariff::Tariff;
use sn_domain::traffic::GIB;
use sn_store::NewSubscription;

async fn tariff_in(db: &TestDb, category: &str, name: &str) -> Tariff {
    let mut tx = db.store.begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: category.into(),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap();
    let tariff = tx
        .tariffs()
        .create(&NewTariff {
            category_id: category.id,
            name: name.into(),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: None,
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![SquadId(uuid::Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![NewPeriod {
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(299),
                    rub: Some(19_900),
                },
                stars: None,
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    tariff
}

async fn give(db: &TestDb, client_id: ClientId, tariff: &Tariff, user: i64) -> SubscriptionId {
    give_mirror(
        db,
        client_id,
        tariff,
        user,
        RemnaMirror {
            status: Some(SubscriptionStatus::Active),
            expire_at: Some(Utc::now() + Duration::days(20)),
            ..RemnaMirror::default()
        },
    )
    .await
}

async fn give_mirror(
    db: &TestDb,
    client_id: ClientId,
    tariff: &Tariff,
    user: i64,
    mirror: RemnaMirror,
) -> SubscriptionId {
    let mut conn = db.store.conn().await.unwrap();
    let subscription = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id,
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
                user_id: RemnaUserId(user),
                username: format!("morozov_a_{user}"),
                short_uuid: None,
            },
        )
        .await
        .unwrap();
    conn.subscriptions()
        .apply_mirror(subscription.id, &mirror)
        .await
        .unwrap();
    subscription.id
}

async fn set_multi(services: &Services, enabled: bool) {
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set_as(
                keys::MULTI_SUBSCRIPTIONS,
                &MultiSubscriptions { enabled },
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
}

#[tokio::test]
async fn переход_на_другую_категорию_предупреждает_перед_оплатой() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let standard = tariff_in(&db, "Обычный", "Стандарт").await;
    let premium = tariff_in(&db, "Быстрый", "Премиум").await;
    handle_update(&ctx, text_update(1, 701, "/start")).await;
    let client = client_of(&services, 701).await.unwrap();
    let only = give(&db, client.id, &standard, 77).await;
    handle_update(&ctx, callback_update(2, 701, "buy")).await;
    let catalog = last_buttons(&server).await;
    assert!(
        catalog.contains(&format!("t:{}", premium.id)),
        "{catalog:?}"
    );
    let cart = Session::load(&services, 701).await.unwrap().cart.unwrap();
    assert_eq!(cart.subscription_id, Some(only));
    handle_update(&ctx, callback_update(3, 701, &format!("t:{}", premium.id))).await;
    handle_update(
        &ctx,
        callback_update(4, 701, &format!("p:{}", premium.periods[0].id)),
    )
    .await;
    let summary = last_text(&server).await;
    assert!(
        summary.contains("<b>Будет переход на тариф «Премиум».</b>"),
        "{summary}"
    );
    assert!(
        summary.contains(
            "Текущая подписка (Стандарт) будет удалена, новая станет доступна сразу после оплаты"
        ),
        "{summary}"
    );
    let warning = summary.find("Будет переход").unwrap();
    let total = summary.find("К оплате").unwrap();
    assert!(warning < total, "{summary}");
    handle_update(&ctx, callback_update(5, 701, &format!("t:{}", standard.id))).await;
    handle_update(
        &ctx,
        callback_update(6, 701, &format!("p:{}", standard.periods[0].id)),
    )
    .await;
    let summary = last_text(&server).await;
    assert!(
        summary.contains("Продление: подписка будет действовать до"),
        "{summary}"
    );
    assert!(!summary.contains("переход"), "{summary}");
    db.cleanup().await;
}

#[tokio::test]
async fn новая_подписка_в_боте_только_с_мультиподписками() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let standard = tariff_in(&db, "Обычный", "Стандарт").await;
    handle_update(&ctx, text_update(1, 702, "/start")).await;
    let client = client_of(&services, 702).await.unwrap();
    let only = give(&db, client.id, &standard, 78).await;
    handle_update(&ctx, callback_update(2, 702, "buy:new")).await;
    let cart = Session::load(&services, 702).await.unwrap().cart.unwrap();
    assert_eq!(cart.subscription_id, Some(only));
    set_multi(&services, true).await;
    handle_update(&ctx, callback_update(3, 702, "buy")).await;
    let buttons = last_buttons(&server).await;
    assert!(buttons.contains(&format!("buy:{only}")), "{buttons:?}");
    assert!(buttons.contains(&"buy:new".to_string()), "{buttons:?}");
    handle_update(&ctx, callback_update(4, 702, "buy:new")).await;
    let cart = Session::load(&services, 702).await.unwrap().cart.unwrap();
    assert_eq!(cart.subscription_id, None);
    let second = give(&db, client.id, &standard, 79).await;
    set_multi(&services, false).await;
    handle_update(&ctx, callback_update(5, 702, "buy")).await;
    let buttons = last_buttons(&server).await;
    assert!(buttons.contains(&format!("buy:{only}")), "{buttons:?}");
    assert!(buttons.contains(&format!("buy:{second}")), "{buttons:?}");
    assert!(!buttons.contains(&"buy:new".to_string()), "{buttons:?}");
    db.cleanup().await;
}

fn detailed(expire_at: chrono::DateTime<Utc>) -> RemnaMirror {
    RemnaMirror {
        status: Some(SubscriptionStatus::Active),
        expire_at: Some(expire_at),
        traffic_used_bytes: Some(12 * GIB),
        traffic_limit_bytes: Some(300 * GIB),
        hwid_limit: Some(3),
        devices_count: Some(1),
        ..RemnaMirror::default()
    }
}

#[tokio::test]
async fn меню_с_одной_подпиской_показывает_подробности() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let standard = tariff_in(&db, "Обычный", "Базовый").await;
    handle_update(&ctx, text_update(1, 704, "/start")).await;
    let client = client_of(&services, 704).await.unwrap();
    let expire_at = Utc::now() + Duration::days(30) - Duration::minutes(5);
    give_mirror(&db, client.id, &standard, 80, detailed(expire_at)).await;
    handle_update(&ctx, callback_update(2, 704, "m")).await;
    let menu = last_text(&server).await;
    let offset = services.settings().remna_offset();
    let expected = [
        "<b>Базовый</b> · активна".to_string(),
        format!(
            "До {}, осталось 30\u{a0}дней",
            crate::text::date(expire_at, offset)
        ),
        "Трафик: 12\u{a0}ГБ из 300\u{a0}ГБ".to_string(),
        "Устройства: 1 из 3".to_string(),
        "Баланс: $0.00".to_string(),
    ];
    for line in &expected {
        assert!(menu.contains(line.as_str()), "{line:?} нет в меню:\n{menu}");
    }
    assert_eq!(menu.matches("Базовый").count(), 1, "{menu}");
    assert!(!menu.contains("Базовый: активна"), "{menu}");

    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(sn_domain::payment::providers::BALANCE),
                &serde_json::json!({"enabled": false}),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    handle_update(&ctx, callback_update(3, 704, "m")).await;
    let menu = last_text(&server).await;
    assert!(menu.contains("Устройства: 1 из 3"), "{menu}");
    assert!(!menu.contains("Баланс"), "{menu}");
    db.cleanup().await;
}

#[tokio::test]
async fn меню_без_лимитов_и_с_истёкшей_подпиской() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let standard = tariff_in(&db, "Обычный", "Базовый").await;
    handle_update(&ctx, text_update(1, 705, "/start")).await;
    let client = client_of(&services, 705).await.unwrap();
    let ended = Utc::now() - Duration::days(2);
    give_mirror(
        &db,
        client.id,
        &standard,
        81,
        RemnaMirror {
            status: Some(SubscriptionStatus::Expired),
            expire_at: Some(ended),
            traffic_limit_bytes: Some(0),
            ..RemnaMirror::default()
        },
    )
    .await;
    handle_update(&ctx, callback_update(2, 705, "m")).await;
    let menu = last_text(&server).await;
    let offset = services.settings().remna_offset();
    assert!(menu.contains("<b>Базовый</b> · истекла"), "{menu}");
    assert!(
        menu.contains(&format!("Закончилась {}", crate::text::date(ended, offset))),
        "{menu}"
    );
    assert!(menu.contains("Трафик: без лимита"), "{menu}");
    assert!(!menu.contains("Устройства"), "{menu}");
    db.cleanup().await;
}

#[tokio::test]
async fn меню_с_двумя_подписками_короткими_строками() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let standard = tariff_in(&db, "Обычный", "Базовый").await;
    let premium = tariff_in(&db, "Быстрый", "Премиум").await;
    handle_update(&ctx, text_update(1, 706, "/start")).await;
    let client = client_of(&services, 706).await.unwrap();
    let expire_at = Utc::now() + Duration::days(30);
    give_mirror(&db, client.id, &standard, 82, detailed(expire_at)).await;
    give_mirror(&db, client.id, &premium, 83, detailed(expire_at)).await;
    handle_update(&ctx, callback_update(2, 706, "m")).await;
    let menu = last_text(&server).await;
    let date = crate::text::date(expire_at, services.settings().remna_offset());
    for tariff in ["Базовый", "Премиум"] {
        let line = format!("{tariff}: активна, до {date}");
        assert_eq!(menu.matches(line.as_str()).count(), 1, "{line:?}:\n{menu}");
    }
    assert!(!menu.contains("Трафик"), "{menu}");
    assert!(!menu.contains("Баланс"), "{menu}");
    db.cleanup().await;
}
