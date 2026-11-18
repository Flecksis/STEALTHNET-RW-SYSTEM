use super::*;
use crate::session::Session;
use chrono::{Duration, Utc};
use sn_domain::ids::{RemnaUserId, SquadId, SubscriptionId};
use sn_domain::module::ModuleId;
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_domain::tariff::{ExtraDevices, Tariff};
use sn_store::NewSubscription;

async fn with_extra(db: &TestDb, category: &str, name: &str, max: u32) -> Tariff {
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
            traffic_addon: None,
            category_id: category.id,
            name: name.into(),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: Some(ExtraDevices {
                    max,
                    monthly: CurrencyPrices {
                        usd: Some(100),
                        rub: Some(9_000),
                    },
                    monthly_stars: None,
                }),
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

async fn subscribed(
    db: &TestDb,
    client_id: ClientId,
    tariff: &Tariff,
    extra_devices: u32,
    user: i64,
) -> SubscriptionId {
    let mut conn = db.store.conn().await.unwrap();
    let subscription = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id,
            tariff_id: Some(tariff.id),
            kind: SubscriptionKind::Paid,
            extra_devices,
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
        .apply_mirror(
            subscription.id,
            &RemnaMirror {
                status: Some(SubscriptionStatus::Active),
                expire_at: Some(Utc::now() + Duration::days(20)),
                ..RemnaMirror::default()
            },
        )
        .await
        .unwrap();
    subscription.id
}

async fn options_on(services: &Services) {
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Options, true, None)
        .await
        .unwrap();
}

async fn cart_devices(services: &Services, chat: i64) -> u32 {
    Session::load(services, chat)
        .await
        .unwrap()
        .cart
        .unwrap()
        .extra_devices
}

#[tokio::test]
async fn при_продлении_в_боте_доп_устройства_подписки_уже_в_корзине() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    options_on(&services).await;
    let standard = with_extra(&db, "Обычный", "Стандарт", 3).await;
    let premium = with_extra(&db, "Быстрый", "Премиум", 3).await;
    handle_update(&ctx, text_update(1, 781, "/start")).await;
    let client = client_of(&services, 781).await.unwrap();
    let sub = subscribed(&db, client.id, &standard, 2, 91).await;
    handle_update(&ctx, callback_update(2, 781, "buy")).await;
    let cart = Session::load(&services, 781).await.unwrap().cart.unwrap();
    assert_eq!(cart.subscription_id, Some(sub));
    handle_update(&ctx, callback_update(3, 781, &format!("t:{}", standard.id))).await;
    handle_update(
        &ctx,
        callback_update(4, 781, &format!("p:{}", standard.periods[0].id)),
    )
    .await;
    let summary = last_text(&server).await;
    assert!(summary.contains("Продление"), "{summary}");
    assert!(
        summary.contains("Доп. устройства: 2 × 1\u{a0}месяц — $2.00"),
        "{summary}"
    );
    assert!(summary.contains("К оплате: <b>$4.99</b>"), "{summary}");
    assert_eq!(cart_devices(&services, 781).await, 2);
    let buttons = last_buttons(&server).await;
    assert!(buttons.contains(&"dv".to_string()), "{buttons:?}");
    assert!(
        last_labels(&server)
            .await
            .contains(&"Изменить доп. устройства".to_string())
    );
    handle_update(&ctx, callback_update(5, 781, "dv")).await;
    let choice = last_text(&server).await;
    assert!(choice.contains("Добавить ещё?"), "{choice}");
    let buttons = last_buttons(&server).await;
    for data in ["d:0", "d:1", "d:2", "d:3", "sum"] {
        assert!(buttons.contains(&data.to_string()), "{data}: {buttons:?}");
    }
    assert_eq!(cart_devices(&services, 781).await, 2, "выбор ещё не сделан");
    handle_update(&ctx, callback_update(6, 781, "d:0")).await;
    assert_eq!(cart_devices(&services, 781).await, 0);
    let summary = last_text(&server).await;
    assert!(!summary.contains("Доп. устройства"), "{summary}");
    assert!(summary.contains("К оплате: <b>$2.99</b>"), "{summary}");
    handle_update(&ctx, callback_update(7, 781, &format!("t:{}", premium.id))).await;
    handle_update(
        &ctx,
        callback_update(8, 781, &format!("p:{}", premium.periods[0].id)),
    )
    .await;
    let other = last_text(&server).await;
    assert!(
        other.contains("Добавить ещё?"),
        "при переходе на другой тариф устройства выбираются заново: {other}"
    );
    assert_eq!(cart_devices(&services, 781).await, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn доп_устройства_в_корзине_продления_не_больше_доступных_в_тарифе() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    options_on(&services).await;
    let small = with_extra(&db, "Обычный", "Малый", 1).await;
    handle_update(&ctx, text_update(1, 782, "/start")).await;
    let client = client_of(&services, 782).await.unwrap();
    subscribed(&db, client.id, &small, 4, 92).await;
    handle_update(&ctx, callback_update(2, 782, "buy")).await;
    handle_update(&ctx, callback_update(3, 782, &format!("t:{}", small.id))).await;
    handle_update(
        &ctx,
        callback_update(4, 782, &format!("p:{}", small.periods[0].id)),
    )
    .await;
    assert_eq!(cart_devices(&services, 782).await, 1);
    let summary = last_text(&server).await;
    assert!(
        summary.contains("Доп. устройства: 1 × 1\u{a0}месяц"),
        "{summary}"
    );
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Options, false, None)
        .await
        .unwrap();
    handle_update(
        &ctx,
        callback_update(5, 782, &format!("p:{}", small.periods[0].id)),
    )
    .await;
    assert_eq!(
        cart_devices(&services, 782).await,
        0,
        "без модуля доп. устройства не продаются"
    );
    assert!(!last_buttons(&server).await.contains(&"dv".to_string()));
    db.cleanup().await;
}

