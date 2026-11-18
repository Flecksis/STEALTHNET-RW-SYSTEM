use super::*;
use crate::catalog::testing::{TestCore, client, context, order, usd};
use crate::context::GrantRefusal;
use sn_domain::money::Currency;
use sn_domain::order::{OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::pricing::Quote;
use sn_store::Store;
use sn_store::testing::test_store;

fn squad(n: u128) -> SquadId {
    SquadId(Uuid::from_u128(n))
}

fn sample(name: &str) -> NewLink {
    NewLink {
        name: name.into(),
        code: None,
        squads: vec![squad(3), squad(3), squad(4)],
        days: 7,
        traffic_gb: Some(20),
        traffic_reset: TrafficReset::Month,
        devices: 2,
        max_activations: None,
        expires_at: None,
    }
}

async fn purchase(store: &Store, client_id: ClientId, kind: OrderKind, status: OrderStatus) {
    let created = order(
        store,
        client_id,
        kind,
        PurchaseIntent::TopUp { amount: usd(100) },
        Quote {
            currency: Currency::Usd,
            lines: Vec::new(),
            subtotal: usd(100),
            discount: usd(0),
            total: usd(100),
            stars: None,
            days: 30,
            extra_devices: 0,
        },
    )
    .await;
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "UPDATE orders SET status = $2, paid_at = now() + interval '1 second' WHERE id = $1",
    )
    .bind(created.id.0)
    .bind(status.as_str())
    .execute(conn.conn())
    .await
    .unwrap();
}

#[test]
fn код_ссылки_берётся_из_параметра_запуска() {
    assert_eq!(
        start_code("pl_blogger-ivan").as_deref(),
        Some("blogger-ivan")
    );
    assert_eq!(
        start_code("pl_Spring_2026__s_youtube").as_deref(),
        Some("Spring_2026")
    );
    assert_eq!(start_code("pl_abc"), None);
    assert_eq!(start_code("ref_ABCDEF"), None);
    assert_eq!(start_code("gift_ABCD-EFGH-JKMN"), None);
    assert_eq!(generate_code().len(), 8);
    assert_eq!(
        bot_link("@vpn_bot", "spring"),
        "https://t.me/vpn_bot?start=pl_spring"
    );
}

#[tokio::test]
async fn ссылка_создаётся_только_с_правильными_полями() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                sn_domain::settings::keys::BOT_USERNAME,
                &serde_json::json!("vpn_test_bot"),
                None,
            )
            .await
            .unwrap();
    }
    let made = create(&ctx, &sample("Блогер Иван"), None).await.unwrap();
    assert_eq!(made.link.squads, vec![squad(3), squad(4)]);
    assert_eq!(made.link.traffic_limit_bytes, Some(20 * GIB));
    assert_eq!(made.link.traffic_reset, TrafficReset::Month);
    assert_eq!(made.state, LinkState::Active);
    assert_eq!(
        made.bot_link,
        Some(format!(
            "https://t.me/vpn_test_bot?start=pl_{}",
            made.link.code
        ))
    );
    let unlimited = create(
        &ctx,
        &NewLink {
            code: Some("spring-2026".into()),
            traffic_gb: Some(0),
            ..sample("Весна")
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(unlimited.link.code, "spring-2026");
    assert_eq!(unlimited.link.traffic_limit_bytes, None);
    assert_eq!(unlimited.link.traffic_reset, TrafficReset::NoReset);
    let bad = [
        NewLink {
            code: Some("spring-2026".into()),
            ..sample("Повтор")
        },
        NewLink {
            code: Some("с пробелом".into()),
            ..sample("Код")
        },
        NewLink {
            squads: Vec::new(),
            ..sample("Без сквадов")
        },
        NewLink {
            days: 0,
            ..sample("Без срока")
        },
        NewLink {
            max_activations: Some(0),
            ..sample("Ноль")
        },
        NewLink {
            expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
            ..sample("Прошла")
        },
        sample("   "),
    ];
    for link in &bad {
        assert!(
            matches!(create(&ctx, link, None).await, Err(ModuleError::Invalid(_))),
            "{link:?}"
        );
    }
    db.cleanup().await;
}

#[tokio::test]
async fn ссылка_срабатывает_один_раз_на_клиента_и_до_лимита() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let link = create(
        &ctx,
        &NewLink {
            max_activations: Some(2),
            ..sample("Блогер")
        },
        None,
    )
    .await
    .unwrap()
    .link;
    let first = client(&db.store).await;
    let second = client(&db.store).await;
    let third = client(&db.store).await;
    let done = activate(&ctx, first, &link.code).await.unwrap();
    assert_eq!(done.days, 7);
    assert_eq!(done.devices, 2);
    let grants = core.grants();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].0.module, ModuleId::Promolinks);
    assert_eq!(
        grants[0].0.spec,
        GrantSpec::Custom {
            name: "Промо-подписка".into(),
            days: 7,
            devices: 2,
            traffic: TrafficPolicy {
                limit_bytes: Some(20 * GIB),
                reset: TrafficReset::Month,
            },
            squads: vec![squad(3), squad(4)],
        }
    );
    assert!(matches!(
        activate(&ctx, first, &link.code).await,
        Err(LinkError::Rejected(LinkRejection::AlreadyUsed))
    ));
    activate(&ctx, second, &link.code).await.unwrap();
    assert!(matches!(
        activate(&ctx, third, &link.code).await,
        Err(LinkError::Rejected(LinkRejection::Exhausted))
    ));
    assert!(matches!(
        activate(&ctx, second, &link.code).await,
        Err(LinkError::Rejected(LinkRejection::AlreadyUsed))
    ));
    assert_eq!(core.grants().len(), 2);
    let listed = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(listed.items[0].link.activations, 2);
    assert_eq!(listed.items[0].state, LinkState::Exhausted);
    assert!(matches!(
        activate(&ctx, third, "nolink").await,
        Err(LinkError::Rejected(LinkRejection::NotFound))
    ));
    assert!(matches!(
        activate(&ctx, third, "не код").await,
        Err(LinkError::Rejected(LinkRejection::NotFound))
    ));
    db.cleanup().await;
}

