use super::super::sources::stats;
use super::money::scene;
use super::*;
use sn_store::testing::test_store;

#[tokio::test]
async fn источники_регистрации_оплаты_и_переходы() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    s.seed
        .exec(
            "INSERT INTO utm_clicks (at, channel, source, campaign, clicks) VALUES \
               ('2026-03-24 09:00+00', 'bot', 'vk', 'games', 10), \
               ('2026-03-24 10:00+00', 'site', 'vk', 'games', 4), \
               ('2026-03-25 10:00+00', 'bot', 'tiktok', 'osen', 5), \
               ('2026-02-25 10:00+00', 'bot', 'tiktok', 'osen', 7)",
        )
        .await;
    let r = ctx_report(&db.store, "sources", month(3, Compare::Prev)).await;
    assert_eq!(value(&r, "sources"), 4.0);
    assert_eq!(metric(&r, "sources").prev, Some(2.0));
    assert_eq!(value(&r, "conversion"), 75.0);
    assert_eq!(value(&r, "utm"), 1.0);
    assert_eq!(value(&r, "unlabeled"), 3.0);
    assert_eq!(value(&r, "clicks"), 19.0);
    spans::refresh(&db.store, Currency::Rub, None)
        .await
        .unwrap();
    let found = stats(
        &db.store,
        utc(2026, 2, 28, 21),
        utc(2026, 3, 31, 21),
        now(),
        &Filters::default(),
    )
    .await
    .unwrap();
    let vk = found.iter().find(|x| x.source_key == "vk").unwrap();
    assert_eq!(
        (
            vk.registrations,
            vk.paid,
            vk.revenue,
            vk.revenue_all,
            vk.clicks
        ),
        (1, 1, 15_000, 15_000, 14)
    );
    let tiktok = found.iter().find(|x| x.source_key == "tiktok").unwrap();
    assert_eq!((tiktok.registrations, tiktok.clicks), (0, 5));
    let none = found.iter().find(|x| x.source_type == "none").unwrap();
    assert_eq!(
        (none.registrations, none.paid, none.revenue),
        (3, 2, 50_000)
    );
    let stat = |found: &[super::super::sources::SourceStat]| {
        let x = found.iter().find(|x| x.source_type == "none").unwrap();
        (x.registrations, x.trials, x.kept_base, x.kept)
    };
    let all = Filters::default();
    let march = || {
        stats(
            &db.store,
            utc(2026, 2, 28, 21),
            utc(2026, 3, 31, 21),
            now(),
            &all,
        )
    };
    let before = stat(&march().await.unwrap());
    let g = s.seed.client(utc(2026, 3, 5, 10), "bot", None).await;
    let sg = s.seed.subscription(g, "paid", Some(s.start)).await;
    s.seed
        .buy(Buy {
            client: g,
            kind: "trial",
            at: utc(2026, 3, 5, 11),
            ..Buy::default()
        })
        .await;
    s.seed
        .buy(Buy {
            client: g,
            subscription: Some(sg),
            kind: "new",
            at: utc(2026, 3, 6, 9),
            tariff: Some(s.start),
            total: 30_000,
            amount: 30_000,
            ..Buy::default()
        })
        .await;
    spans::refresh(&db.store, Currency::Rub, None)
        .await
        .unwrap();
    let after = stat(&march().await.unwrap());
    assert_eq!(after, (before.0 + 1, before.1 + 1, before.2 + 1, before.3));
    s.seed
        .buy(Buy {
            client: g,
            subscription: Some(sg),
            kind: "renewal",
            at: utc(2026, 4, 1, 9),
            tariff: Some(s.start),
            total: 30_000,
            amount: 30_000,
            ..Buy::default()
        })
        .await;
    spans::refresh(&db.store, Currency::Rub, None)
        .await
        .unwrap();
    assert_eq!(stat(&march().await.unwrap()).3, before.3 + 1);
    let q = filtered(month(2, Compare::None), |q| {
        q.source = Some("utm:tiktok/osen".into())
    });
    let r = ctx_report(&db.store, "sources", q).await;
    assert_eq!(value(&r, "sources"), 1.0);
    assert_eq!(value(&r, "clicks"), 7.0);
    let _ = s.a;
}
