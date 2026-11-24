use super::*;
use chrono::{Duration, Utc};
use sn_domain::bot_layout::{MenuMode, Screen as LayoutScreen};
use sn_domain::ids::{AdminId, RemnaUserId, SquadId};
use sn_domain::module::ModuleId;
use sn_domain::settings::{ClientAccess, keys};
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_services::bot_layout::{ClientState, LayoutUpdate};
use sn_store::NewSubscription;
use std::collections::{BTreeMap, BTreeSet};

const APP: &str = "https://vpn.example.com/app";

async fn owner(services: &Services) -> AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "пароль-владельца-режима-меню")
        .await
        .unwrap()
        .0
}

async fn public_url(services: &Services, url: &str) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings()
        .set(keys::PUBLIC_URL, &serde_json::json!(url), None)
        .await
        .unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

async fn enable(services: &Services, modules: &[ModuleId]) {
    for module in modules {
        services
            .modules()
            .set_enabled(services.store(), *module, true, None)
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
}

async fn set_mode(services: &Services, admin: AdminId, mode: MenuMode) {
    services
        .bot_layout()
        .update(
            &LayoutUpdate {
                menu_mode: Some(mode),
                ..LayoutUpdate::default()
            },
            admin,
        )
        .await
        .unwrap();
}

async fn hide(services: &Services, admin: AdminId, keys: &[&str]) {
    services
        .bot_layout()
        .update(
            &LayoutUpdate {
                hidden: keys.iter().map(|k| (k.to_string(), true)).collect(),
                ..LayoutUpdate::default()
            },
            admin,
        )
        .await
        .unwrap();
}

fn target(button: &serde_json::Value) -> String {
    if let Some(url) = button["web_app"]["url"].as_str() {
        format!("app {url}")
    } else if let Some(url) = button["url"].as_str() {
        format!("url {url}")
    } else {
        format!(
            "cb {}",
            button["callback_data"].as_str().unwrap_or_default()
        )
    }
}

async fn menu(server: &MockServer) -> Vec<(String, String)> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .map(|b| {
            (
                b["text"].as_str().unwrap_or_default().to_string(),
                target(&b),
            )
        })
        .collect()
}

fn targets(buttons: &[(String, String)]) -> Vec<&str> {
    buttons.iter().map(|(_, t)| t.as_str()).collect()
}

fn labels(buttons: &[(String, String)]) -> Vec<&str> {
    buttons.iter().map(|(l, _)| l.as_str()).collect()
}

async fn subscribe(db: &TestDb, client_id: ClientId) {
    let mut tx = db.store.begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: "Обычные".into(),
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
            name: "Стандарт".into(),
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
                user_id: RemnaUserId(93),
                username: "menu_mode_93".into(),
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
}

