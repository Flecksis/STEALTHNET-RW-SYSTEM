use super::*;
use sn_store::testing::test_store;

pub struct Scene {
    pub seed: Seed,
    pub start: i64,
    pub pro: i64,
    pub a: i64,
    pub b: i64,
    pub c: i64,
    pub d: i64,
    pub e: i64,
    pub f: i64,
}

pub async fn scene(store: Store) -> Scene {
    let seed = Seed { store };
    let cat = seed.category("Основные").await;
    let start = seed.tariff(cat, "Старт").await;
    let pro = seed.tariff(cat, "Про").await;
    let a = seed.client(utc(2026, 2, 25, 10), "bot", Some(("tiktok", "osen"))).await;
    let b = seed.client(utc(2026, 2, 1, 10), "web", None).await;
    let c = seed.client(utc(2026, 3, 14, 10), "miniapp", None).await;
    let d = seed.client(utc(2026, 3, 19, 10), "bot", None).await;
    let e = seed.client(utc(2026, 3, 24, 10), "bot", Some(("vk", "games"))).await;
    let f = seed.client(utc(2026, 3, 3, 10), "bot", None).await;
    seed.identity(a, "telegram", "1").await;
    seed.identity(b, "email", "b@example.com").await;
    let sa = seed.subscription(a, "paid", Some(start)).await;
    let sb = seed.subscription(b, "paid", Some(pro)).await;
    let sc = seed.subscription(c, "paid", Some(start)).await;
    let sd = seed.subscription(d, "paid", Some(start)).await;
    seed.exec(&format!(
        "INSERT INTO subscriptions (client_id, kind, created_at, expire_at) VALUES ({f}, 'trial', '2026-03-03 10:00+00', '2026-03-06 10:00+00')"
    ))
    .await;
    let (_, pa) = seed
        .buy(Buy {
            client: a,
            subscription: Some(sa),
            kind: "new",
            at: utc(2026, 3, 2, 9),
            tariff: Some(start),
            total: 25_000,
            promo: 5_000,
            method: Some("sbp"),
            amount: 25_000,
            channel: Some("bot"),
            ..Buy::default()
        })
        .await;
    seed.buy(Buy {
        client: b,
        subscription: Some(sb),
        kind: "new",
        at: utc(2026, 2, 10, 9),
        tariff: Some(pro),
        total: 99_000,
        provider: "platega",
        method: Some("card"),
        amount: 99_000,
        channel: Some("cabinet"),
        ..Buy::default()
    })
    .await;
    let (_, pb) = seed
        .buy(Buy {
            client: b,
            subscription: Some(sb),
            kind: "renewal",
            at: utc(2026, 3, 12, 9),
            tariff: Some(pro),
            total: 99_000,
            provider: "platega",
            method: Some("card"),
            amount: 99_000,
            channel: Some("cabinet"),
            ..Buy::default()
        })
        .await;
    seed.buy(Buy {
        client: c,
        kind: "topup",
        at: utc(2026, 3, 15, 9),
        days: 0,
        total: 50_000,
        method: Some("sbp"),
        amount: 50_000,
        channel: Some("miniapp"),
        intent: Some(json!({"type": "top_up", "amount": {"minor": 50_000, "currency": "RUB"}})),
        ..Buy::default()
    })
    .await;
    seed.buy(Buy {
        client: c,
        subscription: Some(sc),
        kind: "new",
        at: utc(2026, 3, 16, 9),
        tariff: Some(start),
        total: 30_000,
        provider: "balance",
        amount: 30_000,
        channel: Some("miniapp"),
        ..Buy::default()
    })
    .await;
    seed.buy(Buy {
        client: d,
        subscription: Some(sd),
        kind: "new",
        at: utc(2026, 3, 20, 9),
        tariff: Some(start),
        total: 30_000,
        provider: "stars",
        currency: "XTR",
        amount: 230,
        channel: Some("bot"),
        ..Buy::default()
    })
    .await;
    let (_, pe) = seed
        .buy(Buy {
            client: e,
            kind: "module",
            at: utc(2026, 3, 25, 9),
            days: 0,
            total: 15_000,
            provider: "cryptopay",
            amount: 15_000,
            channel: Some("bot"),
            intent: Some(json!({"type": "module", "module": "options", "kind": "devices", "payload": {}})),
            ..Buy::default()
        })
        .await;
    seed.exec(&format!(
        "UPDATE payments SET refunded_minor = 10000, status = 'partially_refunded' WHERE id = {pb}; \
         INSERT INTO refunds (payment_id, amount_minor, currency, method, subscription_action, created_at) \
           VALUES ({pb}, 10000, 'RUB', 'provider', 'keep', '2026-03-20 10:00+00'); \
         INSERT INTO payment_notices (provider, event_key, kind, payment_id, client_id, amount_minor, currency, text, created_at) \
           VALUES ('cryptopay', 'd1', 'dispute', {pe}, {e}, 15000, 'RUB', 'спор', '2026-03-26 10:00+00'); \
         INSERT INTO referral_credits (payment_id, level, referrer_id, referral_id, base_minor, percent, amount_minor, currency, credited_at) \
           VALUES ({pa}, 1, {b}, {a}, 25000, 8, 2000, 'RUB', '2026-03-05 10:00+00')"
    ))
    .await;
    Scene {
        seed,
        start,
        pro,
        a,
        b,
        c,
        d,
        e,
        f,
    }
}

