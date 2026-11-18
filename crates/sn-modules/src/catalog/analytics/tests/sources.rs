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

#[tokio::test]
async fn воронка_метки_считает_пробные_подключения_и_оплаты_от_пришедших() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    let seed = &s.seed;
    let label = Some(("blogger_ivan", "nov"));
    let only = seed.client(utc(2026, 3, 2, 10), "bot", label).await;
    let tried = seed.client(utc(2026, 3, 3, 10), "bot", label).await;
    let tried_connected = seed.client(utc(2026, 3, 4, 10), "bot", label).await;
    let paid_direct = seed.client(utc(2026, 3, 5, 10), "bot", label).await;
    let full = seed.client(utc(2026, 3, 6, 10), "bot", label).await;
    for (client, day) in [(tried, 3), (tried_connected, 4), (full, 6)] {
        seed.buy(Buy {
            client,
            kind: "trial",
            at: utc(2026, 3, day, 11),
            ..Buy::default()
        })
        .await;
    }
    for (client, day) in [(paid_direct, 7), (full, 9)] {
        let sub = seed.subscription(client, "paid", Some(s.start)).await;
        seed.buy(Buy {
            client,
            subscription: Some(sub),
            kind: "new",
            at: utc(2026, 3, day, 9),
            tariff: Some(s.start),
            total: 30_000,
            amount: 30_000,
            ..Buy::default()
        })
        .await;
    }
    let connected_sub = seed.subscription(tried_connected, "trial", None).await;
    let only_sub = seed.subscription(only, "trial", None).await;
    seed.exec(&format!(
        "UPDATE subscriptions SET first_connected_at = '2026-03-04 12:00+00' WHERE id = {connected_sub}; \
         UPDATE subscriptions SET first_connected_at = '2026-03-08 12:00+00' WHERE client_id IN ({paid_direct}, {full}); \
         INSERT INTO utm_links (title, source, created_at) VALUES ('Реклама в каналах', 'tg_ads_nov', '2026-04-10 10:00+00')"
    ))
    .await;
    let r = ctx_report(&db.store, "sources", month(3, Compare::None)).await;
    let (columns, rows) = match &metric(&r, "sources").detail {
        Detail::Table { columns, rows, .. } => (columns.clone(), rows.clone()),
        other => panic!("ждали таблицу, а пришло {other:?}"),
    };
    let titles: Vec<&str> = columns.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Перешли",
            "Пришли",
            "Попробовали",
            "Подключились",
            "Оплатили",
            "Выручка за период",
            "LTV",
            "Платят через месяц"
        ]
    );
    assert_eq!(columns[2].share_of, Some(1));
    assert_eq!(columns[3].share_of, Some(1));
    assert_eq!(columns[4].share_of, Some(1));
    let ivan = rows.iter().find(|r| r.label == "blogger_ivan").unwrap();
    assert_eq!(
        ivan.cells[..5],
        [Some(0.0), Some(5.0), Some(3.0), Some(3.0), Some(2.0)]
    );
    assert_eq!(ivan.cells[5], Some(60_000.0));
    assert_eq!(ivan.sub.as_deref(), Some("UTM-метка · идёт 44 дня"));
    let fresh = rows.iter().find(|r| r.label == "tg_ads_nov").unwrap();
    assert_eq!(
        fresh.cells[1..5],
        [Some(0.0), Some(0.0), Some(0.0), Some(0.0)]
    );
    assert_eq!(
        fresh.sub.as_deref(),
        Some("UTM-метка · идёт 5 дней · рано судить")
    );
    let all = Filters::default();
    let ivan_stat = || async {
        let found = stats(
            &db.store,
            utc(2026, 2, 28, 21),
            utc(2026, 3, 31, 21),
            now(),
            &all,
        )
        .await
        .unwrap();
        let x = found
            .into_iter()
            .find(|x| x.source_key == "blogger_ivan")
            .unwrap();
        (x.registrations, x.trials, x.connected, x.paid)
    };
    assert_eq!(ivan_stat().await, (5, 3, 3, 2));
    seed.exec(&format!(
        "UPDATE subscriptions SET first_connected_at = now() WHERE id = {only_sub}"
    ))
    .await;
    spans::refresh(&db.store, Currency::Rub, None)
        .await
        .unwrap();
    assert_eq!(ivan_stat().await, (5, 3, 4, 2));
}
