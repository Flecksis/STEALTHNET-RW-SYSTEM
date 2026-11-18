use super::money::scene;
use super::*;
use super::super::sources::stats;
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
    spans::refresh(&db.store, Currency::Rub, None).await.unwrap();
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
        (vk.registrations, vk.paid, vk.revenue, vk.revenue_all, vk.clicks),
        (1, 1, 15_000, 15_000, 14)
    );
    let tiktok = found.iter().find(|x| x.source_key == "tiktok").unwrap();
    assert_eq!((tiktok.registrations, tiktok.clicks), (0, 5));
    let none = found.iter().find(|x| x.source_type == "none").unwrap();
    assert_eq!((none.registrations, none.paid, none.revenue), (3, 2, 50_000));
    let q = filtered(month(2, Compare::None), |q| q.source = Some("utm:tiktok/osen".into()));
    let r = ctx_report(&db.store, "sources", q).await;
    assert_eq!(value(&r, "sources"), 1.0);
    assert_eq!(value(&r, "clicks"), 7.0);
    let _ = s.a;
}
