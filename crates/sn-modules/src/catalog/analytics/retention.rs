use super::labels;
use super::metric::{
    Bar, CohortRow, Column, Detail, Group, ListRef, Metric, Row, Unit, num, share,
};
use super::query::{SUBSCRIPTION_DIMS, Sql, grid, local_day, midnight, rows};
use super::spans::{GRACE_DAYS, MONTH_DAYS};
use super::tab::{Ctx, both, prev_points, prev_total};
use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, Utc};
use sn_store::StoreResult;

pub const COLUMNS: usize = 12;

type IslandRow = (i64, DateTime<Utc>, i32, DateTime<Utc>, DateTime<Utc>);
type MoneyRow = (NaiveDate, i32, i64);

pub struct Cohorts {
    pub rows: Vec<CohortRow>,
}

fn month_start(day: NaiveDate) -> NaiveDate {
    day.with_day(1).unwrap_or(day)
}

pub async fn cohorts(c: &Ctx<'_>, last: NaiveDate, count: usize) -> StoreResult<Cohorts> {
    let offset = c.range.offset;
    let last = month_start(last);
    let back = u32::try_from(count.saturating_sub(1)).unwrap_or(0);
    let first = last.checked_sub_months(Months::new(back)).unwrap_or(last);
    let starts: Vec<NaiveDate> = (0..count)
        .filter_map(|i| first.checked_add_months(Months::new(u32::try_from(i).unwrap_or(0))))
        .collect();
    let lo = midnight(first, offset);
    let hi = midnight(
        last.checked_add_months(Months::new(1)).unwrap_or(last),
        offset,
    );
    let mut sql = Sql::default();
    let s = sql.time(lo);
    let e = sql.time(hi);
    let f = c.filters.client(&mut sql, "f.client_id");
    let text = format!(
        "SELECT f.client_id, f.starts_at, i.seq, i.starts_at, i.ends_at \
         FROM analytics_islands f JOIN analytics_islands i ON i.client_id = f.client_id \
         WHERE f.seq = 1 AND f.starts_at >= {s} AND f.starts_at < {e} AND {f} ORDER BY f.client_id, i.seq"
    );
    let shift = sql.int(i64::from(offset.local_minus_utc()));
    let span = sql.int(MONTH_DAYS * 86_400);
    let money_text = format!(
        "SELECT date_trunc('month', f.starts_at + make_interval(secs => {shift}))::date, \
           greatest(1, floor(extract(epoch FROM p.paid_at - f.starts_at) / {span})::int + 1), \
           sum(p.amount_minor - p.refunded_minor)::bigint \
         FROM analytics_islands f JOIN analytics_payments p ON p.client_id = f.client_id \
         WHERE f.seq = 1 AND f.starts_at >= {s} AND f.starts_at < {e} AND NOT p.stars AND p.provider <> 'balance' AND {f} \
         GROUP BY 1, 2"
    );
    let mut conn = c.store.conn().await?;
    let islands: Vec<IslandRow> = rows(&mut conn, &sql, &text).await?;
    let money: Vec<MoneyRow> = rows(&mut conn, &sql, &money_text).await?;
    let month = Duration::days(MONTH_DAYS);
    let now = c.range.now;
    let mut out: Vec<CohortRow> = starts
        .iter()
        .map(|m| CohortRow {
            month: *m,
            size: 0,
            kept: vec![None; COLUMNS],
            money: vec![None; COLUMNS],
        })
        .collect();
    let index = |at: DateTime<Utc>| -> Option<usize> {
        let d = local_day(at, offset);
        starts.iter().rposition(|m| *m <= d)
    };
    let cutoff = |i: usize| -> DateTime<Utc> {
        midnight(
            starts[i]
                .checked_add_months(Months::new(1))
                .unwrap_or(starts[i]),
            offset,
        )
    };
    for (_, first_at, seq, starts_at, ends_at) in &islands {
        let Some(i) = index(*first_at) else {
            continue;
        };
        if *seq == 1 {
            out[i].size += 1;
        }
        for k in 1..=COLUMNS {
            let step = i32::try_from(k).unwrap_or(i32::MAX);
            if cutoff(i) + month * step > now {
                break;
            }
            let at = *first_at + month * step;
            let hit = *starts_at <= at && at < *ends_at;
            let cell = &mut out[i].kept[k - 1];
            *cell = Some(cell.unwrap_or(0) + i64::from(hit));
        }
    }
    for (cohort, from_k, amount) in &money {
        let Some(i) = starts.iter().position(|m| m == cohort) else {
            continue;
        };
        let from_k = usize::try_from(*from_k).unwrap_or(1);
        for k in 1..=COLUMNS {
            let step = i32::try_from(k).unwrap_or(i32::MAX);
            if cutoff(i) + month * step > now {
                break;
            }
            let cell = &mut out[i].money[k - 1];
            let add = if k >= from_k { *amount } else { 0 };
            *cell = Some(cell.unwrap_or(0) + add);
        }
    }
    for row in &mut out {
        if row.size == 0 {
            row.kept = vec![None; COLUMNS];
            row.money = vec![None; COLUMNS];
        }
    }
    out.reverse();
    Ok(Cohorts { rows: out })
}

