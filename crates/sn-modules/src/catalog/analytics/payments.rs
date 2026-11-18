use super::labels;
use super::metric::{Bar, Column, Detail, Group, ListRef, Metric, Row, Unit, num, share};
use super::query::{Arg, Dim, Sql, rows};
use super::tab::{Ctx, both, prev_points, prev_total};
use sn_store::StoreResult;

const APPLIES: [Dim; 4] = [Dim::Provider, Dim::Method, Dim::Channel, Dim::Source];

const PAID: &str = "p.status IN ('paid', 'partially_refunded', 'refunded')";

fn cond(c: &Ctx<'_>, sql: &mut Sql) -> String {
    let mut parts = Vec::new();
    if let Some(pr) = &c.filters.provider {
        let v = sql.arg(Arg::Text(pr.clone()));
        parts.push(format!("p.provider = {v}"));
    }
    if let Some(m) = &c.filters.method {
        let v = sql.arg(Arg::Text(m.clone()));
        parts.push(format!("p.method = {v}"));
    }
    if let Some(ch) = &c.filters.channel {
        let v = sql.arg(Arg::Text(if ch == "unknown" {
            String::new()
        } else {
            ch.clone()
        }));
        parts.push(format!("p.channel = {v}"));
    }
    parts.push(c.filters.client(sql, "p.client_id"));
    parts.join(" AND ")
}

fn period(c: &Ctx<'_>, sql: &mut Sql) -> String {
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let f = cond(c, sql);
    format!("p.created_at >= {s} AND p.created_at < {e} AND {f}")
}

type LineRow = (
    Option<String>,
    Option<String>,
    i32,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
);

const GROUPS: &str = "GROUP BY GROUPING SETS ((p.provider, p.method), (p.provider), ())";

