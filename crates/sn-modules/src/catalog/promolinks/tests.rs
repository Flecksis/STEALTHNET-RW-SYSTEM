use super::*;
use crate::catalog::antibot::{JournalFilter, store as journal};
use crate::catalog::testing::{
    TestCore, age_challenge, antibot as guarded, antibot_actions, client, context, order, suspect,
    usd,
};
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

async fn mode(store: &Store, mode: &str) {
    guarded(
        store,
        serde_json::json!({ "mode": mode, "burst_count": 0, "check_username": false }),
    )
    .await;
}

async fn kinds(store: &Store, client_id: ClientId, filter: JournalFilter) -> Vec<String> {
    let mut conn = store.conn().await.unwrap();
    journal::journal(&mut conn, filter, Some(client_id), PageRequest::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|item| format!("{}:{}", item.action.as_str(), item.details["target"]))
        .collect()
}

fn asks(outcome: Result<Activated, LinkError>) {
    match outcome {
        Err(LinkError::Rejected(rejection)) => {
            assert_eq!(rejection, LinkRejection::Confirm);
            assert_eq!(rejection.code(), antibot::CONFIRM_CODE);
        }
        other => panic!("ожидали просьбу подтвердить: {other:?}"),
    }
}

#[tokio::test]
async fn отмеченный_получает_подписку_по_ссылке_только_после_кнопки() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    mode(&db.store, "confirm").await;
    let link = create(&ctx, &sample("Блогер"), None).await.unwrap().link;
    let second = create(&ctx, &sample("Второй блогер"), None)
        .await
        .unwrap()
        .link;
    let honest = client(&db.store).await;
    let marked = client(&db.store).await;
    suspect(&db.store, marked).await;

    activate(&ctx, honest, &link.code).await.unwrap();
    assert_eq!(core.grants().len(), 1);
    assert!(antibot_actions(&db.store, honest).await.is_empty());

    asks(activate(&ctx, marked, &link.code).await);
    asks(activate(&ctx, marked, &link.code).await);
    asks(activate_confirmed(&ctx, marked, &link.code, "чужой").await);
    let token = antibot::challenge(&ctx, marked).await.unwrap();
    asks(activate_confirmed(&ctx, marked, &link.code, &token).await);
    assert_eq!(core.grants().len(), 1);
    assert_eq!(
        antibot_actions(&db.store, marked).await,
        ["promo_link_asked"]
    );

    age_challenge(&db.store, marked).await;
    let done = activate_confirmed(&ctx, marked, &link.code, &token)
        .await
        .unwrap();
    assert_eq!(done.days, 7);
    assert_eq!(core.grants().len(), 2);
    assert_eq!(core.grants()[1].0.client_id, marked);
    activate(&ctx, marked, &second.code).await.unwrap();
    assert_eq!(core.grants().len(), 3);
    assert_eq!(
        antibot_actions(&db.store, marked).await,
        ["promo_link_asked", "confirmed"]
    );
    assert_eq!(
        kinds(&db.store, marked, JournalFilter::Promo).await,
        [
            "confirmed:\"promo_link\"",
            "promo_link_asked:\"promo_link\""
        ]
    );
    assert!(
        kinds(&db.store, marked, JournalFilter::Trial)
            .await
            .is_empty()
    );
    let mut conn = db.store.conn().await.unwrap();
    let page = journal::journal(
        &mut conn,
        JournalFilter::Promo,
        Some(marked),
        PageRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        page.items[1].title,
        "Попросили подтвердить перед промо-ссылкой"
    );
    assert_eq!(page.items[1].details["name"], "Блогер");
    assert_eq!(page.items[1].details["link_id"], link.id);
    db.cleanup().await;
}