pub fn kept_share(rows: &[CohortRow], k: usize, from: NaiveDate, to: NaiveDate) -> Option<f64> {
    let (mut kept, mut size) = (0_i64, 0_i64);
    for r in rows {
        if r.month < month_start(from) || r.month > to {
            continue;
        }
        if let Some(Some(v)) = r.kept.get(k - 1) {
            kept += v;
            size += r.size;
        }
    }
    share(num(kept), num(size))
}

pub struct Renewals {
    pub on_time: f64,
    pub late: f64,
    pub back: f64,
}

pub async fn renewals(
    c: &Ctx<'_>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> StoreResult<Renewals> {
    let mut sql = Sql::default();
    let s = sql.time(start);
    let e = sql.time(end);
    let f = c.filters.spans(&mut sql, "sp");
    let text = format!(
        "SELECT 0, \
           count(*) FILTER (WHERE sp.kind = 'renewal' AND sp.continues AND sp.gap_seconds <= 0)::float8, \
           count(*) FILTER (WHERE sp.kind = 'renewal' AND sp.continues AND sp.gap_seconds > 0)::float8, \
           count(*) FILTER (WHERE NOT sp.continues AND sp.gap_seconds IS NOT NULL)::float8 \
         FROM analytics_spans sp WHERE sp.paid_at >= {s} AND sp.paid_at < {e} AND {f}"
    );
    let mut conn = c.store.conn().await?;
    let r = grid(&mut conn, &sql, &text, 3).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 3]);
    Ok(Renewals {
        on_time: v[0],
        late: v[1],
        back: v[2],
    })
}

pub struct Autorenew {
    pub on: f64,
    pub paid: f64,
    pub results: Vec<(String, f64)>,
}

async fn autorenew(c: &Ctx<'_>) -> StoreResult<Autorenew> {
    let mut sql = Sql::default();
    let now = sql.time(c.range.now);
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let t = c.filters.tariff_cond(&mut sql, "sb.tariff_id");
    let f = c.filters.client(&mut sql, "sb.client_id");
    let text = format!(
        "SELECT 0, \
           count(*) FILTER (WHERE sb.autorenew_enabled)::float8, count(*)::float8 \
         FROM subscriptions sb WHERE sb.kind = 'paid' AND sb.ended_at IS NULL \
           AND COALESCE(sb.status, 'active') IN ('active', 'limited') AND (sb.expire_at IS NULL OR sb.expire_at > {now}) \
           AND {t} AND {f}"
    );
    let results_text = format!(
        "SELECT a.result, count(*)::bigint FROM autorenew_attempts a JOIN subscriptions sb ON sb.id = a.subscription_id \
         WHERE a.created_at >= {s} AND a.created_at < {e} AND {t} AND {f} GROUP BY 1"
    );
    let mut conn = c.store.conn().await?;
    let r = grid(&mut conn, &sql, &text, 2).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 2]);
    let results: Vec<(String, i64)> = rows(&mut conn, &sql, &results_text).await?;
    Ok(Autorenew {
        on: v[0],
        paid: v[1],
        results: results.into_iter().map(|(k, n)| (k, num(n))).collect(),
    })
}

