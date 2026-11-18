use super::facts::crypto_cond;
use super::labels;
use super::metric::{
    BRow, Breakdown, Column, Detail, Group, ListRef, Metric, Point, Row, Unit, num, pct_text,
    ratio, share,
};
use super::query::{Arg, PAYMENT_DIMS, SUBSCRIPTION_DIMS, Sql, local_day, rows};
use super::tab::{Ctx, Series, Window, both, prev_points, prev_total};
use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, Utc};
use sn_store::{Db, StoreResult};

const MONEY: &str = "NOT f.stars AND f.provider <> 'balance'";

fn facts_values() -> [String; 9] {
    let crypto = crypto_cond("f");
    [
        format!("sum(f.amount_minor) FILTER (WHERE {MONEY})"),
        format!("sum(f.amount_minor - f.refunded_minor) FILTER (WHERE {MONEY})"),
        format!("count(*) FILTER (WHERE {MONEY})"),
        "sum(f.amount_minor - f.refunded_minor) FILTER (WHERE f.stars)".into(),
        format!("sum(f.promo_minor) FILTER (WHERE {MONEY})"),
        format!("sum(f.discount_minor) FILTER (WHERE {MONEY})"),
        format!("sum(f.amount_minor - f.refunded_minor) FILTER (WHERE {MONEY} AND {crypto})"),
        "sum(f.amount_minor) FILTER (WHERE f.provider = 'balance' AND NOT f.stars)".into(),
        "count(*) FILTER (WHERE f.stars)".into(),
    ]
}

const GROSS: usize = 0;
const NET: usize = 1;
const PAYS: usize = 2;
const STARS: usize = 3;
const PROMO: usize = 4;
const DISCOUNT: usize = 5;
const CRYPTO: usize = 6;
const BALANCE: usize = 7;
const STAR_PAYS: usize = 8;

pub async fn facts_series(c: &Ctx<'_>) -> StoreResult<(Series, Option<Series>)> {
    let values = facts_values();
    let refs: Vec<&str> = values.iter().map(String::as_str).collect();
    both(
        c,
        "analytics_payments f",
        "f.paid_at",
        |sql| c.filters.facts(sql, "f"),
        &refs,
    )
    .await
}

type DimRow = (
    i32,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i32>,
    Option<String>,
    Option<String>,
    Option<String>,
    f64,
    i64,
);