#[tokio::test]
async fn веб_режим_открывает_разделы_мини_приложения_а_классический_экраны_бота() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    enable(
        &services,
        &[ModuleId::Gifts, ModuleId::Referral, ModuleId::Promo],
    )
    .await;
    public_url(&services, "https://vpn.example.com").await;
    let admin = owner(&services).await;

    let pairs = |raw: &[(&str, String)]| -> Vec<(String, String)> {
        raw.iter()
            .map(|(label, target)| (label.to_string(), target.clone()))
            .collect()
    };

    handle_update(&ctx, text_update(1, 731, "/start")).await;
    let classic = menu(&server).await;
    assert_eq!(
        classic,
        pairs(&[
            ("Открыть приложение", format!("app {APP}")),
            ("Купить подписку", "cb buy".into()),
            ("Баланс", "cb bal".into()),
            ("Промокод", "cb x:promo".into()),
            ("Докупить", "cb x:options".into()),
            ("Подарить подписку", "cb x:gifts".into()),
            ("Пригласить друга", "cb x:referral".into()),
            ("Поддержка", "cb sup".into()),
        ])
    );

    set_mode(&services, admin, MenuMode::Web).await;
    handle_update(&ctx, text_update(2, 731, "/start")).await;
    let web = menu(&server).await;
    assert_eq!(
        web,
        pairs(&[
            ("Открыть приложение", format!("app {APP}")),
            ("Купить подписку", format!("app {APP}/plans")),
            ("Баланс", format!("app {APP}/topup")),
            ("Промокод", format!("app {APP}/promo")),
            ("Докупить", format!("app {APP}/addons")),
            ("Подарить подписку", format!("app {APP}/gifts")),
            ("Пригласить друга", format!("app {APP}/profile")),
            ("Поддержка", "cb sup".into()),
        ])
    );

    let client = client_of(&services, 731).await.unwrap();
    subscribe(&db, client.id).await;
    handle_update(&ctx, callback_update(3, 731, "m")).await;
    let subscribed = menu(&server).await;
    let find = |label: &str| {
        subscribed
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| panic!("нет кнопки {label}: {subscribed:?}"))
    };
    assert_eq!(find("Подписка"), format!("app {APP}"));
    assert_eq!(
        find("Продлить или сменить тариф"),
        format!("app {APP}/plans")
    );
    assert_eq!(find("Поддержка"), "cb sup");
    assert!(!targets(&subscribed).contains(&"cb buy"), "{subscribed:?}");

    set_mode(&services, admin, MenuMode::Classic).await;
    handle_update(&ctx, callback_update(4, 731, "m")).await;
    let back = menu(&server).await;
    assert!(targets(&back).contains(&"cb buy"), "{back:?}");
    assert!(targets(&back).contains(&"cb subs"), "{back:?}");
    db.cleanup().await;
}

#[tokio::test]
async fn без_мини_приложения_веб_режим_оставляет_обычные_кнопки() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    public_url(&services, "https://vpn.example.com").await;
    let admin = owner(&services).await;
    set_mode(&services, admin, MenuMode::Web).await;
    handle_update(&ctx, text_update(1, 732, "/start")).await;
    assert!(targets(&menu(&server).await).contains(&format!("app {APP}/plans").as_str()));
    services
        .settings_ops()
        .update_client_access(
            &ClientAccess {
                miniapp: false,
                ..ClientAccess::default()
            },
            admin,
        )
        .await
        .unwrap();
    handle_update(&ctx, text_update(2, 732, "/start")).await;
    let fallback = menu(&server).await;
    assert!(targets(&fallback).contains(&"cb buy"), "{fallback:?}");
    assert!(targets(&fallback).contains(&"cb bal"), "{fallback:?}");
    assert!(
        fallback.iter().all(|(_, target)| target.starts_with("cb ")),
        "{fallback:?}"
    );
    assert_eq!(
        services.bot_content().content.menu_mode(),
        MenuMode::Web,
        "режим сохраняется и вернётся, когда мини-приложение включат"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn поддержка_в_веб_режиме_ведёт_в_чат_мини_приложения_только_с_ии() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    public_url(&services, "https://vpn.example.com").await;
    let admin = owner(&services).await;
    set_mode(&services, admin, MenuMode::Web).await;
    services
        .modules()
        .set_config(
            services.store(),
            ModuleId::Ai,
            sn_modules::ModuleConfig::from_value(&serde_json::json!({
                "api_key": "gsk_test_key",
                "api_url": "https://ai.example.com/v1",
                "miniapp": true
            })),
            None,
        )
        .await
        .unwrap();
    enable(&services, &[ModuleId::Ai]).await;
    assert!(sn_modules::catalog::ai::channels(services.modules()).miniapp);
    let support = |buttons: Vec<(String, String)>| {
        buttons
            .into_iter()
            .find(|(label, _)| label == "Поддержка")
            .map(|(_, target)| target)
            .unwrap()
    };
    handle_update(&ctx, text_update(1, 733, "/start")).await;
    assert_eq!(support(menu(&server).await), format!("app {APP}/support"));
    let view = services.bot_layout().view().await.unwrap();
    let slot = view.screens[0]
        .slots
        .iter()
        .find(|s| s.key == "menu.btn.support")
        .unwrap();
    assert_eq!(slot.web, Some("Поддержка"));
    assert_eq!(slot.web_note, None);
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Ai, false, None)
        .await
        .unwrap();
    services.reload().await.unwrap();
    handle_update(&ctx, text_update(2, 733, "/start")).await;
    assert_eq!(support(menu(&server).await), "cb sup");
    db.cleanup().await;
}