#[tokio::test]
async fn ссылку_и_выдачу_проверяем_раньше_кнопки_антибота() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    mode(&db.store, "confirm").await;
    let marked = client(&db.store).await;
    suspect(&db.store, marked).await;
    let off = create(&ctx, &sample("Выключенная"), None)
        .await
        .unwrap()
        .link;
    set_enabled(&ctx, off.id, false, None).await.unwrap();
    let old = create(&ctx, &sample("Старая"), None).await.unwrap().link;
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query("UPDATE promo_links SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(old.id)
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    let fine = create(&ctx, &sample("Рабочая"), None).await.unwrap().link;
    assert!(matches!(
        activate(&ctx, marked, "nolink").await,
        Err(LinkError::Rejected(LinkRejection::NotFound))
    ));
    assert!(matches!(
        activate(&ctx, marked, &off.code).await,
        Err(LinkError::Rejected(LinkRejection::Disabled))
    ));
    assert!(matches!(
        activate(&ctx, marked, &old.code).await,
        Err(LinkError::Rejected(LinkRejection::Expired))
    ));
    core.refuse(Some(GrantRefusal::new(
        "grant.has_subscription",
        "У вас уже есть действующая подписка",
    )));
    match activate(&ctx, marked, &fine.code).await {
        Err(LinkError::Rejected(LinkRejection::Grant(refusal))) => {
            assert_eq!(refusal.code, "grant.has_subscription");
        }
        other => panic!("ожидали отказ выдачи: {other:?}"),
    }
    assert!(antibot_actions(&db.store, marked).await.is_empty());
    core.refuse(None);
    asks(activate(&ctx, marked, &fine.code).await);
    assert!(core.grants().is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn в_режиме_запрета_подписки_по_ссылке_нет_пока_отметку_не_сняли() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    mode(&db.store, "block").await;
    let link = create(&ctx, &sample("Блогер"), None).await.unwrap().link;
    let honest = client(&db.store).await;
    let marked = client(&db.store).await;
    suspect(&db.store, marked).await;
    for _ in 0..2 {
        match activate(&ctx, marked, &link.code).await {
            Err(LinkError::Rejected(rejection)) => {
                assert_eq!(rejection, LinkRejection::Denied);
                assert_eq!(rejection.code(), antibot::DENIED_CODE);
                assert!(rejection.message().contains("недоступна"));
            }
            other => panic!("ожидали отказ антибота: {other:?}"),
        }
    }
    assert!(core.grants().is_empty());
    assert_eq!(
        kinds(&db.store, marked, JournalFilter::Promo).await,
        ["promo_link_denied:\"promo_link\""]
    );
    activate(&ctx, honest, &link.code).await.unwrap();
    assert_eq!(core.grants().len(), 1);
    assert!(antibot::clear(&ctx, marked, None).await.unwrap());
    activate(&ctx, marked, &link.code).await.unwrap();
    assert_eq!(core.grants().len(), 2);
    assert_eq!(
        antibot_actions(&db.store, marked).await,
        ["promo_link_denied", "cleared"]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn в_режиме_отметки_и_без_антибота_ссылка_выдаётся_сразу() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let link = create(&ctx, &sample("Блогер"), None).await.unwrap().link;
    let first = client(&db.store).await;
    let second = client(&db.store).await;
    let third = client(&db.store).await;
    for who in [first, second, third] {
        suspect(&db.store, who).await;
    }
    mode(&db.store, "mark").await;
    activate(&ctx, first, &link.code).await.unwrap();
    mode(&db.store, "block").await;
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query("UPDATE module_states SET enabled = false WHERE module = 'antibot'")
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    activate(&ctx, second, &link.code).await.unwrap();
    mode(&db.store, "confirm").await;
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query("UPDATE antibot_marks SET confirmed_at = now() WHERE client_id = $1")
        .bind(third.0)
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    activate(&ctx, third, &link.code).await.unwrap();
    assert_eq!(core.grants().len(), 3);
    for who in [first, second, third] {
        assert!(antibot_actions(&db.store, who).await.is_empty());
    }
    db.cleanup().await;
}
