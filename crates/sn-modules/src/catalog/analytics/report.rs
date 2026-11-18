use super::spans::GRACE_DAYS;
use crate::context::PaymentTitle;
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Utc};
use serde::{Serialize, Serializer};
use sn_domain::money::Currency;
use sn_store::{Db, Store, StoreError, StoreResult};
use std::collections::HashMap;

pub const MAX_MONTHS: i32 = 120;
pub const COHORT_COLUMNS: usize = 12;
const PAID_LIKE: &str = "('paid', 'partially_refunded', 'refunded')";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Month {
    year: i32,
    month: u32,
}

impl Month {
    pub fn new(year: i32, month: u32) -> Option<Month> {
        ((1..=12).contains(&month) && (1970..=2200).contains(&year))
            .then_some(Month { year, month })
    }

    pub fn parse(text: &str) -> Option<Month> {
        let (y, m) = text.trim().split_once('-')?;
        if y.len() != 4 || m.len() != 2 {
            return None;
        }
        Month::new(y.parse().ok()?, m.parse().ok()?)
    }

    pub fn of(at: DateTime<Utc>, offset: FixedOffset) -> Month {
        let local = at.with_timezone(&offset);
        Month {
            year: local.year(),
            month: local.month(),
        }
    }

    fn index(self) -> i32 {
        self.year * 12 + i32::try_from(self.month).unwrap_or(1) - 1
    }

    fn from_index(index: i32) -> Month {
        Month {
            year: index.div_euclid(12),
            month: u32::try_from(index.rem_euclid(12) + 1).unwrap_or(1),
        }
    }

    pub fn shift(self, delta: i32) -> Month {
        Month::from_index(self.index() + delta)
    }

    pub fn until(self, other: Month) -> i32 {
        other.index() - self.index()
    }

    pub fn start(self, offset: FixedOffset) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(self.year, self.month, 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .and_then(|local| offset.from_local_datetime(&local).single())
            .map(|at| at.with_timezone(&Utc))
            .unwrap_or(DateTime::<Utc>::MIN_UTC)
    }

    pub fn label(self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }
}

impl Serialize for Month {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    All,
    Unlabeled,
    Named(String),
}

impl Source {
    pub fn from_query(value: Option<&str>) -> Source {
        match value.map(str::trim) {
            None => Source::All,
            Some("") => Source::Unlabeled,
            Some(name) => Source::Named(name.chars().take(64).collect()),
        }
    }

    fn clause(&self, column: &str, param: usize) -> String {
        match self {
            Source::All => "TRUE".into(),
            Source::Unlabeled => {
                format!("NOT EXISTS (SELECT 1 FROM client_utm u WHERE u.client_id = {column})")
            }
            Source::Named(_) => format!(
                "EXISTS (SELECT 1 FROM client_utm u WHERE u.client_id = {column} AND u.source = ${param})"
            ),
        }
    }
}

type Q<'q, O> = sqlx::query::QueryAs<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments>;

fn with_source<'q, O>(query: Q<'q, O>, source: &'q Source) -> Q<'q, O> {
    match source {
        Source::Named(name) => query.bind(name.as_str()),
        _ => query,
    }
}

fn month_of(column: &str, param: usize) -> String {
    format!("to_char(({column} AT TIME ZONE 'UTC') + make_interval(mins => ${param}), 'YYYY-MM')")
}