#[tokio::test]
async fn скрытые_купить_и_продлить_пропадают_из_меню_в_обоих_режимах() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    public_url(&services, "https://vpn.example.com").await;
    let admin = owner(&services).await;
    hide(&services, admin, &["menu.btn.buy", "menu.btn.extend"]).await;
    handle_update(&ctx, text_update(1, 734, "/start")).await;
    let fresh = menu(&server).await;
    assert!(!labels(&fresh).contains(&"Купить подписку"), "{fresh:?}");
    assert!(!targets(&fresh).contains(&"cb buy"), "{fresh:?}");
    assert!(labels(&fresh).contains(&"Поддержка"), "{fresh:?}");
    set_mode(&services, admin, MenuMode::Web).await;
    handle_update(&ctx, text_update(2, 734, "/start")).await;
    let web = menu(&server).await;
    assert_eq!(labels(&web), labels(&fresh), "{web:?}");
    assert!(
        !targets(&web).contains(&format!("app {APP}/plans").as_str()),
        "{web:?}"
    );
    let client = client_of(&services, 734).await.unwrap();
    subscribe(&db, client.id).await;
    handle_update(&ctx, callback_update(3, 734, "m")).await;
    let subscribed = menu(&server).await;
    assert!(
        !labels(&subscribed).contains(&"Продлить или сменить тариф"),
        "{subscribed:?}"
    );
    assert!(labels(&subscribed).contains(&"Подписка"), "{subscribed:?}");
    handle_update(&ctx, callback_update(4, 734, "subs")).await;
    let screen = menu(&server).await;
    assert!(
        targets(&screen).iter().any(|t| t.starts_with("cb buy:")),
        "продление на экране подписки остаётся: {screen:?}"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn предпросмотр_в_админке_показывает_те_же_кнопки_мини_приложения_что_и_бот() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    enable(&services, &[ModuleId::Gifts, ModuleId::Referral]).await;
    public_url(&services, "https://vpn.example.com").await;
    let admin = owner(&services).await;
    set_mode(&services, admin, MenuMode::Web).await;
    let view = services.bot_layout().view().await.unwrap();
    assert_eq!(view.menu_mode, MenuMode::Web);
    let screen = view
        .screens
        .iter()
        .find(|s| s.key == LayoutScreen::Menu)
        .unwrap();
    let expect = |state: ClientState| -> BTreeMap<String, bool> {
        screen
            .slots
            .iter()
            .filter(|s| s.shown.contains(&state) && !s.hidden)
            .map(|s| (s.text.clone(), s.web.is_some()))
            .collect()
    };
    let opened = |buttons: Vec<(String, String)>| -> BTreeMap<String, bool> {
        buttons
            .into_iter()
            .map(|(label, target)| (label, target.starts_with("app ")))
            .collect()
    };
    handle_update(&ctx, text_update(1, 735, "/start")).await;
    assert_eq!(opened(menu(&server).await), expect(ClientState::New));
    let client = client_of(&services, 735).await.unwrap();
    subscribe(&db, client.id).await;
    handle_update(&ctx, callback_update(2, 735, "m")).await;
    let subscribed = menu(&server).await;
    let unique: BTreeSet<&str> = labels(&subscribed).into_iter().collect();
    assert_eq!(unique.len(), subscribed.len(), "{subscribed:?}");
    assert_eq!(opened(subscribed), expect(ClientState::Active));
    db.cleanup().await;
}