#[tokio::test]
async fn выключенная_просроченная_и_отказ_ядра_отвечают_понятно() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let someone = client(&db.store).await;
    let off = create(&ctx, &sample("Выключенная"), None)
        .await
        .unwrap()
        .link;
    let switched = set_enabled(&ctx, off.id, false, None).await.unwrap();
    assert_eq!(switched.state, LinkState::Disabled);
    assert!(matches!(
        activate(&ctx, someone, &off.code).await,
        Err(LinkError::Rejected(LinkRejection::Disabled))
    ));
    let on = set_enabled(&ctx, off.id, true, None).await.unwrap();
    assert_eq!(on.state, LinkState::Active);
    assert!(on.link.disabled_at.is_none());
    let old = create(&ctx, &sample("Старая"), None).await.unwrap().link;
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query("UPDATE promo_links SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(old.id)
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    assert!(matches!(
        activate(&ctx, someone, &old.code).await,
        Err(LinkError::Rejected(LinkRejection::Expired))
    ));
    core.refuse(Some(GrantRefusal::new(
        "grant.has_subscription",
        "У вас уже есть действующая подписка",
    )));
    match activate(&ctx, someone, &on.link.code).await {
        Err(LinkError::Rejected(LinkRejection::Grant(refusal))) => {
            assert_eq!(refusal.code, "grant.has_subscription");
        }
        other => panic!("ожидали отказ ядра: {other:?}"),
    }
    core.refuse(None);
    let listed = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    let fresh = listed
        .items
        .iter()
        .find(|i| i.link.id == on.link.id)
        .unwrap();
    assert_eq!(fresh.link.activations, 0);
    activate(&ctx, someone, &on.link.code).await.unwrap();
    let active = list(&ctx, ListFilter::Active, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(
        active.items.iter().map(|i| i.link.id).collect::<Vec<_>>(),
        vec![on.link.id]
    );
    let off_list = list(&ctx, ListFilter::Off, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(
        off_list.items.iter().map(|i| i.link.id).collect::<Vec<_>>(),
        vec![old.id]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn статистика_считает_купивших_после_активации() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let link = create(&ctx, &sample("Статистика"), None)
        .await
        .unwrap()
        .link;
    let buyer = client(&db.store).await;
    let renewer = client(&db.store).await;
    let refunded = client(&db.store).await;
    let idle = client(&db.store).await;
    let before = client(&db.store).await;
    purchase(&db.store, before, OrderKind::New, OrderStatus::Fulfilled).await;
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query("UPDATE orders SET paid_at = now() - interval '1 day' WHERE client_id = $1")
        .bind(before.0)
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    for who in [buyer, renewer, refunded, idle, before] {
        activate(&ctx, who, &link.code).await.unwrap();
    }
    purchase(&db.store, buyer, OrderKind::New, OrderStatus::Fulfilled).await;
    purchase(&db.store, buyer, OrderKind::Renewal, OrderStatus::Fulfilled).await;
    purchase(&db.store, renewer, OrderKind::Change, OrderStatus::Paid).await;
    purchase(&db.store, refunded, OrderKind::New, OrderStatus::Refunded).await;
    purchase(&db.store, idle, OrderKind::TopUp, OrderStatus::Fulfilled).await;
    let listed = list(&ctx, ListFilter::All, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(listed.items[0].link.activations, 5);
    assert_eq!(listed.items[0].purchased, 2);
    db.cleanup().await;
}