pub struct Expiring {
    pub auto7: f64,
    pub manual7: f64,
    pub auto30: f64,
    pub manual30: f64,
    pub sum7: f64,
    pub sum30: f64,
}

async fn expiring(c: &Ctx<'_>) -> StoreResult<Expiring> {
    let mut sql = Sql::default();
    let now = sql.time(c.range.now);
    let d7 = sql.time(c.range.now + Duration::days(7));
    let d30 = sql.time(c.range.now + Duration::days(30));
    let t = c.filters.tariff_cond(&mut sql, "sb.tariff_id");
    let f = c.filters.client(&mut sql, "sb.client_id");
    let text = format!(
        "SELECT 0, \
           count(*) FILTER (WHERE sb.expire_at <= {d7} AND sb.autorenew_enabled)::float8, \
           count(*) FILTER (WHERE sb.expire_at <= {d7} AND NOT sb.autorenew_enabled)::float8, \
           count(*) FILTER (WHERE sb.autorenew_enabled)::float8, \
           count(*) FILTER (WHERE NOT sb.autorenew_enabled)::float8, \
           COALESCE(sum(last.price) FILTER (WHERE sb.expire_at <= {d7}), 0)::float8, \
           COALESCE(sum(last.price), 0)::float8 \
         FROM subscriptions sb \
         LEFT JOIN LATERAL (SELECT o.total_minor AS price FROM orders o WHERE o.subscription_id = sb.id \
           AND o.kind IN ('new', 'renewal', 'change') AND o.paid_at IS NOT NULL ORDER BY o.id DESC LIMIT 1) last ON true \
         WHERE sb.kind = 'paid' AND sb.ended_at IS NULL \
           AND sb.expire_at > {now} AND sb.expire_at <= {d30} AND {t} AND {f}"
    );
    let mut conn = c.store.conn().await?;
    let r = grid(&mut conn, &sql, &text, 6).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 6]);
    Ok(Expiring {
        auto7: v[0],
        manual7: v[1],
        auto30: v[2],
        manual30: v[3],
        sum7: v[4],
        sum30: v[5],
    })
}

async fn paying_at(c: &Ctx<'_>, at: DateTime<Utc>) -> StoreResult<f64> {
    let mut sql = Sql::default();
    let a = sql.time(at);
    let f = c.filters.client(&mut sql, "i.client_id");
    let text = format!(
        "SELECT 0, count(DISTINCT i.client_id)::float8 FROM analytics_islands i \
         WHERE i.starts_at <= {a} AND i.ends_at > {a} AND {f}"
    );
    let mut conn = c.store.conn().await?;
    Ok(grid(&mut conn, &sql, &text, 1)
        .await?
        .first()
        .map_or(0.0, |r| r.1[0]))
}

fn autorenew_label(code: &str) -> &'static str {
    match code {
        "charged" => "Прошли",
        "insufficient" => "Не хватило денег",
        "failed" => "Сбой",
        "skipped" => "Пропущены",
        "reminded" => "Напоминания",
        _ => "Другое",
    }
}