#[tokio::test]
async fn новая_подписка_в_боте_спрашивает_доп_устройства_как_раньше() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    options_on(&services).await;
    let standard = with_extra(&db, "Обычный", "Стандарт", 2).await;
    handle_update(&ctx, text_update(1, 783, "/start")).await;
    handle_update(&ctx, callback_update(2, 783, "buy")).await;
    handle_update(&ctx, callback_update(3, 783, &format!("t:{}", standard.id))).await;
    handle_update(
        &ctx,
        callback_update(4, 783, &format!("p:{}", standard.periods[0].id)),
    )
    .await;
    assert!(last_text(&server).await.contains("Добавить ещё?"));
    let buttons = last_buttons(&server).await;
    assert!(
        buttons.contains(&format!("t:{}", standard.id)),
        "{buttons:?}"
    );
    handle_update(&ctx, callback_update(5, 783, "d:2")).await;
    assert_eq!(cart_devices(&services, 783).await, 2);
    let summary = last_text(&server).await;
    assert!(
        summary.contains("Доп. устройства: 2 × 1\u{a0}месяц"),
        "{summary}"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn бот_показывает_лесенку_и_скидку_за_доп_устройства() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    options_on(&services).await;
    let standard = with_extra(&db, "Обычный", "Стандарт", 5).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.tariffs()
            .set_device_discounts(
                standard.id,
                &[
                    sn_domain::tariff::DeviceDiscount {
                        from: 2,
                        percent: 10,
                    },
                    sn_domain::tariff::DeviceDiscount {
                        from: 4,
                        percent: 20,
                    },
                ],
            )
            .await
            .unwrap();
    }
    handle_update(&ctx, text_update(1, 782, "/start")).await;
    handle_update(&ctx, callback_update(2, 782, "buy")).await;
    handle_update(&ctx, callback_update(3, 782, &format!("t:{}", standard.id))).await;
    handle_update(
        &ctx,
        callback_update(4, 782, &format!("p:{}", standard.periods[0].id)),
    )
    .await;
    let screen = last_text(&server).await;
    assert!(
        screen.contains("Скидка за доп. устройства: от 2\u{a0}доп. устройств — 10%, от 4 — 20%."),
        "{screen}"
    );
    handle_update(&ctx, callback_update(5, 782, "d:2")).await;
    let summary = last_text(&server).await;
    assert!(
        summary.contains("Скидка за 2\u{a0}доп. устройства, −10% — −$0.20"),
        "{summary}"
    );
    assert!(summary.contains("$4.79"), "{summary}");
    db.cleanup().await;
}