async fn lines(c: &Ctx<'_>) -> StoreResult<Vec<LineRow>> {
    let mut sql = Sql::default();
    let now = sql.time(c.range.now);
    let w = period(c, &mut sql);
    let text = format!(
        "SELECT p.provider, p.method, GROUPING(p.provider, p.method)::int, count(*)::float8, \
           count(*) FILTER (WHERE {PAID})::float8, \
           count(*) FILTER (WHERE p.status = 'canceled' OR (p.status = 'pending' AND p.expires_at < {now}))::float8, \
           count(*) FILTER (WHERE p.status = 'failed')::float8, \
           COALESCE(sum(p.refunded_minor) FILTER (WHERE {PAID}), 0)::float8, \
           COALESCE(sum(p.amount_minor) FILTER (WHERE {PAID}), 0)::float8 \
         FROM analytics_attempts p WHERE {w} {GROUPS}"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

type WaitRow = (
    Option<String>,
    Option<String>,
    i32,
    Option<f64>,
    Option<f64>,
    f64,
    f64,
    f64,
    f64,
    f64,
);

async fn waits(c: &Ctx<'_>) -> StoreResult<Vec<WaitRow>> {
    let mut sql = Sql::default();
    let w = period(c, &mut sql);
    let text = format!(
        "SELECT p.provider, p.method, GROUPING(p.provider, p.method)::int, \
           percentile_cont(0.5) WITHIN GROUP (ORDER BY p.wait_seconds), \
           percentile_disc(0.9) WITHIN GROUP (ORDER BY p.wait_seconds)::float8, \
           count(*) FILTER (WHERE p.wait_seconds < 60)::float8, \
           count(*) FILTER (WHERE p.wait_seconds >= 60 AND p.wait_seconds < 300)::float8, \
           count(*) FILTER (WHERE p.wait_seconds >= 300 AND p.wait_seconds < 900)::float8, \
           count(*) FILTER (WHERE p.wait_seconds >= 900 AND p.wait_seconds < 3600)::float8, \
           count(*) FILTER (WHERE p.wait_seconds >= 3600)::float8 \
         FROM analytics_attempts p WHERE {w} AND p.wait_seconds IS NOT NULL {GROUPS}"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

async fn reasons(c: &Ctx<'_>) -> StoreResult<Vec<(String, String, f64)>> {
    let mut sql = Sql::default();
    let w = period(c, &mut sql);
    let text = format!(
        "SELECT p.provider, COALESCE(p.reason, 'Без причины'), count(*)::float8 \
         FROM analytics_attempts p WHERE {w} AND p.status = 'failed' \
         GROUP BY 1, 2 ORDER BY 3 DESC, 2 LIMIT 30"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

async fn disputes(c: &Ctx<'_>) -> StoreResult<Vec<(String, i64)>> {
    let mut conn = c.store.conn().await?;
    Ok(sqlx::query_as(
        "SELECT provider, count(*)::bigint FROM payment_notices \
         WHERE kind IN ('dispute', 'chargeback') AND created_at >= $1 AND created_at < $2 GROUP BY 1",
    )
    .bind(c.range.start())
    .bind(c.range.end())
    .fetch_all(sn_store::Db::conn(&mut conn))
    .await?)
}

async fn rejected(c: &Ctx<'_>) -> StoreResult<Vec<(String, String, i64)>> {
    let mut conn = c.store.conn().await?;
    Ok(sqlx::query_as(
        "SELECT source, outcome, count(*)::bigint FROM webhook_inbox \
         WHERE received_at >= $1 AND received_at < $2 \
           AND outcome NOT IN ('received', 'accepted', 'duplicate', 'ignored') GROUP BY 1, 2 ORDER BY 3 DESC",
    )
    .bind(c.range.start())
    .bind(c.range.end())
    .fetch_all(sn_store::Db::conn(&mut conn))
    .await?)
}

#[derive(Default, Clone)]
struct Line {
    invoices: f64,
    paid: f64,
    abandoned: f64,
    failed: f64,
    wait: Option<f64>,
    refunded: f64,
    revenue: f64,
    disputes: f64,
}

impl Line {
    fn from_row(r: &LineRow, waits: &[WaitRow]) -> Line {
        Line {
            invoices: r.3,
            paid: r.4,
            abandoned: r.5,
            failed: r.6,
            wait: waits
                .iter()
                .find(|w| w.2 == r.2 && w.0 == r.0 && w.1 == r.1)
                .and_then(|w| w.3),
            refunded: r.7,
            revenue: r.8,
            disputes: 0.0,
        }
    }

    fn row(&self, label: String, list: ListRef, total: f64, children: Vec<Row>) -> Row {
        Row {
            label,
            sub: None,
            cells: vec![
                Some(self.revenue),
                share(self.revenue, total),
                Some(self.invoices),
                Some(self.paid),
                share(self.paid, self.invoices),
                Some(self.abandoned),
                Some(self.failed),
                self.wait,
                Some(self.refunded),
                Some(self.disputes),
            ],
            list: Some(list),
            children,
        }
    }
}

type ProviderLines = (String, Line, Vec<(String, Line)>);

pub async fn build(c: &Ctx<'_>) -> StoreResult<Vec<Group>> {
    let (found, wait, reasons, disp, rej, (inv, inv_prev)) = tokio::try_join!(
        lines(c),
        waits(c),
        reasons(c),
        disputes(c),
        rejected(c),
        both(
            c,
            "analytics_attempts p",
            "p.created_at",
            |sql| cond(c, sql),
            &[
                "count(*)",
                "count(*) FILTER (WHERE p.status = 'canceled' OR (p.status = 'pending' AND p.expires_at < now()))",
                "count(*) FILTER (WHERE p.status = 'failed')",
                "count(*) FILTER (WHERE p.status IN ('paid', 'partially_refunded', 'refunded'))"
            ]
        ),
    )?;
    let mut total = Line::default();
    let mut providers: Vec<ProviderLines> = Vec::new();
    for r in &found {
        match r.2 {
            3 => total = Line::from_row(r, &wait),
            1 => providers.push((
                r.0.clone().unwrap_or_default(),
                Line::from_row(r, &wait),
                Vec::new(),
            )),
            _ => {}
        }
    }
    for r in found.iter().filter(|r| r.2 == 0) {
        let provider = r.0.clone().unwrap_or_default();
        if let Some(p) = providers.iter_mut().find(|p| p.0 == provider) {
            p.2.push((r.1.clone().unwrap_or_default(), Line::from_row(r, &wait)));
        }
    }
    for (prov, n) in &disp {
        total.disputes += num(*n);
        if let Some(p) = providers.iter_mut().find(|p| &p.0 == prov) {
            p.1.disputes += num(*n);
        }
    }
    providers.sort_by(|a, b| {
        b.1.revenue
            .total_cmp(&a.1.revenue)
            .then(b.1.invoices.total_cmp(&a.1.invoices))
    });
    for p in &mut providers {
        p.2.sort_by(|a, b| b.1.invoices.total_cmp(&a.1.invoices).then(a.0.cmp(&b.0)));
    }
    let (from, to) = (c.range.dates.from, c.range.dates.to);
    let table_rows: Vec<Row> = providers
        .iter()
        .map(|(p, line, methods)| {
            let children = if methods.len() > 1 {
                methods
                    .iter()
                    .map(|(m, l)| {
                        l.row(
                            labels::method(&c.titles, p, m),
                            ListRef::payments("invoices")
                                .slice(format!("method:{p}/{m}"))
                                .dates(from, to),
                            total.revenue,
                            Vec::new(),
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            line.row(
                labels::provider(&c.titles, p),
                ListRef::payments("invoices")
                    .slice(format!("provider:{p}"))
                    .dates(from, to),
                total.revenue,
                children,
            )
        })
        .collect();
    let columns = vec![
        Column::new("Выручка", Unit::Money),
        Column::new("Доля выручки", Unit::Percent),
        Column::new("Счетов", Unit::Count),
        Column::new("Оплачено", Unit::Count),
        Column::new("Доля оплаченных", Unit::Percent),
        Column::new("Брошено", Unit::Count),
        Column::new("Ошибки", Unit::Count),
        Column::new("До оплаты", Unit::Seconds),
        Column::new("Возвраты", Unit::Money),
        Column::new("Споры", Unit::Count),
    ];
    let note = c.filters.ignored(&APPLIES);
    let table = Detail::Table {
        columns,
        rows: table_rows,
        note: Some("Счёт — попытка оплаты, созданная за период. Брошенный — клиент открыл и не оплатил, срок истёк. Время до оплаты — медиана. Нажмите на платёжку — откроются её счета.".into()),
    };
    let invoices_metric = Metric::new(
        "invoices",
        "Счета и оплаты",
        Unit::Count,
        Some(inv.total(0)),
    )
    .prev(prev_total(&inv_prev, 0))
    .note_opt(note.clone())
    .detail(table.clone());
    let paid_share = Metric::new(
        "paid_share",
        "Доля оплаченных",
        Unit::Percent,
        share(inv.total(3), inv.total(0)),
    )
    .prev(
        inv_prev
            .as_ref()
            .and_then(|i| share(i.total(3), i.total(0))),
    )
    .note(format!(
        "{} из {}",
        inv.total(3) as i64,
        inv.total(0) as i64
    ))
    .detail(Detail::Series {
        points: inv.points(3),
        prev: prev_points(&inv_prev, 3),
        breakdowns: Vec::new(),
        list: None,
    });
    let abandoned = Metric::new(
        "abandoned",
        "Брошенные счета",
        Unit::Count,
        Some(inv.total(1)),
    )
    .prev(prev_total(&inv_prev, 1))
    .detail(Detail::Series {
        points: inv.points(1),
        prev: prev_points(&inv_prev, 1),
        breakdowns: Vec::new(),
        list: Some(ListRef::payments("abandoned").dates(from, to)),
    });
    let errors = Metric::new("failed", "Ошибки оплаты", Unit::Count, Some(inv.total(2)))
        .prev(prev_total(&inv_prev, 2))
        .detail(Detail::Table {
            columns: vec![Column::new("Раз", Unit::Count)],
            rows: reasons
                .iter()
                .take(30)
                .map(|(p, reason, n)| Row {
                    label: reason.clone(),
                    sub: Some(labels::provider(&c.titles, p)),
                    cells: vec![Some(*n)],
                    list: None,
                    children: Vec::new(),
                })
                .collect(),
            note: Some("Причины — как их прислала платёжка".into()),
        });
    let (p90, quick, few, quarter, hour, longer) = wait
        .iter()
        .find(|w| w.2 == 3)
        .map_or((None, 0.0, 0.0, 0.0, 0.0, 0.0), |w| {
            (w.4, w.5, w.6, w.7, w.8, w.9)
        });
    let wait_bars = [
        ("До минуты", quick),
        ("1–5 минут", few),
        ("5–15 минут", quarter),
        ("15–60 минут", hour),
        ("Дольше часа", longer),
    ]
    .into_iter()
    .map(|(label, value)| Bar {
        label: label.into(),
        value,
        list: None,
    })
    .collect();
    let waits = Metric::new("time_to_pay", "Время до оплаты", Unit::Seconds, total.wait)
        .note(match p90 {
            Some(v) => format!("90 % — до {}", labels::duration(v as i64)),
            None => "медиана".into(),
        })
        .detail(Detail::Bars {
            bars: wait_bars,
            note: Some("От создания счёта до оплаты".into()),
        });
    let rejected_total: f64 = rej.iter().map(|r| num(r.2)).sum();
    let rejected_metric = Metric::new(
        "rejected",
        "Отклонённые уведомления",
        Unit::Count,
        Some(rejected_total),
    )
    .hint("Уведомления платёжек, которые панель не приняла. Журнал хранится 30 дней.")
    .detail(Detail::Table {
        columns: vec![Column::new("Раз", Unit::Count)],
        rows: rej
            .iter()
            .map(|(source, outcome, n)| Row {
                label: labels::outcome(outcome).into(),
                sub: Some(labels::provider(&c.titles, source)),
                cells: vec![Some(num(*n))],
                list: None,
                children: Vec::new(),
            })
            .collect(),
        note: None,
    });
    let refunds = Metric::new(
        "provider_refunds",
        "Возвраты",
        Unit::Money,
        Some(total.refunded),
    )
    .note(format!("споров: {}", total.disputes as i64))
    .detail(table);
    Ok(vec![
        Group::new(None, vec![invoices_metric, paid_share, abandoned, errors]),
        Group::new(
            Some("Скорость и потери"),
            vec![waits, refunds, rejected_metric],
        ),
    ])
}
