use super::*;
use chrono::{Duration, Utc};
use sn_domain::bot_layout::{ROW_MAX, Rows};
use sn_domain::ids::{AdminId, RemnaUserId, SquadId};
use sn_domain::module::ModuleId;
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_services::bot_content::CONTENT_KEY;
use sn_services::bot_layout::{ClientState, LayoutUpdate};
use sn_store::NewSubscription;
use std::collections::{BTreeMap, BTreeSet};

fn rows(raw: &[&[&str]]) -> Rows {
    raw.iter()
        .map(|row| row.iter().map(|k| k.to_string()).collect())
        .collect()
}

async fn owner(services: &Services) -> AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "пароль-владельца-раскладки")
        .await
        .unwrap()
        .0
}

async fn save(services: &Services, admin: AdminId, screen: &str, layout: Option<Rows>) {
    services
        .bot_layout()
        .update(
            &LayoutUpdate {
                layouts: BTreeMap::from([(screen.to_string(), layout)]),
                hidden: BTreeMap::new(),
            },
            admin,
        )
        .await
        .unwrap();
}

async fn put_content(services: &Services, value: serde_json::Value) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings()
        .set(CONTENT_KEY, &value, None)
        .await
        .unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

async fn keyboard(server: &MockServer) -> Vec<Vec<String>> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            row.as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|b| {
                    b["callback_data"]
                        .as_str()
                        .or_else(|| b["url"].as_str())
                        .or_else(|| b["web_app"]["url"].as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .collect()
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

async fn subscribe(db: &TestDb, client_id: ClientId) -> sn_domain::ids::SubscriptionId {
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
                user_id: RemnaUserId(91),
                username: "morozov_a_91".into(),
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

#[tokio::test]
async fn раскладка_главного_меню_применяется_в_боте() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    enable(&services, &[ModuleId::Gifts, ModuleId::Referral]).await;
    let admin = owner(&services).await;
    handle_update(&ctx, text_update(1, 610, "/start")).await;
    let standard = keyboard(&server).await;
    assert!(standard.iter().all(|row| row.len() == 1), "{standard:?}");
    let flat: Vec<String> = standard.concat();
    assert!(flat.contains(&"x:gifts".to_string()), "{flat:?}");
    save(
        &services,
        admin,
        "menu",
        Some(rows(&[
            &["menu.btn.support", "menu.btn.buy"],
            &[
                "menu.btn.referral",
                "menu.btn.subscription",
                "menu.btn.gifts",
            ],
        ])),
    )
    .await;
    handle_update(&ctx, text_update(2, 610, "/start")).await;
    let arranged = keyboard(&server).await;
    assert_eq!(arranged[0], vec!["sup", "buy"]);
    assert_eq!(arranged[1], vec!["x:referral", "x:gifts"]);
    assert!(
        arranged[2..].iter().all(|row| row.len() == 1),
        "{arranged:?}"
    );
    let rest: Vec<String> = flat
        .iter()
        .filter(|data| !["sup", "buy", "x:referral", "x:gifts"].contains(&data.as_str()))
        .cloned()
        .collect();
    assert_eq!(arranged[2..].concat(), rest);
    db.cleanup().await;
}

#[tokio::test]
async fn новая_кнопка_не_теряется_а_удалённая_пропускается() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 611, "/start")).await;
    let standard: Vec<String> = keyboard(&server).await.concat();
    put_content(
        &services,
        serde_json::json!({
            "layouts": {
                "menu": [["menu.btn.removed_long_ago", "menu.btn.support"], ["menu.btn.buy"]]
            }
        }),
    )
    .await;
    handle_update(&ctx, text_update(2, 611, "/start")).await;
    let arranged = keyboard(&server).await;
    assert_eq!(arranged[0], vec!["sup"]);
    assert_eq!(arranged[1], vec!["buy"]);
    let tail: Vec<String> = standard
        .iter()
        .filter(|data| *data != "sup" && *data != "buy")
        .cloned()
        .collect();
    assert!(!tail.is_empty());
    assert_eq!(arranged[2..].concat(), tail);
    assert!(arranged[2..].iter().all(|row| row.len() == 1));
    let shown: BTreeSet<String> = arranged.concat().into_iter().collect();
    let before: BTreeSet<String> = standard.into_iter().collect();
    assert_eq!(shown, before);
    db.cleanup().await;
}

