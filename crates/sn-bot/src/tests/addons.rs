use super::*;
use chrono::{Duration, Utc};
use sn_domain::ids::{RemnaUserId, SquadId};
use sn_domain::order::{OrderKind, PurchaseIntent};
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_domain::tariff::{ExtraDevices, TrafficAddon, TrafficLeftover, TrafficPack};
use sn_domain::traffic::{GIB, TrafficReset};
use sn_store::NewSubscription;

#[tokio::test]
async fn докупка_в_боте_по_дням_для_устройств_и_пакетами_для_трафика() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    handle_update(&ctx, text_update(1, 711, "/start")).await;
    let client = client_of(&services, 711).await.unwrap();
    let sub = {
        let mut tx = db.store.begin().await.unwrap();
        let category = tx
            .tariffs()
            .create_category(&NewCategory {
                name: "Лимит".into(),
                sort: 0,
                visible: true,
            })
            .await
            .unwrap();
        let tariff = tx
            .tariffs()
            .create(&NewTariff {
                category_id: category.id,
                name: "Сто гигабайт".into(),
                description: None,
                on_sale: true,
                sort: 0,
                devices: DevicePolicy {
                    included: 3,
                    extra: Some(ExtraDevices {
                        max: 2,
                        monthly: CurrencyPrices {
                            usd: Some(100),
                            rub: Some(5_000),
                        },
                        monthly_stars: Some(Stars(40)),
                    }),
                },
                traffic: TrafficPolicy {
                    limit_bytes: Some(100 * GIB),
                    reset: TrafficReset::OnPayment,
                },
                traffic_addon: Some(TrafficAddon {
                    packs: vec![TrafficPack {
                        gb: 10,
                        prices: CurrencyPrices {
                            usd: Some(150),
                            rub: Some(10_000),
                        },
                        stars: Some(Stars(75)),
                    }],
                    leftover: TrafficLeftover::Burn,
                }),
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
        let sub = tx
            .subscriptions()
            .create(&NewSubscription {
                client_id: client.id,
                tariff_id: Some(tariff.id),
                kind: SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: AutoRenewState::default(),
            })
            .await
            .unwrap();
        tx.subscriptions()
            .link_remna(
                sub.id,
                &RemnaLink {
                    user_id: RemnaUserId(91),
                    username: "addons_91".into(),
                    short_uuid: None,
                },
            )
            .await
            .unwrap();
        tx.subscriptions()
            .apply_mirror(
                sub.id,
                &RemnaMirror {
                    status: Some(SubscriptionStatus::Active),
                    expire_at: Some(Utc::now() + Duration::days(14) + Duration::hours(12)),
                    traffic_limit_bytes: Some(100 * GIB),
                    traffic_used_bytes: Some(98 * GIB),
                    ..RemnaMirror::default()
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        sub.id
    };
    handle_update(&ctx, callback_update(2, 711, "x:options")).await;
    let menu = last_buttons(&server).await;
    assert!(menu.contains(&format!("opt:{sub}:d")), "{menu:?}");
    assert!(menu.contains(&format!("opt:{sub}:t")), "{menu:?}");
    handle_update(&ctx, callback_update(3, 711, &format!("opt:{sub}:d"))).await;
    let devices = last_text(&server).await;
    assert!(
        devices.contains("До конца подписки 15\u{a0}дней"),
        "{devices}"
    );
    assert!(devices.contains("$0.50 или 20\u{a0}звёзд"), "{devices}");
    let counts = last_buttons(&server).await;
    assert!(counts.contains(&format!("opt:{sub}:d:2")), "{counts:?}");
    handle_update(&ctx, callback_update(4, 711, &format!("opt:{sub}:d:2"))).await;
    let summary = last_text(&server).await;
    assert!(
        summary.contains("Доп. устройства: +2 на 15\u{a0}дней — $1.00"),
        "{summary}"
    );
    handle_update(&ctx, callback_update(5, 711, &format!("opt:{sub}:t"))).await;
    let traffic = last_text(&server).await;
    assert!(traffic.contains("Израсходовано 98"), "{traffic}");
    assert!(
        traffic.contains("Что не израсходуете до продления, сгорит."),
        "{traffic}"
    );
    let packs = last_buttons(&server).await;
    assert!(packs.contains(&format!("opt:{sub}:t:10")), "{packs:?}");
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.ledger()
            .append(&sn_domain::ledger::NewLedgerEntry {
                client_id: client.id,
                wallet: sn_domain::ledger::Wallet::Main,
                amount: sn_domain::money::Money::new(1_000, sn_domain::money::Currency::Usd),
                kind: sn_domain::ledger::LedgerKind::TopUp,
                order_id: None,
                payment_id: None,
                admin_id: None,
                comment: None,
                idempotency_key: Some("test:topup:711".into()),
            })
            .await
            .unwrap();
    }
    handle_update(&ctx, callback_update(6, 711, &format!("opt:{sub}:t:10"))).await;
    let quote = last_text(&server).await;
    assert!(quote.contains("Трафик: +10\u{a0}ГБ — $1.50"), "{quote}");
    let methods = last_buttons(&server).await;
    let balance = format!("op:{sub}:t:10:balance");
    assert!(methods.contains(&balance), "{methods:?}");
    handle_update(&ctx, callback_update(7, 711, &balance)).await;
    let paid = last_text(&server).await;
    assert!(paid.contains("Оплачено с баланса"), "{paid}");
    let mut conn = db.store.conn().await.unwrap();
    let orders = conn
        .orders()
        .for_client(
            client.id,
            sn_domain::paging::PageRequest::new(Some(10), None),
        )
        .await
        .unwrap();
    let order = orders
        .items
        .iter()
        .find(|o| o.kind == OrderKind::Module)
        .expect("заказ докупки");
    assert!(matches!(
        &order.intent,
        PurchaseIntent::Module { kind, .. } if kind == "traffic"
    ));
    assert_eq!(order.quote.total.minor, 150);
    drop(conn);
    db.cleanup().await;
}