fn count(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MonthRow {
    pub month: String,
    pub revenue: i64,
    pub stars: i64,
    pub refunds: i64,
    pub payments: u32,
    pub mrr: i64,
    pub new: u32,
    pub renewed: u32,
    pub returned: u32,
    pub churned: u32,
    pub paying: u32,
    pub trials: u32,
    pub trials_bought: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub revenue: i64,
    pub stars: i64,
    pub refunds: i64,
    pub payments: u32,
    pub new: u32,
    pub renewed: u32,
    pub returned: u32,
    pub churned: u32,
    pub trials: u32,
    pub trials_bought: u32,
    pub mrr: i64,
    pub paying: u32,
    pub ltv: i64,
    pub ltv_clients: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CohortRow {
    pub month: String,
    pub size: u32,
    pub revenue: i64,
    pub retained: Vec<Option<u32>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MethodRow {
    pub code: Option<String>,
    pub title: String,
    pub revenue: i64,
    pub stars: i64,
    pub payments: u32,
    pub invoices: u32,
    pub failed: u32,
    pub refunds: u32,
    pub refunded: i64,
}

impl MethodRow {
    fn add(&mut self, other: &MethodRow) {
        self.revenue += other.revenue;
        self.stars += other.stars;
        self.payments += other.payments;
        self.invoices += other.invoices;
        self.failed += other.failed;
        self.refunds += other.refunds;
        self.refunded += other.refunded;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderRow {
    pub provider: String,
    #[serde(flatten)]
    pub total: MethodRow,
    pub methods: Vec<MethodRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRow {
    pub source: Option<String>,
    pub clients: u32,
    pub paying: u32,
    pub revenue: i64,
    pub stars: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub currency: Currency,
    pub from: Month,
    pub to: Month,
    pub current: Month,
    pub months: Vec<MonthRow>,
    pub totals: Totals,
    pub cohorts: Vec<CohortRow>,
    pub providers: Vec<ProviderRow>,
    pub sources: Vec<SourceRow>,
    pub source_options: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Params {
    pub currency: Currency,
    pub offset: FixedOffset,
    pub from: Month,
    pub to: Month,
    pub now: DateTime<Utc>,
    pub source: Source,
}

pub async fn first_month(db: &mut impl Db, offset: FixedOffset) -> StoreResult<Option<Month>> {
    let first: Option<DateTime<Utc>> = sqlx::query_scalar(&format!(
        "SELECT LEAST( \
           (SELECT min(paid_at) FROM payments WHERE paid_at IS NOT NULL AND status IN {PAID_LIKE} AND provider <> 'balance'), \
           (SELECT min(starts_at) FROM analytics_spans))"
    ))
    .fetch_one(db.conn())
    .await?;
    Ok(first.map(|at| Month::of(at, offset)))
}

fn method_title(titles: &[PaymentTitle], provider: &str, code: &str) -> String {
    if code.is_empty() {
        return titles
            .iter()
            .find(|t| t.provider == provider)
            .map(|t| t.button.trim())
            .filter(|button| !button.is_empty())
            .map_or_else(
                || "Без выбора способа".into(),
                |button| format!("Кнопка «{button}»"),
            );
    }
    titles
        .iter()
        .find(|t| t.provider == provider)
        .and_then(|t| t.methods.iter().find(|m| m.code == code))
        .map_or_else(|| code.to_string(), |m| m.title.clone())
}

pub fn provider_title(titles: &[PaymentTitle], provider: &str) -> String {
    if let Some(t) = titles.iter().find(|t| t.provider == provider) {
        return t.title.clone();
    }
    match provider {
        "manual" => "Вручную".into(),
        "stars" => "Звёзды Telegram".into(),
        other => other.to_string(),
    }
}

struct Window {
    offset_minutes: i32,
    labels: Vec<String>,
    slot: HashMap<String, usize>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    points: Vec<DateTime<Utc>>,
    ends: Vec<DateTime<Utc>>,
}

impl Window {
    fn new(p: &Params) -> Window {
        let span = p.from.until(p.to).max(0) + 1;
        let months: Vec<Month> = (0..span).map(|i| p.from.shift(i)).collect();
        let labels: Vec<String> = months.iter().map(|m| m.label()).collect();
        let slot = labels
            .iter()
            .enumerate()
            .map(|(i, l)| (l.clone(), i))
            .collect();
        let ends: Vec<DateTime<Utc>> = months.iter().map(|m| m.shift(1).start(p.offset)).collect();
        Window {
            offset_minutes: p.offset.local_minus_utc() / 60,
            labels,
            slot,
            start: p.from.start(p.offset),
            end: p.to.shift(1).start(p.offset),
            points: ends.iter().map(|e| (*e).min(p.now)).collect(),
            ends,
        }
    }

    fn index(&self, month: &str) -> Option<usize> {
        self.slot.get(month).copied()
    }

    fn last_point(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        self.points.last().copied().unwrap_or(now)
    }
}

type ByMonth = Vec<(String, i64)>;
type CohortIsland = (String, DateTime<Utc>, i32, DateTime<Utc>, DateTime<Utc>);
type ByMonthPair = Vec<(String, i64, i64)>;

struct Flow {
    money: Vec<(String, i64, i64, i64, i64)>,
    starts: ByMonthPair,
    gone: ByMonth,
    renewed: ByMonth,
}

async fn flow(store: &Store, p: &Params, w: &Window) -> StoreResult<Flow> {
    let mut conn = store.conn().await?;
    let src = &p.source;
    let money = with_source(
        sqlx::query_as(&format!(
            "SELECT {m}, \
               COALESCE(sum(p.amount_minor - p.refunded_minor) FILTER (WHERE p.currency = $4), 0)::bigint, \
               COALESCE(sum(p.amount_minor - p.refunded_minor) FILTER (WHERE p.currency = 'XTR'), 0)::bigint, \
               COALESCE(sum(p.refunded_minor) FILTER (WHERE p.currency = $4), 0)::bigint, \
               count(*) FILTER (WHERE p.currency = $4 AND p.amount_minor > p.refunded_minor) \
             FROM payments p \
             WHERE p.paid_at >= $1 AND p.paid_at < $2 AND p.status IN {PAID_LIKE} \
               AND p.provider <> 'balance' AND {f} \
             GROUP BY 1",
            m = month_of("p.paid_at", 3),
            f = src.clause("p.client_id", 5)
        ))
        .bind(w.start)
        .bind(w.end)
        .bind(w.offset_minutes)
        .bind(p.currency.code()),
        src,
    )
    .fetch_all(conn.conn())
    .await?;
    let starts = with_source(
        sqlx::query_as(&format!(
            "SELECT {m}, count(*) FILTER (WHERE i.seq = 1), count(*) FILTER (WHERE i.seq > 1) \
             FROM analytics_islands i WHERE i.starts_at >= $1 AND i.starts_at < $2 AND {f} GROUP BY 1",
            m = month_of("i.starts_at", 3),
            f = src.clause("i.client_id", 4)
        ))
        .bind(w.start)
        .bind(w.end)
        .bind(w.offset_minutes),
        src,
    )
    .fetch_all(conn.conn())
    .await?;
    let gone = with_source(
        sqlx::query_as(&format!(
            "SELECT {m}, count(*) FROM analytics_islands i \
             WHERE i.ends_at >= $1 AND i.ends_at < $2 AND i.ends_at <= $4 AND {f} GROUP BY 1",
            m = month_of("i.ends_at", 3),
            f = src.clause("i.client_id", 5)
        ))
        .bind(w.start)
        .bind(w.end)
        .bind(w.offset_minutes)
        .bind(p.now - Duration::days(GRACE_DAYS)),
        src,
    )
    .fetch_all(conn.conn())
    .await?;
    let renewed = with_source(
        sqlx::query_as(&format!(
            "SELECT {m}, count(DISTINCT s.client_id) FROM analytics_spans s \
             WHERE s.continues AND s.paid_at >= $1 AND s.paid_at < $2 AND {f} GROUP BY 1",
            m = month_of("s.paid_at", 3),
            f = src.clause("s.client_id", 4)
        ))
        .bind(w.start)
        .bind(w.end)
        .bind(w.offset_minutes),
        src,
    )
    .fetch_all(conn.conn())
    .await?;
    Ok(Flow {
        money,
        starts,
        gone,
        renewed,
    })
}

async fn curves(store: &Store, p: &Params, w: &Window) -> StoreResult<(Vec<i64>, Vec<i64>)> {
    let mut conn = store.conn().await?;
    let last = w.last_point(p.now);
    let mrr = deltas(
        &mut conn,
        "SELECT s.starts_at AS at, s.monthly_minor AS d FROM analytics_spans s WHERE s.starts_at < $2 AND {f} \
         UNION ALL SELECT s.ends_at, -s.monthly_minor FROM analytics_spans s WHERE s.ends_at < $2 AND {f}",
        "s.client_id",
        &w.points,
        last,
        &p.source,
    )
    .await?;
    let paying = deltas(
        &mut conn,
        "SELECT i.starts_at AS at, 1::bigint AS d FROM analytics_islands i WHERE i.starts_at < $2 AND {f} \
         UNION ALL SELECT i.ends_at, -1::bigint FROM analytics_islands i WHERE i.ends_at < $2 AND {f}",
        "i.client_id",
        &w.points,
        last,
        &p.source,
    )
    .await?;
    Ok((mrr, paying))
}

async fn trials(store: &Store, p: &Params, w: &Window) -> StoreResult<ByMonthPair> {
    let mut conn = store.conn().await?;
    let src = &p.source;
    with_source(
        sqlx::query_as(&format!(
            "SELECT {m}, count(*), count(*) FILTER (WHERE l.last_paid >= o.paid_at) \
             FROM orders o \
             LEFT JOIN (SELECT client_id, max(paid_at) AS last_paid FROM analytics_spans GROUP BY client_id) l \
               ON l.client_id = o.client_id \
             WHERE o.kind = 'trial' AND o.paid_at >= $1 AND o.paid_at < $2 \
               AND o.status IN ('paid', 'fulfilled', 'review') AND {f} GROUP BY 1",
            m = month_of("o.paid_at", 3),
            f = src.clause("o.client_id", 4)
        ))
        .bind(w.start)
        .bind(w.end)
        .bind(w.offset_minutes),
        src,
    )
    .fetch_all(conn.conn())
    .await
    .map_err(Into::into)
}

async fn lifetime(store: &Store, p: &Params) -> StoreResult<(i64, i64)> {
    let mut conn = store.conn().await?;
    let src = &p.source;
    with_source(
        sqlx::query_as(&format!(
            "SELECT count(*), COALESCE(sum(x.net), 0)::bigint FROM ( \
               SELECT p.client_id, sum(p.amount_minor - p.refunded_minor) AS net FROM payments p \
               WHERE p.status IN {PAID_LIKE} AND p.provider <> 'balance' AND p.currency = $1 AND {f} \
               GROUP BY p.client_id HAVING sum(p.amount_minor - p.refunded_minor) > 0) x",
            f = src.clause("p.client_id", 2)
        ))
        .bind(p.currency.code()),
        src,
    )
    .fetch_one(conn.conn())
    .await
    .map_err(Into::into)
}

async fn source_options(store: &Store) -> StoreResult<Vec<String>> {
    let mut conn = store.conn().await?;
    sqlx::query_scalar(
        "SELECT source FROM client_utm WHERE source IS NOT NULL \
         GROUP BY source ORDER BY count(*) DESC, source LIMIT 200",
    )
    .fetch_all(conn.conn())
    .await
    .map_err(Into::into)
}

pub async fn build(store: &Store, p: &Params, titles: &[PaymentTitle]) -> StoreResult<Report> {
    let w = Window::new(p);
    let (
        flow,
        (mrr, paying),
        trials,
        (ltv_clients, ltv_total),
        (cohorts, providers),
        (sources, source_options),
    ) = tokio::try_join!(
        flow(store, p, &w),
        curves(store, p, &w),
        trials(store, p, &w),
        lifetime(store, p),
        async {
            Ok::<_, StoreError>((
                cohorts(store, p, &w).await?,
                providers(store, p, &w, titles).await?,
            ))
        },
        async { Ok::<_, StoreError>((sources(store, p, &w).await?, source_options(store).await?)) },
    )?;
    let mut rows: Vec<MonthRow> = w
        .labels
        .iter()
        .map(|l| MonthRow {
            month: l.clone(),
            ..MonthRow::default()
        })
        .collect();
    for (month, revenue, stars, refunds, payments) in flow.money {
        if let Some(i) = w.index(&month) {
            rows[i].revenue = revenue;
            rows[i].stars = stars;
            rows[i].refunds = refunds;
            rows[i].payments = count(payments);
        }
    }
    for (month, fresh, back) in flow.starts {
        if let Some(i) = w.index(&month) {
            rows[i].new = count(fresh);
            rows[i].returned = count(back);
        }
    }
    for (month, n) in flow.gone {
        if let Some(i) = w.index(&month) {
            rows[i].churned = count(n);
        }
    }
    for (month, n) in flow.renewed {
        if let Some(i) = w.index(&month) {
            rows[i].renewed = count(n);
        }
    }
    for (month, taken, bought) in trials {
        if let Some(i) = w.index(&month) {
            rows[i].trials = count(taken);
            rows[i].trials_bought = count(bought);
        }
    }
    for (i, row) in rows.iter_mut().enumerate() {
        row.mrr = mrr.get(i).copied().unwrap_or(0);
        row.paying = count(paying.get(i).copied().unwrap_or(0));
    }
    let mut totals = Totals::default();
    for row in &rows {
        totals.revenue += row.revenue;
        totals.stars += row.stars;
        totals.refunds += row.refunds;
        totals.payments += row.payments;
        totals.new += row.new;
        totals.renewed += row.renewed;
        totals.returned += row.returned;
        totals.churned += row.churned;
        totals.trials += row.trials;
        totals.trials_bought += row.trials_bought;
    }
    if let Some(last) = rows.last() {
        totals.mrr = last.mrr;
        totals.paying = last.paying;
    }
    totals.ltv_clients = count(ltv_clients);
    totals.ltv = if ltv_clients > 0 {
        (ltv_total + ltv_clients / 2) / ltv_clients
    } else {
        0
    };
    Ok(Report {
        currency: p.currency,
        from: p.from,
        to: p.to,
        current: Month::of(p.now, p.offset),
        months: rows,
        totals,
        cohorts,
        providers,
        sources,
        source_options,
    })
}

async fn deltas(
    db: &mut impl Db,
    events: &str,
    column: &str,
    points: &[DateTime<Utc>],
    last: DateTime<Utc>,
    src: &Source,
) -> StoreResult<Vec<i64>> {
    let sql = format!(
        "SELECT width_bucket(e.at, $1::timestamptz[]), sum(e.d)::bigint FROM ({}) e GROUP BY 1",
        events.replace("{f}", &src.clause(column, 3))
    );
    let buckets: Vec<(i32, i64)> = with_source(sqlx::query_as(&sql).bind(points).bind(last), src)
        .fetch_all(db.conn())
        .await?;
    let mut by_bucket = vec![0i64; points.len() + 1];
    for (bucket, sum) in buckets {
        if let Some(slot) = usize::try_from(bucket)
            .ok()
            .and_then(|b| by_bucket.get_mut(b))
        {
            *slot += sum;
        }
    }
    let mut running = 0i64;
    Ok((0..points.len())
        .map(|k| {
            running += by_bucket[k];
            running
        })
        .collect())
}

async fn cohorts(store: &Store, p: &Params, w: &Window) -> StoreResult<Vec<CohortRow>> {
    let mut conn = store.conn().await?;
    let db = &mut conn;
    let src = &p.source;
    let (start, end, offset_minutes) = (w.start, w.end, w.offset_minutes);
    let islands: Vec<CohortIsland> = with_source(
        sqlx::query_as(&format!(
            "SELECT {m}, f.starts_at, i.seq, i.starts_at, i.ends_at \
             FROM analytics_islands f JOIN analytics_islands i ON i.client_id = f.client_id \
             WHERE f.seq = 1 AND f.starts_at >= $1 AND f.starts_at < $2 AND {f}",
            m = month_of("f.starts_at", 3),
            f = src.clause("f.client_id", 4)
        ))
        .bind(start)
        .bind(end)
        .bind(offset_minutes),
        src,
    )
    .fetch_all(db.conn())
    .await?;
    let revenue: Vec<(String, i64)> = with_source(
        sqlx::query_as(&format!(
            "SELECT {m}, COALESCE(sum(p.amount_minor - p.refunded_minor), 0)::bigint \
             FROM analytics_islands f JOIN payments p ON p.client_id = f.client_id \
             WHERE f.seq = 1 AND f.starts_at >= $1 AND f.starts_at < $2 \
               AND p.status IN {PAID_LIKE} AND p.provider <> 'balance' AND p.currency = $4 AND {f} \
             GROUP BY 1",
            m = month_of("f.starts_at", 3),
            f = src.clause("f.client_id", 5)
        ))
        .bind(start)
        .bind(end)
        .bind(offset_minutes)
        .bind(p.currency.code()),
        src,
    )
    .fetch_all(db.conn())
    .await?;
    let month_len = Duration::days(super::spans::MONTH_DAYS);
    let mut rows: Vec<CohortRow> = w
        .labels
        .iter()
        .map(|l| CohortRow {
            month: l.clone(),
            size: 0,
            revenue: 0,
            retained: vec![None; COHORT_COLUMNS],
        })
        .collect();
    for (month, first, seq, starts_at, ends_at) in islands {
        let Some(i) = w.index(&month) else {
            continue;
        };
        let cutoff = w.ends.get(i).copied().unwrap_or(p.now);
        let row = &mut rows[i];
        if seq == 1 {
            row.size += 1;
        }
        for k in 1..=COHORT_COLUMNS {
            let Ok(step) = i32::try_from(k) else {
                continue;
            };
            if cutoff + month_len * step > p.now {
                break;
            }
            let at = first + month_len * step;
            let cell = &mut row.retained[k - 1];
            let hit = starts_at <= at && at < ends_at;
            *cell = Some(cell.unwrap_or(0) + u32::from(hit));
        }
    }
    for (month, sum) in revenue {
        if let Some(i) = w.index(&month) {
            rows[i].revenue = sum;
        }
    }
    for row in &mut rows {
        if row.size == 0 {
            row.retained = vec![None; COHORT_COLUMNS];
        }
    }
    Ok(rows)
}

async fn providers(
    store: &Store,
    p: &Params,
    w: &Window,
    titles: &[PaymentTitle],
) -> StoreResult<Vec<ProviderRow>> {
    let mut conn = store.conn().await?;
    let db = &mut conn;
    let src = &p.source;
    let (start, end) = (w.start, w.end);
    type Row = (String, String, i64, i64, i64, i64, i64, i64, i64);
    let rows: Vec<Row> = with_source(
        sqlx::query_as(&format!(
            "WITH paid AS ( \
               SELECT p.provider, COALESCE(p.pay_method, '') AS method, \
                 COALESCE(sum(p.amount_minor - p.refunded_minor) FILTER (WHERE p.currency = $3), 0)::bigint AS revenue, \
                 COALESCE(sum(p.amount_minor - p.refunded_minor) FILTER (WHERE p.currency = 'XTR'), 0)::bigint AS stars, \
                 count(*) FILTER (WHERE p.amount_minor > p.refunded_minor) AS payments, \
                 count(*) FILTER (WHERE p.refunded_minor > 0) AS refunds, \
                 COALESCE(sum(p.refunded_minor) FILTER (WHERE p.currency = $3), 0)::bigint AS refunded \
               FROM payments p \
               WHERE p.paid_at >= $1 AND p.paid_at < $2 AND p.status IN {PAID_LIKE} \
                 AND p.provider <> 'balance' AND p.amount_minor > 0 AND {f} \
               GROUP BY 1, 2 \
             ), asked AS ( \
               SELECT p.provider, COALESCE(p.pay_method, '') AS method, count(*) AS invoices, \
                 count(*) FILTER (WHERE p.status IN ('failed', 'canceled') \
                   OR (p.status = 'pending' AND p.expires_at <= $4)) AS failed \
               FROM payments p \
               WHERE p.created_at >= $1 AND p.created_at < $2 AND p.provider <> 'balance' \
                 AND p.amount_minor > 0 AND {f} \
               GROUP BY 1, 2 \
             ) \
             SELECT COALESCE(a.provider, b.provider), COALESCE(a.method, b.method), \
               COALESCE(a.revenue, 0), COALESCE(a.stars, 0), COALESCE(a.payments, 0), \
               COALESCE(b.invoices, 0), COALESCE(b.failed, 0), COALESCE(a.refunds, 0), COALESCE(a.refunded, 0) \
             FROM paid a FULL JOIN asked b ON a.provider = b.provider AND a.method = b.method",
            f = src.clause("p.client_id", 5)
        ))
        .bind(start)
        .bind(end)
        .bind(p.currency.code())
        .bind(p.now),
        src,
    )
    .fetch_all(db.conn())
    .await?;
    let mut out: Vec<ProviderRow> = Vec::new();
    for (provider, method, revenue, stars, payments, invoices, failed, refunds, refunded) in rows {
        let line = MethodRow {
            code: (!method.is_empty()).then(|| method.clone()),
            title: method_title(titles, &provider, &method),
            revenue,
            stars,
            payments: count(payments),
            invoices: count(invoices),
            failed: count(failed),
            refunds: count(refunds),
            refunded,
        };
        match out.iter_mut().find(|r| r.provider == provider) {
            Some(row) => {
                row.total.add(&line);
                row.methods.push(line);
            }
            None => {
                let mut total = MethodRow {
                    title: provider_title(titles, &provider),
                    ..MethodRow::default()
                };
                total.add(&line);
                out.push(ProviderRow {
                    provider,
                    total,
                    methods: vec![line],
                });
            }
        }
    }
    for row in &mut out {
        row.methods.sort_by(|a, b| {
            (b.revenue, b.stars, b.invoices).cmp(&(a.revenue, a.stars, a.invoices))
        });
    }
    out.sort_by(|a, b| {
        (
            b.total.revenue,
            b.total.stars,
            b.total.invoices,
            &a.provider,
        )
            .cmp(&(
                a.total.revenue,
                a.total.stars,
                a.total.invoices,
                &b.provider,
            ))
    });
    Ok(out)
}

async fn sources(store: &Store, p: &Params, w: &Window) -> StoreResult<Vec<SourceRow>> {
    let mut conn = store.conn().await?;
    let db = &mut conn;
    let src = &p.source;
    let (start, end) = (w.start, w.end);
    let rows: Vec<(Option<String>, i64, i64, i64, i64)> = with_source(
        sqlx::query_as(&format!(
            "WITH c AS ( \
               SELECT cl.id AS client_id, u.source FROM clients cl \
               LEFT JOIN client_utm u ON u.client_id = cl.id \
               WHERE cl.created_at >= $1 AND cl.created_at < $2 AND cl.deleted_at IS NULL AND {f} \
             ), m AS ( \
               SELECT p.client_id, \
                 COALESCE(sum(p.amount_minor - p.refunded_minor) FILTER (WHERE p.currency = $3), 0) AS revenue, \
                 COALESCE(sum(p.amount_minor - p.refunded_minor) FILTER (WHERE p.currency = 'XTR'), 0) AS stars \
               FROM payments p JOIN c ON c.client_id = p.client_id \
               WHERE p.status IN {PAID_LIKE} AND p.provider <> 'balance' \
               GROUP BY p.client_id \
             ) \
             SELECT c.source, count(*), \
               count(*) FILTER (WHERE COALESCE(m.revenue, 0) > 0 OR COALESCE(m.stars, 0) > 0), \
               COALESCE(sum(m.revenue), 0)::bigint, COALESCE(sum(m.stars), 0)::bigint \
             FROM c LEFT JOIN m ON m.client_id = c.client_id \
             GROUP BY c.source ORDER BY count(*) DESC, c.source NULLS FIRST LIMIT 100",
            f = src.clause("cl.id", 4)
        ))
        .bind(start)
        .bind(end)
        .bind(p.currency.code()),
        src,
    )
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(source, clients, paying, revenue, stars)| SourceRow {
            source,
            clients: count(clients),
            paying: count(paying),
            revenue,
            stars,
        })
        .collect())
}