#[tokio::test]
async fn выручка_разбивки_и_средние_за_месяц() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    let r = ctx_report(&db.store, "money", month(3, Compare::Prev)).await;
    assert_eq!(value(&r, "revenue"), 189_000.0);
    assert_eq!(metric(&r, "revenue").prev, Some(99_000.0));
    assert_eq!(value(&r, "net"), 179_000.0);
    assert_eq!(value(&r, "check"), 47_250.0);
    assert_eq!(value(&r, "stars"), 230.0);
    assert_eq!(value(&r, "promo"), 5_000.0);
    assert_eq!(value(&r, "crypto"), 15_000.0);
    assert_eq!(value(&r, "balance"), 30_000.0);
    let kinds = breakdown(&r, "revenue", "kind");
    assert_eq!(
        kinds,
        vec![
            ("renewal".to_string(), 99_000.0),
            ("topup".to_string(), 50_000.0),
            ("new".to_string(), 25_000.0),
            ("devices".to_string(), 15_000.0),
        ]
    );
    let providers = breakdown(&r, "revenue", "provider");
    assert_eq!(
        providers,
        vec![
            ("platega".to_string(), 99_000.0),
            ("yookassa".to_string(), 75_000.0),
            ("cryptopay".to_string(), 15_000.0),
        ]
    );
    let channels = breakdown(&r, "revenue", "channel");
    assert_eq!(
        channels,
        vec![
            ("cabinet".to_string(), 99_000.0),
            ("miniapp".to_string(), 50_000.0),
            ("bot".to_string(), 40_000.0),
        ]
    );
    let tariffs = breakdown(&r, "revenue", "tariff");
    assert_eq!(
        tariffs,
        vec![
            (s.pro.to_string(), 99_000.0),
            ("none".to_string(), 65_000.0),
            (s.start.to_string(), 25_000.0),
        ]
    );
    let periods = breakdown(&r, "revenue", "period");
    assert_eq!(periods, vec![("30".to_string(), 124_000.0), ("none".to_string(), 65_000.0)]);
    assert_eq!(value(&r, "arpu"), 37_800.0);
    assert_eq!(value(&r, "arppu"), 47_250.0);
    assert_eq!(value(&r, "ltv"), 69_500.0);
    assert_eq!(value(&r, "referral"), 2_000.0);
    assert_eq!(value(&r, "refunds"), 25_000.0);
    let _ = (s.a, s.b, s.c, s.d, s.e, s.f);
}

#[tokio::test]
async fn mrr_и_его_движение() {
    let Some(db) = test_store().await else {
        return;
    };
    let _s = scene(db.store.clone()).await;
    let r = ctx_report(&db.store, "money", month(3, Compare::None)).await;
    assert_eq!(value(&r, "mrr"), 184_000.0);
    assert_eq!(value(&r, "mrr_moves"), 85_000.0);
    let rows = table(&r, "mrr_moves");
    assert_eq!(rows[0], ("Новые".to_string(), vec![Some(85_000.0), Some(3.0)]));
    assert_eq!(rows[3], ("Ушли".to_string(), vec![Some(-0.0), Some(0.0)]));
    let r = ctx_report(&db.store, "money", month(4, Compare::None)).await;
    let rows = table(&r, "mrr_moves");
    assert_eq!(rows[3].1[1], Some(3.0));
}

#[tokio::test]
async fn фильтры_сужают_выручку() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    let q = filtered(month(3, Compare::None), |q| q.provider = Some("yookassa".into()));
    let r = ctx_report(&db.store, "money", q).await;
    assert_eq!(value(&r, "revenue"), 75_000.0);
    let q = filtered(month(3, Compare::None), |q| q.source = Some("utm:tiktok".into()));
    let r = ctx_report(&db.store, "money", q).await;
    assert_eq!(value(&r, "revenue"), 25_000.0);
    let q = filtered(month(3, Compare::None), |q| q.tariff = Some(s.pro));
    let r = ctx_report(&db.store, "money", q).await;
    assert_eq!(value(&r, "revenue"), 99_000.0);
    assert_eq!(value(&r, "mrr"), 99_000.0);
    let q = filtered(month(3, Compare::None), |q| q.channel = Some("bot".into()));
    let r = ctx_report(&db.store, "money", q).await;
    assert_eq!(value(&r, "revenue"), 40_000.0);
    assert_eq!(metric(&r, "mrr").note.as_deref(), Some("без фильтра по каналу"));
}

#[tokio::test]
async fn прогноз_складывает_полученное_и_ожидаемое() {
    let Some(db) = test_store().await else {
        return;
    };
    let _s = scene(db.store.clone()).await;
    let r = ctx_report(&db.store, "money", month(4, Compare::None)).await;
    let rows = table(&r, "forecast");
    assert_eq!(rows[0].1[0], Some(0.0));
    let total: f64 = rows[..3].iter().map(|r| r.1[0].unwrap()).sum();
    assert_eq!(rows[3].1[0], Some(total));
    assert_eq!(metric(&r, "forecast").title, "Прогноз на апрель");
}