#[tokio::test]
async fn в_ряду_бота_не_больше_трёх_кнопок() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    enable(&services, &[ModuleId::Gifts, ModuleId::Referral]).await;
    let admin = owner(&services).await;
    handle_update(&ctx, text_update(1, 612, "/start")).await;
    let standard: BTreeSet<String> = keyboard(&server).await.concat().into_iter().collect();
    let all: Vec<&str> = sn_domain::bot_layout::MENU.iter().map(|s| s.key).collect();
    put_content(
        &services,
        serde_json::json!({ "layouts": { "menu": [all] } }),
    )
    .await;
    handle_update(&ctx, text_update(2, 612, "/start")).await;
    let arranged = keyboard(&server).await;
    assert!(
        arranged.iter().all(|row| row.len() <= ROW_MAX),
        "{arranged:?}"
    );
    assert_eq!(arranged[0].len(), ROW_MAX, "{arranged:?}");
    let shown: BTreeSet<String> = arranged.concat().into_iter().collect();
    assert_eq!(shown, standard);
    let crowded = services
        .bot_layout()
        .update(
            &LayoutUpdate {
                layouts: BTreeMap::from([(
                    "menu".to_string(),
                    Some(rows(&[&[
                        "menu.btn.buy",
                        "menu.btn.support",
                        "menu.btn.promo",
                        "menu.btn.gifts",
                    ]])),
                )]),
                hidden: BTreeMap::new(),
            },
            admin,
        )
        .await;
    assert!(crowded.is_err());
    db.cleanup().await;
}

#[tokio::test]
async fn вернуть_как_было_возвращает_стандартный_порядок() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    handle_update(&ctx, text_update(1, 613, "/start")).await;
    let standard = keyboard(&server).await;
    save(
        &services,
        admin,
        "menu",
        Some(rows(&[&[
            "menu.btn.support",
            "menu.btn.buy",
            "menu.btn.promo",
        ]])),
    )
    .await;
    handle_update(&ctx, text_update(2, 613, "/start")).await;
    assert_ne!(keyboard(&server).await, standard);
    save(&services, admin, "menu", None).await;
    handle_update(&ctx, text_update(3, 613, "/start")).await;
    assert_eq!(keyboard(&server).await, standard);
    db.cleanup().await;
}

#[tokio::test]
async fn раскладка_экрана_подписки_применяется_в_боте() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    let admin = owner(&services).await;
    handle_update(&ctx, text_update(1, 614, "/start")).await;
    let client = client_of(&services, 614).await.unwrap();
    let id = subscribe(&db, client.id).await;
    handle_update(&ctx, callback_update(2, 614, "subs")).await;
    let standard = keyboard(&server).await;
    assert_eq!(
        standard,
        vec![
            vec![format!("dev:{id}")],
            vec![format!("rei:{id}")],
            vec![format!("buy:{id}")],
            vec!["subs".to_string(), "m".to_string()],
        ]
    );
    save(
        &services,
        admin,
        "subscription",
        Some(rows(&[
            &["common.btn.menu", "sub.btn.extend"],
            &["sub.btn.devices"],
        ])),
    )
    .await;
    handle_update(&ctx, callback_update(3, 614, "subs")).await;
    assert_eq!(
        keyboard(&server).await,
        vec![
            vec!["m".to_string(), format!("buy:{id}")],
            vec![format!("dev:{id}")],
            vec![format!("rei:{id}")],
            vec!["subs".to_string()],
        ]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn предпросмотр_в_админке_совпадает_с_кнопками_бота() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    enable(&services, &[ModuleId::Gifts, ModuleId::Referral]).await;
    let admin = owner(&services).await;
    save(
        &services,
        admin,
        "menu",
        Some(rows(&[&["menu.btn.gifts", "menu.btn.support"]])),
    )
    .await;
    let view = services.bot_layout().view().await.unwrap();
    let menu = view
        .screens
        .iter()
        .find(|s| s.key == sn_domain::bot_layout::Screen::Menu)
        .unwrap();
    let expect = |state: ClientState| -> BTreeSet<String> {
        menu.slots
            .iter()
            .filter(|s| s.shown.contains(&state) && !s.hidden)
            .map(|s| s.text.clone())
            .collect()
    };
    handle_update(&ctx, text_update(1, 615, "/start")).await;
    let fresh: BTreeSet<String> = last_labels(&server).await.into_iter().collect();
    assert_eq!(fresh, expect(ClientState::New));
    let client = client_of(&services, 615).await.unwrap();
    subscribe(&db, client.id).await;
    handle_update(&ctx, callback_update(2, 615, "m")).await;
    let subscribed: BTreeSet<String> = last_labels(&server).await.into_iter().collect();
    assert_eq!(subscribed, expect(ClientState::Active));
    assert_eq!(
        last_labels(&server).await[..2],
        ["Подарить подписку".to_string(), "Поддержка".to_string()]
    );
    db.cleanup().await;
}
