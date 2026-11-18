use super::money::scene;
use super::*;
use sn_store::testing::test_store;

#[tokio::test]
async fn продления_уход_возврат_и_когорты() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    let seed = &s.seed;
    let g = seed.client(utc(2026, 1, 4, 10), "bot", None).await;
    let sg = seed.subscription(g, "paid", Some(s.start)).await;
    for at in [utc(2026, 1, 5, 9), utc(2026, 3, 10, 9)] {
        seed.buy(Buy {
            client: g,
            subscription: Some(sg),
            kind: if at.month() == 1 { "new" } else { "renewal" },
            at,
            tariff: Some(s.start),
            total: 30_000,
            amount: 30_000,
            ..Buy::default()
        })
        .await;
    }
    let sa: i64 = seed
        .scalar(&format!(
            "SELECT id FROM subscriptions WHERE client_id = {}",
            s.a
        ))
        .await;
    seed.buy(Buy {
        client: s.a,
        subscription: Some(sa),
        kind: "renewal",
        at: utc(2026, 4, 3, 9),
        tariff: Some(s.start),
        total: 25_000,
        amount: 25_000,
        ..Buy::default()
    })
    .await;
    seed.exec(&format!(
        "UPDATE subscriptions SET autorenew_enabled = true, expire_at = '2026-04-20 12:00+00' WHERE client_id = {b}; \
         UPDATE subscriptions SET expire_at = '2026-05-05 12:00+00' WHERE client_id = {c}; \
         INSERT INTO autorenew_attempts (subscription_id, period_key, attempt_no, result, created_at) \
           SELECT id, 'p1', 1, 'charged', '2026-03-12 08:00+00' FROM subscriptions WHERE client_id = {b}; \
         INSERT INTO autorenew_attempts (subscription_id, period_key, attempt_no, result, created_at) \
           SELECT id, 'p2', 1, 'insufficient', '2026-03-13 08:00+00' FROM subscriptions WHERE client_id = {b}",
        b = s.b,
        c = s.c
    ))
    .await;
    let r = ctx_report(&db.store, "retention", month(3, Compare::None)).await;
    assert_eq!(value(&r, "on_time"), 50.0);
    assert_eq!(value(&r, "after_break"), 50.0);
    assert_eq!(value(&r, "late"), 0.0);
    assert_eq!(value(&r, "returned"), 1.0);
    assert_eq!(value(&r, "autorenew_ok"), 50.0);
    assert_eq!(value(&r, "autorenew_fail"), 1.0);
    assert_eq!(value(&r, "autorenew_on"), 20.0);
    assert_eq!(value(&r, "expiring7"), 1.0);
    assert_eq!(value(&r, "expiring30"), 2.0);
    assert!(metric(&r, "kept1").value.is_none());
    let r = ctx_report(&db.store, "retention", month(4, Compare::None)).await;
    assert_eq!(value(&r, "late"), 100.0);
    assert_eq!(value(&r, "churned"), 2.0);
    let q = TabQuery {
        from: Some(day(2026, 1, 1)),
        to: Some(day(2026, 3, 31)),
        compare: Some(Compare::None),
        ..TabQuery::default()
    };
    let r = ctx_report(&db.store, "retention", q).await;
    assert_eq!(value(&r, "kept1"), 50.0);
    match &metric(&r, "cohorts").detail {
        Detail::Cohorts { rows, .. } => {
            let feb = rows.iter().find(|r| r.month == day(2026, 2, 1)).unwrap();
            assert_eq!(feb.size, 1);
            assert_eq!(feb.kept[0], Some(1));
            assert_eq!(feb.kept[1], None);
            assert_eq!(feb.money[0], Some(99_000));
            let jan = rows.iter().find(|r| r.month == day(2026, 1, 1)).unwrap();
            assert_eq!(jan.kept[0], Some(0));
            assert_eq!(jan.kept[1], Some(0));
        }
        other => panic!("ждали когорты, а пришло {other:?}"),
    }
}

use chrono::Datelike;