pub async fn build(c: &Ctx<'_>) -> StoreResult<Vec<Group>> {
    let grace = Duration::days(GRACE_DAYS);
    let churn_upper = c.range.now - grace;
    let churn_cond = |sql: &mut Sql| {
        let u = sql.time(churn_upper);
        format!(
            "i.ends_at < {u} AND {}",
            c.filters.client(sql, "i.client_id")
        )
    };
    let back_cond =
        |sql: &mut Sql| format!("i.seq > 1 AND {}", c.filters.client(sql, "i.client_id"));
    let renew_prev = async {
        match c.range.prev_window() {
            Some((s, e)) => Ok(Some(renewals(c, s, e).await?)),
            None => StoreResult::Ok(None),
        }
    };
    let (coh, ren, ren_prev, auto, exp, (churn, churn_prev), (back, back_prev), paying_start) = tokio::try_join!(
        cohorts(c, c.range.dates.to, COLUMNS),
        renewals(c, c.range.start(), c.range.end()),
        renew_prev,
        autorenew(c),
        expiring(c),
        both(
            c,
            "analytics_islands i",
            "i.ends_at",
            churn_cond,
            &["count(DISTINCT i.client_id)"]
        ),
        both(
            c,
            "analytics_islands i",
            "i.starts_at",
            back_cond,
            &["count(DISTINCT i.client_id)"]
        ),
        paying_at(c, c.range.start()),
    )?;
    let note = c.filters.ignored(&SUBSCRIPTION_DIMS);
    let (from, to) = (c.range.dates.from, c.range.dates.to);
    let cohorts_metric = Metric::new("cohorts", "Когорты по месяцу первой оплаты", Unit::Percent, kept_share(&coh.rows, 1, from, to))
        .note("через месяц")
        .hint("Какая часть клиентов, впервые заплативших в этом месяце, через 1, 2, 3… месяца всё ещё с оплаченной подпиской. Пустые клетки — срок ещё не наступил.")
        .note_opt(note.clone())
        .detail(Detail::Cohorts {
            columns: COLUMNS,
            rows: coh.rows.clone(),
        });
    let kept = |key: &'static str, title: &str, k: usize, sub: bool| {
        let m = Metric::new(
            key,
            title,
            Unit::Percent,
            kept_share(&coh.rows, k, from, to),
        )
        .detail(Detail::Cohorts {
            columns: COLUMNS,
            rows: coh.rows.clone(),
        });
        let m = if kept_share(&coh.rows, k, from, to).is_none() {
            m.note("срок не наступил")
        } else {
            m
        };
        if sub { m.sub() } else { m }
    };
    let total = ren.on_time + ren.late + ren.back;
    let total_prev = ren_prev.as_ref().map(|r| r.on_time + r.late + r.back);
    let ren_bars = Detail::Bars {
        bars: vec![
            Bar { label: "Вовремя".into(), value: ren.on_time, list: Some(ListRef::clients("renewed_on_time").dates(from, to)) },
            Bar { label: "Опоздали на 1–3 дня".into(), value: ren.late, list: Some(ListRef::clients("renewed_late").dates(from, to)) },
            Bar { label: "После перерыва".into(), value: ren.back, list: Some(ListRef::clients("returned").dates(from, to)) },
        ],
        note: Some("Продления, оплаченные в периоде: до конца подписки, с опозданием до трёх дней или после перерыва".into()),
    };
    let on_time = Metric::new(
        "on_time",
        "Вовремя",
        Unit::Percent,
        share(ren.on_time, total),
    )
    .prev(
        ren_prev
            .as_ref()
            .and_then(|r| share(r.on_time, total_prev.unwrap_or(0.0))),
    )
    .note(format!("{} продлений", ren.on_time as i64))
    .detail(ren_bars.clone());
    let late = Metric::new(
        "late",
        "Опоздали на 1–3 дня",
        Unit::Percent,
        share(ren.late, total),
    )
    .prev(
        ren_prev
            .as_ref()
            .and_then(|r| share(r.late, total_prev.unwrap_or(0.0))),
    )
    .detail(ren_bars.clone());
    let after = Metric::new(
        "after_break",
        "После перерыва",
        Unit::Percent,
        share(ren.back, total),
    )
    .prev(
        ren_prev
            .as_ref()
            .and_then(|r| share(r.back, total_prev.unwrap_or(0.0))),
    )
    .note(format!("{} чел.", ren.back as i64))
    .detail(ren_bars);
    let tried: f64 = auto
        .results
        .iter()
        .filter(|(k, _)| matches!(k.as_str(), "charged" | "insufficient" | "failed"))
        .map(|(_, n)| *n)
        .sum();
    let charged = auto
        .results
        .iter()
        .find(|(k, _)| k == "charged")
        .map_or(0.0, |r| r.1);
    let poor = auto
        .results
        .iter()
        .find(|(k, _)| k == "insufficient")
        .map_or(0.0, |r| r.1);
    let failed = auto
        .results
        .iter()
        .find(|(k, _)| k == "failed")
        .map_or(0.0, |r| r.1);
    let auto_bars = Detail::Bars {
        bars: auto
            .results
            .iter()
            .map(|(k, n)| Bar {
                label: autorenew_label(k).into(),
                value: *n,
                list: None,
            })
            .collect(),
        note: Some("Попытки автопродления за период".into()),
    };
    let auto_on = Metric::new(
        "autorenew_on",
        "Включено",
        Unit::Percent,
        share(auto.on, auto.paid),
    )
    .note(format!("{} из {}", auto.on as i64, auto.paid as i64))
    .hint("Сейчас, среди активных платных подписок")
    .detail(Detail::Bars {
        bars: vec![
            Bar {
                label: "Включено".into(),
                value: auto.on,
                list: Some(ListRef::clients("autorenew_on")),
            },
            Bar {
                label: "Выключено".into(),
                value: auto.paid - auto.on,
                list: Some(ListRef::clients("autorenew_off")),
            },
        ],
        note: None,
    });
    let auto_ok = Metric::new(
        "autorenew_ok",
        "Списания прошли",
        Unit::Percent,
        share(charged, tried),
    )
    .note(format!("{} из {}", charged as i64, tried as i64))
    .detail(auto_bars.clone());
    let auto_fail = Metric::new(
        "autorenew_fail",
        "Не прошли",
        Unit::Count,
        Some(poor + failed),
    )
    .note(format!("денег нет: {}", poor as i64))
    .detail(auto_bars);
    let churned = churn.total(0);
    let churn_metric = Metric::new("churned", "Ушли", Unit::Count, Some(churned))
        .prev(prev_total(&churn_prev, 0))
        .note_opt(
            share(churned, paying_start)
                .map(|p| format!("{} платящих", super::metric::pct_text(p))),
        )
        .hint("Оплаченный период закончился больше трёх дней назад и не продлён")
        .detail(Detail::Series {
            points: churn.points(0),
            prev: prev_points(&churn_prev, 0),
            breakdowns: Vec::new(),
            list: Some(ListRef::clients("churned").dates(from, to)),
        });
    let back_metric = Metric::new("returned", "Вернулись", Unit::Count, Some(back.total(0)))
        .prev(prev_total(&back_prev, 0))
        .hint("Снова оплатили после перерыва дольше трёх дней")
        .detail(Detail::Series {
            points: back.points(0),
            prev: prev_points(&back_prev, 0),
            breakdowns: Vec::new(),
            list: Some(ListRef::clients("returned").dates(from, to)),
        });
    let exp_table = |a: f64, m: f64, set: &str| Detail::Table {
        columns: vec![Column {
            title: "Подписок".into(),
            unit: Unit::Count,
        }],
        rows: vec![
            Row {
                label: "С автопродлением".into(),
                sub: None,
                cells: vec![Some(a)],
                list: Some(ListRef::clients(set).slice("auto:on")),
                children: Vec::new(),
            },
            Row {
                label: "Без автопродления".into(),
                sub: None,
                cells: vec![Some(m)],
                list: Some(ListRef::clients(set).slice("auto:off")),
                children: Vec::new(),
            },
        ],
        note: Some("Сумма — по последней оплате каждой подписки".into()),
    };
    let exp7 = Metric::new(
        "expiring7",
        "Истекают за 7 дней",
        Unit::Count,
        Some(exp.auto7 + exp.manual7),
    )
    .note(format!("на {}", labels::money_short(exp.sum7, c.currency)))
    .detail(exp_table(exp.auto7, exp.manual7, "expiring7"));
    let exp30 = Metric::new(
        "expiring30",
        "Истекают за 30 дней",
        Unit::Count,
        Some(exp.auto30 + exp.manual30),
    )
    .note(format!("на {}", labels::money_short(exp.sum30, c.currency)))
    .detail(exp_table(exp.auto30, exp.manual30, "expiring30"));
    Ok(vec![
        Group::new(None, vec![cohorts_metric]),
        Group::new(
            Some("Удержание"),
            vec![
                kept("kept1", "Платят через 1 месяц", 1, false),
                kept("kept3", "через 3 месяца", 3, true),
                kept("kept6", "через 6 месяцев", 6, true),
                kept("kept12", "через 12 месяцев", 12, true),
            ],
        ),
        Group::new(Some("Продления"), vec![on_time, late, after]),
        Group::new(Some("Автопродление"), vec![auto_on, auto_ok, auto_fail]),
        Group::new(
            Some("Уход и возврат"),
            vec![churn_metric, back_metric, exp7, exp30],
        ),
    ])
}
