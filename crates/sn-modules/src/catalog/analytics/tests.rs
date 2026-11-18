use super::report::{self, Month, Params, Report, Source};
use super::spans;
use super::{ReportQuery, period};
use crate::context::{MethodTitle, PaymentTitle};
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use sn_domain::money::Currency;
use sn_store::testing::test_store;
use sn_store::{Db, Store};

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

fn now() -> DateTime<Utc> {
    utc(2026, 4, 15, 12, 0)
}

fn msk() -> FixedOffset {
    FixedOffset::east_opt(3 * 3600).unwrap()
}

struct Seed {
    store: Store,
}

impl Seed {
    async fn exec(&self, sql: &str) {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::query(sql).execute(conn.conn()).await.unwrap();
    }

    async fn client(&self, created: DateTime<Utc>, source: Option<&str>) -> i64 {
        let mut conn = self.store.conn().await.unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO clients (registration_source, created_at) VALUES ('bot', $1) RETURNING id",
        )
        .bind(created)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        if let Some(source) = source {
            sqlx::query(
                "INSERT INTO client_utm (client_id, source, start_param, registration_source, created_at) \
                 VALUES ($1, $2, $3, 'bot', $4)",
            )
            .bind(id)
            .bind(source)
            .bind(format!("s_{source}"))
            .bind(created)
            .execute(conn.conn())
            .await
            .unwrap();
        }
        id
    }

    async fn subscription(&self, client: i64) -> i64 {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::query_scalar(
            "INSERT INTO subscriptions (client_id, kind) VALUES ($1, 'paid') RETURNING id",
        )
        .bind(client)
        .fetch_one(conn.conn())
        .await
        .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    async fn order(
        &self,
        client: i64,
        subscription: Option<i64>,
        kind: &str,
        status: &str,
        paid_at: Option<DateTime<Utc>>,
        days: i64,
        total: i64,
        stars: Option<i64>,
    ) -> i64 {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::query_scalar(
            "INSERT INTO orders (client_id, subscription_id, kind, status, intent, quote, currency, total_minor, stars_total, created_at, paid_at) \
             VALUES ($1, $2, $3, $4, '{}'::jsonb, jsonb_build_object('days', $5::bigint), 'RUB', $6, $7, COALESCE($8, now()), $8) RETURNING id",
        )
        .bind(client)
        .bind(subscription)
        .bind(kind)
        .bind(status)
        .bind(days)
        .bind(total)
        .bind(stars)
        .bind(paid_at)
        .fetch_one(conn.conn())
        .await
        .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    async fn payment(
        &self,
        order: i64,
        client: i64,
        provider: &str,
        method: Option<&str>,
        amount: i64,
        currency: &str,
        status: &str,
        created: DateTime<Utc>,
        paid_at: Option<DateTime<Utc>>,
        refunded: i64,
        expires: Option<DateTime<Utc>>,
    ) {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::query(
            "INSERT INTO payments (order_id, client_id, provider, pay_method, amount_minor, currency, status, created_at, paid_at, refunded_minor, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(order)
        .bind(client)
        .bind(provider)
        .bind(method)
        .bind(amount)
        .bind(currency)
        .bind(status)
        .bind(created)
        .bind(paid_at)
        .bind(refunded)
        .bind(expires)
        .execute(conn.conn())
        .await
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    async fn buy(
        &self,
        client: i64,
        subscription: i64,
        kind: &str,
        at: DateTime<Utc>,
        days: i64,
        total: i64,
        provider: &str,
        method: Option<&str>,
    ) -> i64 {
        let order = self
            .order(
                client,
                Some(subscription),
                kind,
                "fulfilled",
                Some(at),
                days,
                total,
                None,
            )
            .await;
        self.payment(
            order,
            client,
            provider,
            method,
            total,
            "RUB",
            "paid",
            at,
            Some(at),
            0,
            None,
        )
        .await;
        order
    }

    async fn trial(&self, client: i64, at: DateTime<Utc>) {
        self.order(client, None, "trial", "fulfilled", Some(at), 3, 0, None)
            .await;
    }
}

struct Scene {
    a: i64,
}

async fn scene(store: &Store) -> Scene {
    let s = Seed {
        store: store.clone(),
    };
    s.exec("INSERT INTO settings (key, value) VALUES ('core.currency', '\"RUB\"'), ('remnawave.utc_offset_minutes', '180')")
        .await;

    let a = s.client(utc(2026, 1, 2, 9, 0), Some("tiktok")).await;
    s.trial(a, utc(2026, 1, 2, 10, 0)).await;
    let a1 = s.subscription(a).await;
    s.buy(
        a,
        a1,
        "new",
        utc(2026, 1, 5, 10, 0),
        30,
        30_000,
        "yookassa",
        Some("sbp"),
    )
    .await;
    s.buy(
        a,
        a1,
        "renewal",
        utc(2026, 2, 1, 10, 0),
        30,
        30_000,
        "yookassa",
        Some("bank_card"),
    )
    .await;
    let topup = s
        .order(
            a,
            None,
            "topup",
            "fulfilled",
            Some(utc(2026, 3, 1, 9, 0)),
            0,
            50_000,
            None,
        )
        .await;
    s.payment(
        topup,
        a,
        "yookassa",
        Some("sbp"),
        50_000,
        "RUB",
        "paid",
        utc(2026, 3, 1, 9, 0),
        Some(utc(2026, 3, 1, 9, 0)),
        0,
        None,
    )
    .await;
    s.buy(
        a,
        a1,
        "renewal",
        utc(2026, 3, 5, 10, 0),
        30,
        30_000,
        "balance",
        None,
    )
    .await;

    let b = s.client(utc(2026, 1, 20, 9, 0), None).await;
    let b1 = s.subscription(b).await;
    let b_order = s
        .order(
            b,
            Some(b1),
            "new",
            "fulfilled",
            Some(utc(2026, 1, 31, 22, 30)),
            90,
            80_000,
            None,
        )
        .await;
    s.payment(
        b_order,
        b,
        "cryptopay",
        None,
        80_000,
        "RUB",
        "partially_refunded",
        utc(2026, 1, 31, 22, 30),
        Some(utc(2026, 1, 31, 22, 30)),
        10_000,
        None,
    )
    .await;

    let c = s.client(utc(2026, 2, 10, 7, 0), Some("tiktok")).await;
    s.trial(c, utc(2026, 2, 10, 8, 0)).await;
    let c1 = s.subscription(c).await;
    let c_stars = s
        .order(
            c,
            Some(c1),
            "new",
            "fulfilled",
            Some(utc(2026, 2, 12, 8, 0)),
            30,
            30_000,
            Some(350),
        )
        .await;
    s.payment(
        c_stars,
        c,
        "stars",
        None,
        350,
        "XTR",
        "paid",
        utc(2026, 2, 12, 8, 0),
        Some(utc(2026, 2, 12, 8, 0)),
        0,
        None,
    )
    .await;
    let c2 = s.subscription(c).await;
    s.buy(
        c,
        c2,
        "new",
        utc(2026, 4, 1, 8, 0),
        30,
        30_000,
        "yookassa",
        Some("sbp"),
    )
    .await;

    let d = s.client(utc(2026, 3, 1, 11, 0), None).await;
    s.trial(d, utc(2026, 3, 1, 12, 0)).await;
    let failed = s
        .order(d, None, "new", "canceled", None, 30, 30_000, None)
        .await;
    s.payment(
        failed,
        d,
        "yookassa",
        Some("bank_card"),
        30_000,
        "RUB",
        "failed",
        utc(2026, 3, 1, 12, 30),
        None,
        0,
        None,
    )
    .await;
    let waiting = s
        .order(d, None, "new", "pending", None, 30, 30_000, None)
        .await;
    s.payment(
        waiting,
        d,
        "cryptopay",
        None,
        30_000,
        "RUB",
        "pending",
        utc(2026, 3, 2, 12, 0),
        None,
        0,
        Some(utc(2026, 3, 2, 13, 0)),
    )
    .await;

    let e = s.client(utc(2025, 12, 20, 9, 0), Some("vk")).await;
    let e1 = s.subscription(e).await;
    s.buy(
        e,
        e1,
        "new",
        utc(2025, 12, 20, 10, 0),
        30,
        30_000,
        "yookassa",
        Some("sbp"),
    )
    .await;
    s.buy(
        e,
        e1,
        "renewal",
        utc(2026, 1, 19, 9, 0),
        30,
        30_000,
        "yookassa",
        Some("sbp"),
    )
    .await;
    s.buy(
        e,
        e1,
        "change",
        utc(2026, 2, 10, 10, 0),
        90,
        80_000,
        "yookassa",
        Some("bank_card"),
    )
    .await;

    let f = s.client(utc(2026, 4, 2, 9, 0), None).await;
    let free = s
        .order(
            f,
            None,
            "new",
            "fulfilled",
            Some(utc(2026, 4, 2, 9, 30)),
            30,
            0,
            None,
        )
        .await;
    s.payment(
        free,
        f,
        "manual",
        None,
        0,
        "RUB",
        "paid",
        utc(2026, 4, 2, 9, 30),
        Some(utc(2026, 4, 2, 9, 30)),
        0,
        None,
    )
    .await;

    Scene { a }
}

fn titles() -> Vec<PaymentTitle> {
    vec![PaymentTitle {
        provider: "yookassa".into(),
        title: "ЮKassa".into(),
        button: "Банковская карта".into(),
        methods: vec![
            MethodTitle {
                code: "sbp".into(),
                title: "СБП".into(),
            },
            MethodTitle {
                code: "bank_card".into(),
                title: "Карта".into(),
            },
        ],
    }]
}

async fn report_of(store: &Store, source: Source) -> Report {
    spans::refresh(store, Currency::Rub, None).await.unwrap();
    let mut conn = store.conn().await.unwrap();
    let offset = super::offset(&mut conn).await.unwrap();
    assert_eq!(offset, msk());
    drop(conn);
    report::build(
        store,
        &Params {
            currency: Currency::Rub,
            offset,
            from: Month::parse("2026-01").unwrap(),
            to: Month::parse("2026-04").unwrap(),
            now: now(),
            source,
        },
        &titles(),
    )
    .await
    .unwrap()
}

fn column<T>(report: &Report, pick: impl Fn(&report::MonthRow) -> T) -> Vec<T> {
    report.months.iter().map(pick).collect()
}

#[tokio::test]
async fn деньги_и_mrr_по_месяцам_в_часовом_поясе_установки() {
    let Some(db) = test_store().await else {
        return;
    };
    scene(&db.store).await;
    let r = report_of(&db.store, Source::All).await;
    assert_eq!(
        column(&r, |m| m.month.clone()),
        ["2026-01", "2026-02", "2026-03", "2026-04"]
    );
    assert_eq!(column(&r, |m| m.revenue), [60_000, 180_000, 50_000, 30_000]);
    assert_eq!(column(&r, |m| m.stars), [0, 350, 0, 0]);
    assert_eq!(column(&r, |m| m.refunds), [0, 10_000, 0, 0]);
    assert_eq!(column(&r, |m| m.payments), [2, 3, 1, 1]);
    assert_eq!(column(&r, |m| m.mrr), [60_000, 113_334, 83_334, 83_334]);
    assert_eq!(r.totals.revenue, 320_000);
    assert_eq!(r.totals.payments, 7);
    assert_eq!(r.totals.mrr, 83_334);
    assert_eq!((r.totals.ltv_clients, r.totals.ltv), (4, 87_500));
    db.cleanup().await;
}

#[tokio::test]
async fn клиенты_новые_продлили_вернулись_ушли_и_пробный() {
    let Some(db) = test_store().await else {
        return;
    };
    scene(&db.store).await;
    let r = report_of(&db.store, Source::All).await;
    assert_eq!(column(&r, |m| m.new), [1, 2, 0, 0]);
    assert_eq!(column(&r, |m| m.renewed), [1, 2, 1, 0]);
    assert_eq!(column(&r, |m| m.returned), [0, 0, 0, 1]);
    assert_eq!(column(&r, |m| m.churned), [0, 0, 1, 1]);
    assert_eq!(column(&r, |m| m.paying), [2, 4, 3, 3]);
    assert_eq!(column(&r, |m| m.trials), [1, 1, 1, 0]);
    assert_eq!(column(&r, |m| m.trials_bought), [1, 1, 0, 0]);
    assert_eq!((r.totals.trials, r.totals.trials_bought), (3, 2));
    assert_eq!(r.totals.paying, 3);
    db.cleanup().await;
}

#[tokio::test]
async fn удержание_по_месяцу_первой_покупки() {
    let Some(db) = test_store().await else {
        return;
    };
    scene(&db.store).await;
    let r = report_of(&db.store, Source::All).await;
    let jan = &r.cohorts[0];
    assert_eq!(
        (jan.month.as_str(), jan.size, jan.revenue),
        ("2026-01", 1, 110_000)
    );
    assert_eq!(&jan.retained[..3], &[Some(1), Some(1), None]);
    let feb = &r.cohorts[1];
    assert_eq!((feb.size, feb.revenue), (2, 100_000));
    assert_eq!(&feb.retained[..2], &[Some(1), None]);
    assert!(r.cohorts[2].retained.iter().all(Option::is_none));
    assert_eq!(r.cohorts[2].size, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn платёжки_и_способы_оплаты_с_отказами_и_возвратами() {
    let Some(db) = test_store().await else {
        return;
    };
    scene(&db.store).await;
    let r = report_of(&db.store, Source::All).await;
    let names: Vec<&str> = r.providers.iter().map(|p| p.provider.as_str()).collect();
    assert_eq!(names, ["yookassa", "cryptopay", "stars"]);
    let yk = &r.providers[0];
    assert_eq!(yk.total.title, "ЮKassa");
    assert_eq!(
        (
            yk.total.revenue,
            yk.total.payments,
            yk.total.invoices,
            yk.total.failed
        ),
        (250_000, 6, 7, 1)
    );
    let methods: Vec<(&str, i64, u32, u32, u32)> = yk
        .methods
        .iter()
        .map(|m| {
            (
                m.title.as_str(),
                m.revenue,
                m.payments,
                m.invoices,
                m.failed,
            )
        })
        .collect();
    assert_eq!(
        methods,
        [("СБП", 140_000, 4, 4, 0), ("Карта", 110_000, 2, 3, 1)]
    );
    let crypto = &r.providers[1];
    assert_eq!(crypto.total.title, "cryptopay");
    assert_eq!(
        (
            crypto.total.revenue,
            crypto.total.payments,
            crypto.total.invoices,
            crypto.total.failed
        ),
        (70_000, 1, 2, 1)
    );
    assert_eq!((crypto.total.refunds, crypto.total.refunded), (1, 10_000));
    assert_eq!(crypto.methods.len(), 1);
    assert_eq!(crypto.methods[0].code, None);
    assert_eq!(crypto.methods[0].title, "Без выбора способа");
    let stars = &r.providers[2];
    assert_eq!(stars.total.title, "Звёзды Telegram");
    assert_eq!(
        (stars.total.revenue, stars.total.stars, stars.total.payments),
        (0, 350, 1)
    );
    assert!(
        r.providers
            .iter()
            .all(|p| p.provider != "manual" && p.provider != "balance")
    );
    db.cleanup().await;
}

#[tokio::test]
async fn источники_и_фильтр_по_источнику() {
    let Some(db) = test_store().await else {
        return;
    };
    scene(&db.store).await;
    let r = report_of(&db.store, Source::All).await;
    let rows: Vec<(Option<&str>, u32, u32, i64, i64)> = r
        .sources
        .iter()
        .map(|s| (s.source.as_deref(), s.clients, s.paying, s.revenue, s.stars))
        .collect();
    assert_eq!(
        rows,
        [
            (None, 3, 1, 70_000, 0),
            (Some("tiktok"), 2, 2, 140_000, 350)
        ]
    );
    assert_eq!(r.source_options, ["tiktok", "vk"]);

    let tiktok = report_of(&db.store, Source::Named("tiktok".into())).await;
    assert_eq!(
        column(&tiktok, |m| m.revenue),
        [30_000, 30_000, 50_000, 30_000]
    );
    assert_eq!(column(&tiktok, |m| m.stars), [0, 350, 0, 0]);
    assert_eq!(column(&tiktok, |m| m.new), [1, 1, 0, 0]);
    assert_eq!(column(&tiktok, |m| m.returned), [0, 0, 0, 1]);
    assert_eq!(column(&tiktok, |m| m.churned), [0, 0, 1, 1]);
    assert_eq!(tiktok.totals.paying, 1);
    assert_eq!((tiktok.totals.ltv_clients, tiktok.totals.ltv), (2, 70_000));
    assert_eq!(tiktok.sources.len(), 1);

    let unlabeled = report_of(&db.store, Source::Unlabeled).await;
    assert_eq!(unlabeled.totals.revenue, 70_000);
    assert_eq!(unlabeled.totals.paying, 1);
    assert_eq!(column(&unlabeled, |m| m.new), [0, 1, 0, 0]);
    assert_eq!(unlabeled.providers.len(), 2);
    db.cleanup().await;
}

#[tokio::test]
async fn новая_оплата_досчитывается_без_полного_пересчёта() {
    let Some(db) = test_store().await else {
        return;
    };
    let scene = scene(&db.store).await;
    let first = spans::refresh(&db.store, Currency::Rub, None)
        .await
        .unwrap();
    assert!(first.full);
    let again = spans::refresh(&db.store, Currency::Rub, None)
        .await
        .unwrap();
    assert!(!again.full);
    let seed = Seed {
        store: db.store.clone(),
    };
    let a1: i64 = {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query_scalar("SELECT id FROM subscriptions WHERE client_id = $1")
            .bind(scene.a)
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    seed.buy(
        scene.a,
        a1,
        "renewal",
        utc(2026, 4, 7, 10, 0),
        30,
        30_000,
        "yookassa",
        Some("sbp"),
    )
    .await;
    let r = report_of(&db.store, Source::All).await;
    assert_eq!(column(&r, |m| m.churned), [0, 0, 1, 0]);
    assert_eq!(column(&r, |m| m.renewed), [1, 2, 1, 1]);
    assert_eq!(r.totals.paying, 4);
    assert_eq!(r.totals.mrr, 113_334);
    let islands: i64 = {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query_scalar("SELECT count(*) FROM analytics_islands WHERE client_id = $1")
            .bind(scene.a)
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    assert_eq!(islands, 1);
    db.cleanup().await;
}

#[test]
fn период_по_умолчанию_все_время_и_ошибки() {
    let current = Month::parse("2026-04").unwrap();
    let first = Month::parse("2024-11");
    let default = period(&ReportQuery::default(), current, None).unwrap();
    assert_eq!(
        (default.0.label(), default.1.label()),
        ("2025-05".into(), "2026-04".into())
    );
    let three = ReportQuery {
        months: Some(3),
        ..ReportQuery::default()
    };
    assert_eq!(period(&three, current, None).unwrap().0.label(), "2026-02");
    let all = ReportQuery {
        all: Some(true),
        ..ReportQuery::default()
    };
    assert_eq!(period(&all, current, first).unwrap().0.label(), "2024-11");
    assert_eq!(period(&all, current, None).unwrap().0.label(), "2026-04");
    let future = ReportQuery {
        to: Some("2026-05".into()),
        ..ReportQuery::default()
    };
    assert!(period(&future, current, None).is_err());
    let reversed = ReportQuery {
        from: Some("2026-03".into()),
        to: Some("2026-01".into()),
        ..ReportQuery::default()
    };
    assert!(period(&reversed, current, None).is_err());
    let broken = ReportQuery {
        from: Some("2026-13".into()),
        ..ReportQuery::default()
    };
    assert!(period(&broken, current, None).is_err());
    let long = ReportQuery {
        from: Some("2001-01".into()),
        ..ReportQuery::default()
    };
    assert_eq!(period(&long, current, None).unwrap().0.label(), "2016-05");
    let zero = ReportQuery {
        months: Some(0),
        ..ReportQuery::default()
    };
    assert!(period(&zero, current, None).is_err());
}

#[test]
fn месяц_считается_по_местному_времени() {
    let at = utc(2026, 1, 31, 22, 30);
    assert_eq!(Month::of(at, msk()).label(), "2026-02");
    assert_eq!(
        Month::of(at, FixedOffset::east_opt(0).unwrap()).label(),
        "2026-01"
    );
    assert_eq!(
        Month::parse("2026-02").unwrap().start(msk()),
        utc(2026, 1, 31, 21, 0)
    );
    assert_eq!(Month::parse("2026-12").unwrap().shift(1).label(), "2027-01");
    assert_eq!(
        Month::parse("2026-01").unwrap().shift(-1).label(),
        "2025-12"
    );
    assert!(Month::parse("26-01").is_none());
}

#[test]
fn названия_платёжек_и_способов() {
    let t = titles();
    assert_eq!(report::provider_title(&t, "yookassa"), "ЮKassa");
    assert_eq!(report::provider_title(&t, "manual"), "Вручную");
    assert_eq!(report::provider_title(&t, "old_bot_pay"), "old_bot_pay");
}

#[tokio::test]
async fn суточный_пересчёт_подбирает_пропущенный_заказ() {
    let Some(db) = test_store().await else {
        return;
    };
    scene(&db.store).await;
    spans::refresh(&db.store, Currency::Rub, None)
        .await
        .unwrap();
    let seed = Seed {
        store: db.store.clone(),
    };
    seed.exec("UPDATE analytics_state SET watermark = now() + interval '1 day'")
        .await;
    let late = seed.client(utc(2026, 3, 3, 9, 0), None).await;
    let sub = seed.subscription(late).await;
    seed.buy(
        late,
        sub,
        "new",
        utc(2026, 3, 3, 10, 0),
        30,
        30_000,
        "yookassa",
        Some("sbp"),
    )
    .await;
    let spans_of = |client: i64| {
        let store = db.store.clone();
        async move {
            let mut conn = store.conn().await.unwrap();
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM analytics_spans WHERE client_id = $1",
            )
            .bind(client)
            .fetch_one(conn.conn())
            .await
            .unwrap()
        }
    };
    let missed = spans::refresh(&db.store, Currency::Rub, None)
        .await
        .unwrap();
    assert!(!missed.full);
    assert_eq!(spans_of(late).await, 0);
    let fresh = spans::refresh(&db.store, Currency::Rub, Some(chrono::Duration::days(1)))
        .await
        .unwrap();
    assert!(!fresh.full);
    seed.exec("UPDATE analytics_state SET rebuilt_at = now() - interval '2 days'")
        .await;
    let daily = spans::refresh(&db.store, Currency::Rub, Some(chrono::Duration::days(1)))
        .await
        .unwrap();
    assert!(daily.full);
    assert_eq!(spans_of(late).await, 1);
    let after = spans::refresh(&db.store, Currency::Rub, Some(chrono::Duration::days(1)))
        .await
        .unwrap();
    assert!(!after.full);
    db.cleanup().await;
}
