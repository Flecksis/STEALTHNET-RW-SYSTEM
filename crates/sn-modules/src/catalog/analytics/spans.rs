use chrono::{DateTime, Duration, Utc};
use sn_domain::money::Currency;
use sn_store::{Db, Store, StoreResult};

pub const GRACE_DAYS: i64 = 3;
pub const MONTH_DAYS: i64 = 30;
pub const VERSION: i32 = 1;
const LOCK: i64 = 7_240_001;
const OVERLAP_MINUTES: i64 = 10;
const CHUNK: usize = 2_000;
const ANALYZE_AFTER: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    New,
    Renewal,
    Change,
}

impl Kind {
    fn parse(value: &str) -> Option<Kind> {
        match value {
            "new" => Some(Kind::New),
            "renewal" => Some(Kind::Renewal),
            "change" => Some(Kind::Change),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Purchase {
    pub order_id: i64,
    pub subscription_id: Option<i64>,
    pub kind: Kind,
    pub paid_at: DateTime<Utc>,
    pub days: i64,
    pub value_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub order_id: i64,
    pub paid_at: DateTime<Utc>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub monthly_minor: i64,
    pub continues: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Island {
    pub seq: i32,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

pub fn monthly(value_minor: i64, days: i64) -> i64 {
    if days <= 0 || value_minor <= 0 {
        return 0;
    }
    let scaled = i128::from(value_minor) * i128::from(MONTH_DAYS);
    let days = i128::from(days);
    i64::try_from((scaled * 2 + days) / (days * 2)).unwrap_or(i64::MAX)
}

pub fn build(mut purchases: Vec<Purchase>) -> (Vec<Span>, Vec<Island>) {
    purchases.retain(|p| p.days > 0);
    purchases.sort_by_key(|p| (p.subscription_id.unwrap_or(0), p.paid_at, p.order_id));
    let mut spans: Vec<Span> = Vec::with_capacity(purchases.len());
    let mut chain: Option<i64> = None;
    let mut last: Option<usize> = None;
    for p in purchases {
        let key = p.subscription_id.unwrap_or(0);
        if chain != Some(key) {
            chain = Some(key);
            last = None;
        }
        let starts_at = match (p.kind, last) {
            (Kind::Renewal, Some(i)) => spans[i].ends_at.max(p.paid_at),
            _ => p.paid_at,
        };
        if p.kind != Kind::Renewal
            && let Some(i) = last
            && spans[i].ends_at > p.paid_at
        {
            spans[i].ends_at = spans[i].starts_at.max(p.paid_at);
        }
        spans.push(Span {
            order_id: p.order_id,
            paid_at: p.paid_at,
            starts_at,
            ends_at: starts_at + Duration::days(p.days),
            monthly_minor: monthly(p.value_minor, p.days),
            continues: false,
        });
        last = Some(spans.len() - 1);
    }
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| (spans[i].starts_at, spans[i].order_id));
    let grace = Duration::days(GRACE_DAYS);
    let mut islands: Vec<Island> = Vec::new();
    for i in order {
        let (starts_at, ends_at) = (spans[i].starts_at, spans[i].ends_at);
        if ends_at <= starts_at {
            continue;
        }
        match islands.last_mut() {
            Some(island) if starts_at <= island.ends_at + grace => {
                island.ends_at = island.ends_at.max(ends_at);
                spans[i].continues = true;
            }
            _ => {
                let seq = i32::try_from(islands.len() + 1).unwrap_or(i32::MAX);
                islands.push(Island {
                    seq,
                    starts_at,
                    ends_at,
                });
            }
        }
    }
    (spans, islands)
}

type OrderRow = (i64, i64, Option<i64>, String, DateTime<Utc>, i64, i64);

async fn rebuild(db: &mut impl Db, clients: &[i64], currency: Currency) -> StoreResult<()> {
    let rows: Vec<OrderRow> = sqlx::query_as(
        "SELECT o.id, o.client_id, o.subscription_id, o.kind, o.paid_at, \
           CASE WHEN jsonb_typeof(o.quote -> 'days') = 'number' \
             THEN LEAST(GREATEST((o.quote ->> 'days')::numeric, 0), 36600)::bigint ELSE 0 END, \
           o.total_minor \
         FROM orders o \
         WHERE o.client_id = ANY($1) AND o.kind IN ('new', 'renewal', 'change') \
           AND o.status IN ('paid', 'fulfilled', 'review') AND o.paid_at IS NOT NULL \
           AND o.currency = $2 AND (o.total_minor > 0 OR COALESCE(o.stars_total, 0) > 0) \
         ORDER BY o.client_id",
    )
    .bind(clients)
    .bind(currency.code())
    .fetch_all(db.conn())
    .await?;
    sqlx::query("DELETE FROM analytics_spans WHERE client_id = ANY($1)")
        .bind(clients)
        .execute(db.conn())
        .await?;
    sqlx::query("DELETE FROM analytics_islands WHERE client_id = ANY($1)")
        .bind(clients)
        .execute(db.conn())
        .await?;
    let mut span_cols = SpanColumns::default();
    let mut island_cols = IslandColumns::default();
    let mut start = 0;
    while start < rows.len() {
        let client_id = rows[start].1;
        let mut end = start;
        while end < rows.len() && rows[end].1 == client_id {
            end += 1;
        }
        let purchases = rows[start..end]
            .iter()
            .filter_map(|(id, _, sub, kind, paid_at, days, total)| {
                Kind::parse(kind).map(|kind| Purchase {
                    order_id: *id,
                    subscription_id: *sub,
                    kind,
                    paid_at: *paid_at,
                    days: *days,
                    value_minor: *total,
                })
            })
            .collect();
        let (spans, islands) = build(purchases);
        span_cols.push(client_id, &spans);
        island_cols.push(client_id, &islands);
        start = end;
    }
    if !span_cols.order.is_empty() {
        sqlx::query(
            "INSERT INTO analytics_spans (order_id, client_id, paid_at, starts_at, ends_at, monthly_minor, continues) \
             SELECT * FROM unnest($1::bigint[], $2::bigint[], $3::timestamptz[], $4::timestamptz[], $5::timestamptz[], $6::bigint[], $7::bool[])",
        )
        .bind(&span_cols.order)
        .bind(&span_cols.client)
        .bind(&span_cols.paid)
        .bind(&span_cols.starts)
        .bind(&span_cols.ends)
        .bind(&span_cols.monthly)
        .bind(&span_cols.continues)
        .execute(db.conn())
        .await?;
    }
    if !island_cols.client.is_empty() {
        sqlx::query(
            "INSERT INTO analytics_islands (client_id, seq, starts_at, ends_at) \
             SELECT * FROM unnest($1::bigint[], $2::int[], $3::timestamptz[], $4::timestamptz[])",
        )
        .bind(&island_cols.client)
        .bind(&island_cols.seq)
        .bind(&island_cols.starts)
        .bind(&island_cols.ends)
        .execute(db.conn())
        .await?;
    }
    Ok(())
}

#[derive(Default)]
struct SpanColumns {
    order: Vec<i64>,
    client: Vec<i64>,
    paid: Vec<DateTime<Utc>>,
    starts: Vec<DateTime<Utc>>,
    ends: Vec<DateTime<Utc>>,
    monthly: Vec<i64>,
    continues: Vec<bool>,
}

impl SpanColumns {
    fn push(&mut self, client_id: i64, spans: &[Span]) {
        for s in spans {
            self.order.push(s.order_id);
            self.client.push(client_id);
            self.paid.push(s.paid_at);
            self.starts.push(s.starts_at);
            self.ends.push(s.ends_at);
            self.monthly.push(s.monthly_minor);
            self.continues.push(s.continues);
        }
    }
}

#[derive(Default)]
struct IslandColumns {
    client: Vec<i64>,
    seq: Vec<i32>,
    starts: Vec<DateTime<Utc>>,
    ends: Vec<DateTime<Utc>>,
}

impl IslandColumns {
    fn push(&mut self, client_id: i64, islands: &[Island]) {
        for i in islands {
            self.client.push(client_id);
            self.seq.push(i.seq);
            self.starts.push(i.starts_at);
            self.ends.push(i.ends_at);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refreshed {
    pub full: bool,
    pub skipped: bool,
    pub clients: usize,
}

type State = (String, i32, DateTime<Utc>, DateTime<Utc>);

async fn state(db: &mut impl Db) -> StoreResult<Option<State>> {
    Ok(sqlx::query_as(
        "SELECT currency, version, watermark, rebuilt_at FROM analytics_state WHERE id",
    )
    .fetch_optional(db.conn())
    .await?)
}

pub async fn refresh(
    store: &Store,
    currency: Currency,
    rebuild_after: Option<Duration>,
) -> StoreResult<Refreshed> {
    let mut tx = store.begin().await?;
    let free: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(LOCK)
        .fetch_one(tx.conn())
        .await?;
    if !free {
        if rebuild_after.is_none() && state(&mut tx).await?.is_some() {
            tx.rollback().await?;
            return Ok(Refreshed {
                full: false,
                skipped: true,
                clients: 0,
            });
        }
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(LOCK)
            .execute(tx.conn())
            .await?;
    }
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(tx.conn())
        .await?;
    let since = match state(&mut tx).await? {
        Some((code, version, watermark, rebuilt_at))
            if code == currency.code()
                && version == VERSION
                && rebuild_after.is_none_or(|after| now - rebuilt_at < after) =>
        {
            Some((watermark - Duration::minutes(OVERLAP_MINUTES), rebuilt_at))
        }
        _ => None,
    };
    let clients: Vec<i64> = match since {
        Some((since, _)) => {
            sqlx::query_scalar(
                "SELECT DISTINCT client_id FROM orders \
                 WHERE updated_at > $1 AND kind IN ('new', 'renewal', 'change')",
            )
            .bind(since)
            .fetch_all(tx.conn())
            .await?
        }
        None => {
            sqlx::query("DELETE FROM analytics_spans")
                .execute(tx.conn())
                .await?;
            sqlx::query("DELETE FROM analytics_islands")
                .execute(tx.conn())
                .await?;
            sqlx::query_scalar(
                "SELECT DISTINCT client_id FROM orders \
                 WHERE kind IN ('new', 'renewal', 'change') AND paid_at IS NOT NULL",
            )
            .fetch_all(tx.conn())
            .await?
        }
    };
    for chunk in clients.chunks(CHUNK) {
        rebuild(&mut tx, chunk, currency).await?;
    }
    if since.is_none() || clients.len() >= ANALYZE_AFTER {
        sqlx::query("ANALYZE analytics_spans, analytics_islands")
            .execute(tx.conn())
            .await?;
    }
    let rebuilt_at = since.map_or(now, |(_, at)| at);
    sqlx::query(
        "INSERT INTO analytics_state (id, currency, version, watermark, refreshed_at, rebuilt_at) \
         VALUES (true, $1, $2, $3, $3, $4) \
         ON CONFLICT (id) DO UPDATE SET currency = EXCLUDED.currency, version = EXCLUDED.version, \
           watermark = EXCLUDED.watermark, refreshed_at = EXCLUDED.refreshed_at, rebuilt_at = EXCLUDED.rebuilt_at",
    )
    .bind(currency.code())
    .bind(VERSION)
    .bind(now)
    .bind(rebuilt_at)
    .execute(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(Refreshed {
        full: since.is_none(),
        skipped: false,
        clients: clients.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap() + Duration::days(i64::from(day) - 1)
    }

    fn buy(order_id: i64, sub: i64, kind: Kind, day: u32, days: i64, value: i64) -> Purchase {
        Purchase {
            order_id,
            subscription_id: Some(sub),
            kind,
            paid_at: at(day),
            days,
            value_minor: value,
        }
    }

    #[test]
    fn месячная_стоимость_делит_оплату_на_месяцы() {
        assert_eq!(monthly(30_000, 30), 30_000);
        assert_eq!(monthly(80_000, 90), 26_667);
        assert_eq!(monthly(299_000, 365), 24_575);
        assert_eq!(monthly(0, 30), 0);
        assert_eq!(monthly(100, 0), 0);
    }

    #[test]
    fn раннее_продление_начинается_с_конца_прошлого_периода() {
        let (spans, islands) = build(vec![
            buy(1, 7, Kind::New, 1, 30, 30_000),
            buy(2, 7, Kind::Renewal, 25, 30, 30_000),
        ]);
        assert_eq!(spans[0].starts_at, at(1));
        assert_eq!(spans[0].ends_at, at(31));
        assert_eq!(spans[1].starts_at, at(31));
        assert_eq!(spans[1].ends_at, at(61));
        assert!(!spans[0].continues);
        assert!(spans[1].continues);
        assert_eq!(islands.len(), 1);
        assert_eq!((islands[0].starts_at, islands[0].ends_at), (at(1), at(61)));
    }

    #[test]
    fn смена_тарифа_обрезает_прошлый_период() {
        let (spans, islands) = build(vec![
            buy(1, 7, Kind::New, 1, 30, 30_000),
            buy(2, 7, Kind::Change, 11, 90, 80_000),
        ]);
        assert_eq!(spans[0].ends_at, at(11));
        assert_eq!(spans[1].starts_at, at(11));
        assert_eq!(spans[1].ends_at, at(101));
        assert_eq!(islands.len(), 1);
    }

    #[test]
    fn перерыв_дольше_трёх_дней_это_уход_и_возврат() {
        let (spans, islands) = build(vec![
            buy(1, 7, Kind::New, 1, 30, 30_000),
            buy(2, 7, Kind::Renewal, 33, 30, 30_000),
            buy(3, 7, Kind::Renewal, 70, 30, 30_000),
        ]);
        assert_eq!(spans[1].starts_at, at(33));
        assert!(spans[1].continues);
        assert_eq!(spans[2].starts_at, at(70));
        assert!(!spans[2].continues);
        assert_eq!(islands.len(), 2);
        assert_eq!(islands[0].ends_at, at(63));
        assert_eq!(islands[1].seq, 2);
        assert_eq!(islands[1].starts_at, at(70));
    }

    #[test]
    fn две_подписки_клиента_сливаются_в_один_отрезок() {
        let (spans, islands) = build(vec![
            buy(1, 7, Kind::New, 1, 30, 30_000),
            buy(2, 8, Kind::New, 10, 30, 20_000),
        ]);
        assert_eq!(spans.len(), 2);
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].ends_at, at(40));
        assert!(spans.iter().any(|s| s.order_id == 2 && s.continues));
    }
}