async fn dims(c: &Ctx<'_>, w: &Window) -> StoreResult<Vec<DimRow>> {
    let mut sql = Sql::default();
    let s = sql.time(w.start());
    let e = sql.time(w.end());
    let f = c.filters.facts(&mut sql, "f");
    let text = format!(
        "SELECT GROUPING(f.kind, f.item, f.tariff_id, f.period_days, f.provider, f.method, f.channel)::int, \
           f.kind, f.item, f.tariff_id, f.period_days, f.provider, f.method, f.channel, \
           COALESCE(sum(f.amount_minor), 0)::float8, count(*)::bigint \
         FROM analytics_payments f \
         WHERE f.paid_at >= {s} AND f.paid_at < {e} AND {MONEY} AND {f} \
         GROUP BY GROUPING SETS ((f.kind, f.item), (f.tariff_id), (f.period_days), (f.provider), \
           (f.provider, f.method), (f.channel))"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

struct Cut {
    key: String,
    label: String,
    value: f64,
    count: i64,
}

fn cuts(c: &Ctx<'_>, rows: &[DimRow], set: i32) -> Vec<Cut> {
    let mut out: Vec<Cut> = Vec::new();
    for (g, kind, item, tariff, period, provider, method, channel, value, count) in rows {
        if *g != set {
            continue;
        }
        let (key, label) = match set {
            31 => {
                let (k, l) =
                    labels::kind(kind.as_deref().unwrap_or(""), item.as_deref().unwrap_or(""));
                (k.to_string(), l.to_string())
            }
            111 => (
                tariff.map_or_else(|| "none".into(), |t| t.to_string()),
                c.names.tariff(*tariff),
            ),
            119 => (
                period.map_or_else(|| "none".into(), |p| p.to_string()),
                labels::period(period.map(i64::from)),
            ),
            123 => {
                let p = provider.clone().unwrap_or_default();
                (p.clone(), labels::provider(&c.titles, &p))
            }
            121 => {
                let p = provider.clone().unwrap_or_default();
                let m = method.clone().unwrap_or_default();
                (
                    format!("{p}/{m}"),
                    format!(
                        "{} · {}",
                        labels::provider(&c.titles, &p),
                        labels::method(&c.titles, &p, &m)
                    ),
                )
            }
            _ => {
                let ch = channel.clone().unwrap_or_default();
                (
                    if ch.is_empty() {
                        "unknown".into()
                    } else {
                        ch.clone()
                    },
                    labels::channel(&ch).to_string(),
                )
            }
        };
        if let Some(existing) = out.iter_mut().find(|x| x.key == key) {
            existing.value += value;
            existing.count += count;
        } else {
            out.push(Cut {
                key,
                label,
                value: *value,
                count: *count,
            });
        }
    }
    out.sort_by(|a, b| b.value.total_cmp(&a.value));
    out
}

const DIM_SETS: [(i32, &str, &str, &str); 6] = [
    (31, "kind", "Вид оплаты", "kind"),
    (111, "tariff", "Тариф", "tariff"),
    (119, "period", "Срок", "period"),
    (123, "provider", "Платёжка", "provider"),
    (121, "method", "Способ", "method"),
    (126, "channel", "Канал", "channel"),
];

fn breakdowns(
    c: &Ctx<'_>,
    now: &[DimRow],
    before: Option<&Vec<DimRow>>,
    set_name: &str,
) -> Vec<Breakdown> {
    DIM_SETS
        .iter()
        .map(|(set, key, title, slice)| {
            let prev = before.map(|b| cuts(c, b, *set));
            Breakdown {
                key,
                title,
                rows: cuts(c, now, *set)
                    .into_iter()
                    .map(|cut| BRow {
                        prev: prev
                            .as_ref()
                            .map(|p| p.iter().find(|x| x.key == cut.key).map_or(0.0, |x| x.value)),
                        list: Some(
                            ListRef::payments(set_name)
                                .slice(format!("{slice}:{}", cut.key))
                                .dates(c.range.dates.from, c.range.dates.to),
                        ),
                        key: cut.key,
                        label: cut.label,
                        value: cut.value,
                        count: Some(cut.count),
                    })
                    .collect(),
            }
        })
        .collect()
}

pub async fn mrr_points(c: &Ctx<'_>, w: &Window, upper: DateTime<Utc>) -> StoreResult<Vec<f64>> {
    let mut sql = Sql::default();
    let b0 = sql.time(w.start());
    let f = c.filters.spans(&mut sql, "s");
    let text = format!(
        "SELECT 0, COALESCE(sum(s.monthly_minor), 0)::float8 FROM analytics_spans s \
         WHERE s.starts_at < {b0} AND s.ends_at >= {b0} AND {f}"
    );
    let mut conn = c.store.conn().await?;
    let base = super::query::grid(&mut conn, &sql, &text, 1)
        .await?
        .first()
        .map_or(0.0, |r| r.1[0]);
    let mut sql = Sql::default();
    let b = sql.times(w.bounds.clone());
    let lo = sql.time(w.start());
    let hi = sql.time(w.end().min(upper));
    let f1 = c.filters.spans(&mut sql, "s");
    let text = format!(
        "SELECT width_bucket(e.at, {b}), COALESCE(sum(e.d), 0)::float8 FROM ( \
           SELECT s.starts_at AS at, s.monthly_minor AS d FROM analytics_spans s \
             WHERE s.starts_at >= {lo} AND s.starts_at < {hi} AND {f1} \
           UNION ALL \
           SELECT s.ends_at, -s.monthly_minor FROM analytics_spans s \
             WHERE s.ends_at >= {lo} AND s.ends_at < {hi} AND {f1} \
         ) e GROUP BY 1"
    );
    let deltas = super::query::grid(&mut conn, &sql, &text, 1).await?;
    let steps = super::query::spread(w.len(), &deltas, 0);
    let mut acc = base;
    Ok(steps
        .into_iter()
        .map(|d| {
            acc += d;
            acc
        })
        .collect())
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Moves {
    pub new: (f64, f64),
    pub expansion: (f64, f64),
    pub contraction: (f64, f64),
    pub churn: (f64, f64),
    pub returned: (f64, f64),
}

pub async fn mrr_moves(c: &Ctx<'_>, t0: DateTime<Utc>, t1: DateTime<Utc>) -> StoreResult<Moves> {
    let mut sql = Sql::default();
    let a = sql.time(t0);
    let b = sql.time(t1);
    let fa = c.filters.spans(&mut sql, "s");
    let text = format!(
        "WITH a AS (SELECT s.client_id, sum(s.monthly_minor) AS m FROM analytics_spans s \
                    WHERE s.starts_at < {a} AND s.ends_at >= {a} AND {fa} GROUP BY 1), \
              b AS (SELECT s.client_id, sum(s.monthly_minor) AS m FROM analytics_spans s \
                    WHERE s.starts_at < {b} AND s.ends_at >= {b} AND {fa} GROUP BY 1), \
              j AS MATERIALIZED (SELECT COALESCE(a.m, 0) AS m0, COALESCE(b.m, 0) AS m1, \
                    (COALESCE(a.m, 0) = 0 AND COALESCE(b.m, 0) > 0 AND EXISTS (SELECT 1 FROM analytics_spans x \
                      WHERE x.client_id = b.client_id AND x.starts_at < {a})) AS back \
                    FROM a FULL JOIN b ON a.client_id = b.client_id) \
         SELECT 0, \
           COALESCE(sum(j.m1) FILTER (WHERE j.m0 = 0 AND j.m1 > 0 AND NOT j.back), 0)::float8, \
           count(*) FILTER (WHERE j.m0 = 0 AND j.m1 > 0 AND NOT j.back)::float8, \
           COALESCE(sum(j.m1 - j.m0) FILTER (WHERE j.m0 > 0 AND j.m1 > j.m0), 0)::float8, \
           count(*) FILTER (WHERE j.m0 > 0 AND j.m1 > j.m0)::float8, \
           COALESCE(sum(j.m0 - j.m1) FILTER (WHERE j.m1 > 0 AND j.m1 < j.m0), 0)::float8, \
           count(*) FILTER (WHERE j.m1 > 0 AND j.m1 < j.m0)::float8, \
           COALESCE(sum(j.m0) FILTER (WHERE j.m0 > 0 AND j.m1 = 0), 0)::float8, \
           count(*) FILTER (WHERE j.m0 > 0 AND j.m1 = 0)::float8, \
           COALESCE(sum(j.m1) FILTER (WHERE j.back), 0)::float8, \
           count(*) FILTER (WHERE j.back)::float8 \
         FROM j"
    );
    let mut conn = c.store.conn().await?;
    let r = super::query::grid(&mut conn, &sql, &text, 10).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 10]);
    Ok(Moves {
        new: (v[0], v[1]),
        expansion: (v[2], v[3]),
        contraction: (v[4], v[5]),
        churn: (v[6], v[7]),
        returned: (v[8], v[9]),
    })
}

async fn active_clients(c: &Ctx<'_>, start: DateTime<Utc>, end: DateTime<Utc>) -> StoreResult<f64> {
    let mut sql = Sql::default();
    let s = sql.time(start);
    let e = sql.time(end);
    let fs = c.filters.spans(&mut sql, "s");
    let ft = c.filters.tariff_cond(&mut sql, "sb.tariff_id");
    let fc = c.filters.client(&mut sql, "sb.client_id");
    let text = format!(
        "SELECT 0, count(*)::float8 FROM ( \
           SELECT s.client_id FROM analytics_spans s WHERE s.starts_at < {e} AND s.ends_at > {s} AND {fs} \
           UNION \
           SELECT sb.client_id FROM subscriptions sb WHERE sb.kind <> 'paid' AND sb.created_at < {e} \
             AND COALESCE(sb.ended_at, sb.expire_at, 'infinity'::timestamptz) > {s} AND {ft} AND {fc} \
         ) x"
    );
    let mut conn = c.store.conn().await?;
    Ok(super::query::grid(&mut conn, &sql, &text, 1)
        .await?
        .first()
        .map_or(0.0, |r| r.1[0]))
}

async fn payers(c: &Ctx<'_>, start: DateTime<Utc>, end: DateTime<Utc>) -> StoreResult<f64> {
    let mut sql = Sql::default();
    let s = sql.time(start);
    let e = sql.time(end);
    let f = c.filters.facts(&mut sql, "f");
    let text = format!(
        "SELECT 0, count(DISTINCT f.client_id)::float8 FROM analytics_payments f \
         WHERE f.paid_at >= {s} AND f.paid_at < {e} AND {MONEY} AND {f}"
    );
    let mut conn = c.store.conn().await?;
    Ok(super::query::grid(&mut conn, &sql, &text, 1)
        .await?
        .first()
        .map_or(0.0, |r| r.1[0]))
}

async fn lifetime(c: &Ctx<'_>, upper: DateTime<Utc>) -> StoreResult<(f64, f64)> {
    let mut sql = Sql::default();
    let u = sql.time(upper);
    let f = c.filters.facts(&mut sql, "f");
    let text = format!(
        "SELECT 0, COALESCE(sum(f.amount_minor - f.refunded_minor), 0)::float8, count(DISTINCT f.client_id)::float8 \
         FROM analytics_payments f WHERE f.paid_at < {u} AND {MONEY} AND {f}"
    );
    let mut conn = c.store.conn().await?;
    let r = super::query::grid(&mut conn, &sql, &text, 2).await?;
    Ok(r.first().map_or((0.0, 0.0), |r| (r.1[0], r.1[1])))
}

struct Payouts {
    accrued: Series,
    accrued_prev: Option<Series>,
    withdrawn: f64,
    spent: f64,
}

async fn payouts(c: &Ctx<'_>) -> StoreResult<Payouts> {
    let cur = c.currency.code();
    let (accrued, accrued_prev) = both(
        c,
        "referral_credits r",
        "r.credited_at",
        |sql| format!("r.currency = {}", sql.text(cur)),
        &["sum(r.amount_minor - r.reversed_minor)", "count(*)"],
    )
    .await?;
    let (start, end) = (c.range.start(), c.range.end());
    let mut conn = c.store.conn().await?;
    let withdrawn: Option<i64> = sqlx::query_scalar(
        "SELECT sum(amount_minor)::bigint FROM referral_withdrawals \
         WHERE status = 'paid' AND decided_at >= $1 AND decided_at < $2 AND currency = $3",
    )
    .bind(start)
    .bind(end)
    .bind(cur)
    .fetch_one(conn.conn())
    .await?;
    let spent: Option<i64> = sqlx::query_scalar(
        "SELECT (-sum(delta_minor))::bigint FROM balance_entries \
         WHERE wallet = 'referral' AND kind = 'purchase' AND created_at >= $1 AND created_at < $2 AND currency = $3",
    )
    .bind(start)
    .bind(end)
    .bind(cur)
    .fetch_one(conn.conn())
    .await?;
    Ok(Payouts {
        accrued,
        accrued_prev,
        withdrawn: num(withdrawn.unwrap_or(0)),
        spent: num(spent.unwrap_or(0)),
    })
}

struct Losses {
    refunds: Series,
    refunds_prev: Option<Series>,
    disputes: (f64, f64),
    disputes_prev: Option<(f64, f64)>,
    declined: f64,
}

async fn losses(c: &Ctx<'_>) -> StoreResult<Losses> {
    let cur = c.currency.code();
    let cond = |sql: &mut Sql| {
        let k = sql.text(cur);
        let mut parts = vec![format!("r.currency = {k}")];
        if let Some(p) = &c.filters.provider {
            let p = sql.arg(Arg::Text(p.clone()));
            parts.push(format!("p.provider = {p}"));
        }
        parts.push(c.filters.client(sql, "p.client_id"));
        parts.join(" AND ")
    };
    let (refunds, refunds_prev) = both(
        c,
        "refunds r JOIN payments p ON p.id = r.payment_id",
        "r.created_at",
        cond,
        &[
            "sum(r.amount_minor)",
            "count(*)",
            "sum(r.amount_minor) FILTER (WHERE r.method = 'balance')",
            "sum(r.amount_minor) FILTER (WHERE r.method = 'provider')",
        ],
    )
    .await?;
    let disputes_in = |start: DateTime<Utc>, end: DateTime<Utc>| async move {
        let mut conn = c.store.conn().await?;
        let row: (i64, Option<i64>) = sqlx::query_as(
            "SELECT count(*), sum(amount_minor) FILTER (WHERE currency = $3)::bigint FROM payment_notices \
             WHERE kind IN ('dispute', 'chargeback') AND created_at >= $1 AND created_at < $2",
        )
        .bind(start)
        .bind(end)
        .bind(cur)
        .fetch_one(conn.conn())
        .await?;
        StoreResult::Ok((num(row.0), num(row.1.unwrap_or(0))))
    };
    let disputes = disputes_in(c.range.start(), c.range.end()).await?;
    let disputes_prev = match c.range.prev_window() {
        Some((s, e)) => Some(disputes_in(s, e).await?),
        None => None,
    };
    let mut conn = c.store.conn().await?;
    let declined: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM provider_refunds WHERE status = 'declined' AND resolved_at >= $1 AND resolved_at < $2",
    )
    .bind(c.range.start())
    .bind(c.range.end())
    .fetch_one(conn.conn())
    .await?;
    Ok(Losses {
        refunds,
        refunds_prev,
        disputes,
        disputes_prev,
        declined: num(declined),
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Forecast {
    pub month: NaiveDate,
    pub received: f64,
    pub renewals: f64,
    pub other: f64,
}

impl Forecast {
    pub fn total(&self) -> f64 {
        self.received + self.renewals + self.other
    }
}

pub async fn forecast(c: &Ctx<'_>) -> StoreResult<Forecast> {
    let now = c.range.now;
    let offset = c.range.offset;
    let today = local_day(now, offset);
    let month = today.with_day(1).unwrap_or(today);
    let next = month.checked_add_months(Months::new(1)).unwrap_or(month);
    let month_start = super::query::midnight(month, offset);
    let month_end = super::query::midnight(next, offset);
    let days_left = (month_end - now).num_seconds().max(0) as f64 / 86_400.0;
    let mut sql = Sql::default();
    let ms = sql.time(month_start);
    let nw = sql.time(now);
    let ago28 = sql.time(now - Duration::days(28));
    let ago90 = sql.time(now - Duration::days(90));
    let me = sql.time(month_end);
    let f = c.filters.facts(&mut sql, "f");
    let fs = c.filters.spans(&mut sql, "s");
    let text = format!(
        "SELECT 0, \
           (SELECT COALESCE(sum(f.amount_minor), 0) FROM analytics_payments f \
              WHERE f.paid_at >= {ms} AND f.paid_at < {nw} AND {MONEY} AND {f})::float8, \
           (SELECT COALESCE(sum(f.amount_minor), 0) FROM analytics_payments f \
              WHERE f.paid_at >= {ago28} AND f.paid_at < {nw} AND f.kind <> 'renewal' AND {MONEY} AND {f})::float8, \
           (SELECT count(*) FROM analytics_spans s WHERE s.kind = 'renewal' AND s.continues \
              AND s.starts_at >= {ago90} AND s.starts_at < {nw} AND {fs})::float8, \
           (SELECT count(*) FROM analytics_spans s WHERE s.ends_at >= {ago90} AND s.ends_at < {nw} AND {fs})::float8, \
           (SELECT COALESCE(sum(s.monthly_minor * s.period_days / 30.0), 0) FROM analytics_spans s \
              WHERE s.ends_at >= {nw} AND s.ends_at < {me} AND s.starts_at < {nw} AND {fs})::float8"
    );
    let mut conn = c.store.conn().await?;
    let r = super::query::grid(&mut conn, &sql, &text, 5).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 5]);
    let rate = if v[3] > 0.0 {
        (v[2] / v[3]).min(1.0)
    } else {
        0.0
    };
    Ok(Forecast {
        month,
        received: v[0],
        renewals: (rate * v[4]).round(),
        other: (v[1] / 28.0 * days_left).round(),
    })
}

async fn promo_days(c: &Ctx<'_>) -> StoreResult<f64> {
    let mut conn = c.store.conn().await?;
    let days: Option<i64> = sqlx::query_scalar(
        "SELECT sum(pc.days)::bigint FROM promo_redemptions r JOIN promo_codes pc ON pc.id = r.promo_id \
         WHERE r.status = 'used' AND pc.kind = 'days' AND r.used_at >= $1 AND r.used_at < $2",
    )
    .bind(c.range.start())
    .bind(c.range.end())
    .fetch_one(conn.conn())
    .await?;
    Ok(num(days.unwrap_or(0)))
}

const MONTHS: [&str; 12] = [
    "январь",
    "февраль",
    "март",
    "апрель",
    "май",
    "июнь",
    "июль",
    "август",
    "сентябрь",
    "октябрь",
    "ноябрь",
    "декабрь",
];

fn ratio_points(labels: &[NaiveDate], a: &[f64], b: &[f64]) -> Vec<Point> {
    labels
        .iter()
        .zip(a.iter().zip(b.iter()))
        .map(|(at, (x, y))| Point {
            at: *at,
            value: if *y > 0.0 { (x / y).round() } else { 0.0 },
        })
        .collect()
}

pub async fn build(c: &Ctx<'_>) -> StoreResult<Vec<Group>> {
    let now_w = c.now();
    let prev_w = c.before();
    let moment = c.range.moment();
    let mrr_prev = async {
        match &prev_w {
            Some(w) => Ok(Some(mrr_points(c, w, w.end()).await?)),
            None => StoreResult::Ok(None),
        }
    };
    let dims_prev = async {
        match &prev_w {
            Some(w) => Ok(Some(dims(c, w).await?)),
            None => StoreResult::Ok(None),
        }
    };
    let prev_active = async {
        match c.range.prev_window() {
            Some((s, e)) => Ok(Some((
                active_clients(c, s, e).await?,
                payers(c, s, e).await?,
            ))),
            None => StoreResult::Ok(None),
        }
    };
    let ltv_prev = async {
        match c.range.prev_window() {
            Some((_, e)) => Ok(Some(lifetime(c, e).await?)),
            None => StoreResult::Ok(None),
        }
    };
    let (
        (series, prev),
        dims_now,
        dims_before,
        mrr,
        mrr_before,
        moves,
        active,
        paying,
        before_active,
        ltv,
        ltv_before,
        pay,
        loss,
        days,
    ) = tokio::try_join!(
        facts_series(c),
        dims(c, &now_w),
        dims_prev,
        mrr_points(c, &now_w, moment),
        mrr_prev,
        mrr_moves(c, c.range.start(), moment),
        active_clients(c, c.range.start(), c.range.end()),
        payers(c, c.range.start(), c.range.end()),
        prev_active,
        lifetime(c, moment),
        ltv_prev,
        payouts(c),
        losses(c),
        promo_days(c),
    )?;
    let fc = if c.range.live() {
        Some(forecast(c).await?)
    } else {
        None
    };
    let pay_note = c.filters.ignored(&PAYMENT_DIMS);
    let sub_note = c.filters.ignored(&SUBSCRIPTION_DIMS);
    let gross = series.total(GROSS);
    let pays = series.total(PAYS);
    let revenue = Metric::new("revenue", "Выручка", Unit::Money, Some(gross))
        .prev(prev_total(&prev, GROSS))
        .hint("Все оплаты, кроме звёзд и покупок с баланса")
        .note_opt(pay_note.clone())
        .detail(Detail::Series {
            points: series.points(GROSS),
            prev: prev_points(&prev, GROSS),
            breakdowns: breakdowns(c, &dims_now, dims_before.as_ref(), "revenue"),
            list: Some(ListRef::payments("revenue").dates(c.range.dates.from, c.range.dates.to)),
        });
    let net = Metric::new("net", "без возвратов", Unit::Money, Some(series.total(NET)))
        .prev(prev_total(&prev, NET))
        .sub()
        .detail(Detail::Series {
            points: series.points(NET),
            prev: prev_points(&prev, NET),
            breakdowns: Vec::new(),
            list: None,
        });
    let mrr_value = mrr.last().copied().unwrap_or(0.0);
    let mrr_metric = Metric::new("mrr", "MRR на конец периода", Unit::Money, Some(mrr_value))
        .prev(mrr_before.as_ref().and_then(|p| p.last().copied()))
        .hint("Оплаченные подписки в пересчёте на месяц")
        .note_opt(sub_note.clone())
        .detail(Detail::Series {
            points: super::metric::points(&now_w.labels, &mrr),
            prev: match (&prev_w, &mrr_before) {
                (Some(w), Some(p)) => super::metric::points(&w.labels, p),
                _ => Vec::new(),
            },
            breakdowns: Vec::new(),
            list: None,
        });
    let moved =
        moves.new.0 + moves.expansion.0 + moves.returned.0 - moves.contraction.0 - moves.churn.0;
    let move_row = |label: &str, v: (f64, f64), sign: f64, set: &str| Row {
        label: label.into(),
        sub: None,
        cells: vec![Some(v.0 * sign), Some(v.1)],
        list: Some(ListRef::clients(set).dates(c.range.dates.from, c.range.dates.to)),
        children: Vec::new(),
    };
    let moves_metric = Metric::new("mrr_moves", "Движение MRR", Unit::Money, Some(moved))
        .note_opt(sub_note.clone())
        .detail(Detail::Table {
            columns: vec![
                Column::new("MRR", Unit::Money),
                Column::new("Клиентов", Unit::Count),
            ],
            rows: vec![
                move_row("Новые", moves.new, 1.0, "mrr_new"),
                move_row("Рост", moves.expansion, 1.0, "mrr_up"),
                move_row("Снижение", moves.contraction, -1.0, "mrr_down"),
                move_row("Ушли", moves.churn, -1.0, "mrr_churn"),
                move_row("Вернулись", moves.returned, 1.0, "mrr_back"),
                Row {
                    label: "Итого".into(),
                    sub: None,
                    cells: vec![Some(moved), None],
                    list: None,
                    children: Vec::new(),
                },
            ],
            note: Some(
                "Сравнение месячной стоимости подписок каждого клиента в начале и в конце периода"
                    .into(),
            ),
        });
    let check = Metric::new("check", "Средний чек", Unit::Money, ratio(gross, pays))
        .prev(
            prev.as_ref()
                .and_then(|p| ratio(p.total(GROSS), p.total(PAYS))),
        )
        .detail(Detail::Series {
            points: ratio_points(
                &series.labels,
                &series.columns[GROSS],
                &series.columns[PAYS],
            ),
            prev: prev
                .as_ref()
                .map(|p| ratio_points(&p.labels, &p.columns[GROSS], &p.columns[PAYS]))
                .unwrap_or_default(),
            breakdowns: Vec::new(),
            list: None,
        });
    let table2 = |a: &str, av: f64, au: Unit, b: &str, bv: f64, total: &str, tv: Option<f64>| {
        Detail::Table {
            columns: vec![Column::new("Значение", Unit::Money)],
            rows: vec![
                Row {
                    label: a.into(),
                    sub: None,
                    cells: vec![Some(av)],
                    list: None,
                    children: Vec::new(),
                },
                Row {
                    label: b.into(),
                    sub: Some(match au {
                        Unit::Count => "клиентов".into(),
                        _ => String::new(),
                    }),
                    cells: vec![Some(bv)],
                    list: None,
                    children: Vec::new(),
                },
                Row {
                    label: total.into(),
                    sub: None,
                    cells: vec![tv],
                    list: None,
                    children: Vec::new(),
                },
            ],
            note: None,
        }
    };
    let arpu = Metric::new(
        "arpu",
        "ARPU — на активного",
        Unit::Money,
        ratio(gross, active),
    )
    .prev(match (&prev, before_active) {
        (Some(p), Some((a, _))) => ratio(p.total(GROSS), a),
        _ => None,
    })
    .hint("Выручка на клиента с активной подпиской любого вида")
    .detail(table2(
        "Выручка",
        gross,
        Unit::Money,
        "Клиентов с подпиской",
        active,
        "ARPU",
        ratio(gross, active),
    ));
    let arppu = Metric::new(
        "arppu",
        "ARPPU — на платящего",
        Unit::Money,
        ratio(gross, paying),
    )
    .prev(match (&prev, before_active) {
        (Some(p), Some((_, b))) => ratio(p.total(GROSS), b),
        _ => None,
    })
    .hint("Выручка на клиента, который платил в этом периоде")
    .detail(table2(
        "Выручка",
        gross,
        Unit::Money,
        "Платили",
        paying,
        "ARPPU",
        ratio(gross, paying),
    ));
    let ltv_metric = Metric::new(
        "ltv",
        "LTV — за всё время",
        Unit::Money,
        ratio(ltv.0, ltv.1),
    )
    .prev(ltv_before.and_then(|(a, b)| ratio(a, b)))
    .hint("Сколько в среднем принёс платящий клиент за всё время, за вычетом возвратов")
    .detail(table2(
        "Выручка за всё время",
        ltv.0,
        Unit::Money,
        "Платящих за всё время",
        ltv.1,
        "LTV",
        ratio(ltv.0, ltv.1),
    ));
    let promo = Metric::new(
        "promo",
        "Скидки по промокодам",
        Unit::Money,
        Some(series.total(PROMO)),
    )
    .prev(prev_total(&prev, PROMO))
    .note_opt(if days > 0.0 {
        Some(format!(
            "и {} по промокодам на дни",
            labels::count(days as i64, "день", "дня", "дней")
        ))
    } else {
        pay_note.clone()
    })
    .detail(Detail::Series {
        points: series.points(PROMO),
        prev: prev_points(&prev, PROMO),
        breakdowns: Vec::new(),
        list: Some(ListRef::payments("promo").dates(c.range.dates.from, c.range.dates.to)),
    });
    let discount = Metric::new(
        "discount",
        "Другие скидки",
        Unit::Money,
        Some(series.total(DISCOUNT)),
    )
    .prev(prev_total(&prev, DISCOUNT))
    .hint("Личные скидки и скидки за устройства")
    .detail(Detail::Series {
        points: series.points(DISCOUNT),
        prev: prev_points(&prev, DISCOUNT),
        breakdowns: Vec::new(),
        list: None,
    });
    let accrued = pay.accrued.total(0);
    let referral = Metric::new("referral", "Выплаты рефералам", Unit::Money, Some(accrued))
        .prev(pay.accrued_prev.as_ref().map(|p| p.total(0)))
        .hint("Начислено приглашавшим за оплаты их друзей")
        .note_opt(c.filters.ignored(&super::query::NO_DIMS))
        .detail(Detail::Series {
            points: pay.accrued.points(0),
            prev: pay
                .accrued_prev
                .as_ref()
                .map(|p| p.points(0))
                .unwrap_or_default(),
            breakdowns: vec![Breakdown {
                key: "kind",
                title: "Куда",
                rows: vec![
                    BRow {
                        key: "accrued".into(),
                        label: "Начислено".into(),
                        value: accrued,
                        count: Some(pay.accrued.total(1) as i64),
                        prev: None,
                        list: None,
                    },
                    BRow {
                        key: "withdrawn".into(),
                        label: "Выведено деньгами".into(),
                        value: pay.withdrawn,
                        count: None,
                        prev: None,
                        list: None,
                    },
                    BRow {
                        key: "spent".into(),
                        label: "Потрачено на покупки".into(),
                        value: pay.spent,
                        count: None,
                        prev: None,
                        list: None,
                    },
                ],
            }],
            list: None,
        });
    let refunds_sum = loss.refunds.total(0);
    let lost = refunds_sum + loss.disputes.1;
    let refunds = Metric::new("refunds", "Возвраты и споры", Unit::Money, Some(lost))
        .prev(match (&loss.refunds_prev, loss.disputes_prev) {
            (Some(p), Some(d)) => Some(p.total(0) + d.1),
            (Some(p), None) => Some(p.total(0)),
            _ => None,
        })
        .note(share(lost, gross).map_or_else(String::new, |s| format!("{} выручки", pct_text(s))))
        .detail(Detail::Series {
            points: loss.refunds.points(0),
            prev: loss
                .refunds_prev
                .as_ref()
                .map(|p| p.points(0))
                .unwrap_or_default(),
            breakdowns: vec![Breakdown {
                key: "kind",
                title: "Вид",
                rows: vec![
                    BRow {
                        key: "balance".into(),
                        label: "Возвраты на баланс".into(),
                        value: loss.refunds.total(2),
                        count: None,
                        prev: None,
                        list: None,
                    },
                    BRow {
                        key: "provider".into(),
                        label: "Возвраты в платёжку".into(),
                        value: loss.refunds.total(3),
                        count: None,
                        prev: None,
                        list: None,
                    },
                    BRow {
                        key: "disputes".into(),
                        label: "Споры и чарджбэки".into(),
                        value: loss.disputes.1,
                        count: Some(loss.disputes.0 as i64),
                        prev: None,
                        list: None,
                    },
                    BRow {
                        key: "declined".into(),
                        label: "Платёжка отказала в возврате".into(),
                        value: 0.0,
                        count: Some(loss.declined as i64),
                        prev: None,
                        list: None,
                    },
                ],
            }],
            list: Some(ListRef::payments("refunds").dates(c.range.dates.from, c.range.dates.to)),
        });
    let stars = Metric::new(
        "stars",
        "Звёзды Telegram, шт.",
        Unit::Stars,
        Some(series.total(STARS)),
    )
    .prev(prev_total(&prev, STARS))
    .note(format!("{} оплат", series.total(STAR_PAYS) as i64))
    .detail(Detail::Series {
        points: series.points(STARS),
        prev: prev_points(&prev, STARS),
        breakdowns: Vec::new(),
        list: Some(ListRef::payments("stars").dates(c.range.dates.from, c.range.dates.to)),
    });
    let crypto = Metric::new(
        "crypto",
        "Криптовалюта",
        Unit::Money,
        Some(series.total(CRYPTO)),
    )
    .prev(prev_total(&prev, CRYPTO))
    .hint("Оплаты через криптовалютные платёжки и способы")
    .detail(Detail::Series {
        points: series.points(CRYPTO),
        prev: prev_points(&prev, CRYPTO),
        breakdowns: Vec::new(),
        list: Some(ListRef::payments("crypto").dates(c.range.dates.from, c.range.dates.to)),
    });
    let balance = Metric::new(
        "balance",
        "Оплачено с баланса",
        Unit::Money,
        Some(series.total(BALANCE)),
    )
    .prev(prev_total(&prev, BALANCE))
    .hint("В выручку не входит: деньги учтены при пополнении баланса")
    .detail(Detail::Series {
        points: series.points(BALANCE),
        prev: prev_points(&prev, BALANCE),
        breakdowns: Vec::new(),
        list: Some(ListRef::payments("balance").dates(c.range.dates.from, c.range.dates.to)),
    });
    let mut extra = vec![stars, crypto, balance];
    if let Some(fc) = fc {
        let month = MONTHS[fc.month.month0() as usize];
        extra.push(
            Metric::new("forecast", &format!("Прогноз на {month}"), Unit::Money, Some(fc.total()))
                .note("оценка")
                .detail(Detail::Table {
                    columns: vec![Column::new("Сумма", Unit::Money)],
                    rows: vec![
                        Row { label: "Уже получено".into(), sub: None, cells: vec![Some(fc.received)], list: None, children: Vec::new() },
                        Row { label: "Ожидаем продлений".into(), sub: Some("подписки, которые закончатся до конца месяца, с поправкой на долю продлений за 3 месяца".into()), cells: vec![Some(fc.renewals)], list: None, children: Vec::new() },
                        Row { label: "Ожидаем других оплат".into(), sub: Some("новые продажи, пополнения и допы — по среднему за последние 4 недели".into()), cells: vec![Some(fc.other)], list: None, children: Vec::new() },
                        Row { label: "Прогноз".into(), sub: None, cells: vec![Some(fc.total())], list: None, children: Vec::new() },
                    ],
                    note: None,
                }),
        );
    }
    Ok(vec![
        Group::new(None, vec![revenue, net]),
        Group::new(Some("Подписки"), vec![mrr_metric, moves_metric]),
        Group::new(Some("На клиента"), vec![check, arpu, arppu, ltv_metric]),
        Group::new(
            Some("Скидки, выплаты, потери"),
            vec![promo, discount, referral, refunds],
        ),
        Group::new(Some("Отдельно"), extra),
    ])
}
