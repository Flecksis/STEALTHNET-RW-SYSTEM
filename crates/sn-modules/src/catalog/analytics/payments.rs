use super::clients::median;
use super::labels;
use super::metric::{Bar, Column, Detail, Group, ListRef, Metric, Row, Unit, num, share};
use super::query::{Arg, Dim, Sql, rows};
use super::tab::{Ctx, both, prev_points, prev_total};
use chrono::{DateTime, Utc};
use sn_store::StoreResult;

const APPLIES: [Dim; 4] = [Dim::Provider, Dim::Method, Dim::Channel, Dim::Source];

fn cond(c: &Ctx<'_>, sql: &mut Sql) -> String {
    let mut parts = vec!["p.provider NOT IN ('balance', 'manual')".to_string()];
    if let Some(pr) = &c.filters.provider {
        let v = sql.arg(Arg::Text(pr.clone()));
        parts.push(format!("p.provider = {v}"));
    }
    if let Some(m) = &c.filters.method {
        let v = sql.arg(Arg::Text(m.clone()));
        parts.push(format!("COALESCE(p.pay_method, '') = {v}"));
    }
    if let Some(ch) = &c.filters.channel {
        let v = sql.arg(Arg::Text(if ch == "unknown" {
            String::new()
        } else {
            ch.clone()
        }));
        parts.push(format!(
            "COALESCE((SELECT o.channel FROM orders o WHERE o.id = p.order_id), '') = {v}"
        ));
    }
    parts.push(c.filters.client(sql, "p.client_id"));
    parts.join(" AND ")
}

type InvoiceRow = (
    String,
    Option<String>,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    i64,
    i64,
    Option<String>,
    String,
);

async fn invoices(c: &Ctx<'_>) -> StoreResult<Vec<InvoiceRow>> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let cur = sql.text(c.currency.code());
    let f = cond(c, &mut sql);
    let text = format!(
        "SELECT p.provider, p.pay_method, p.status, p.created_at, p.paid_at, p.expires_at, \
           CASE WHEN p.currency = {cur} THEN p.amount_minor ELSE 0 END, \
           CASE WHEN p.currency = {cur} THEN p.refunded_minor ELSE 0 END, p.failure_reason, p.currency \
         FROM payments p WHERE p.created_at >= {s} AND p.created_at < {e} AND {f}"
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
    waits: Vec<i64>,
    revenue: f64,
    refunded: f64,
    disputes: f64,
}

impl Line {
    fn add(&mut self, r: &InvoiceRow, now: DateTime<Utc>, money: bool) {
        let (_, _, status, created, paid_at, expires, amount, refunded, _, _) = r;
        self.invoices += 1.0;
        match status.as_str() {
            "paid" | "partially_refunded" | "refunded" => {
                self.paid += 1.0;
                if let Some(p) = paid_at {
                    self.waits.push((*p - *created).num_seconds().max(0));
                }
                if money {
                    self.revenue += num(*amount);
                    self.refunded += num(*refunded);
                }
            }
            "failed" => self.failed += 1.0,
            "canceled" => self.abandoned += 1.0,
            "pending" if expires.is_some_and(|x| x < now) => self.abandoned += 1.0,
            _ => {}
        }
    }

    fn row(&self, label: String, list: ListRef, total: f64, children: Vec<Row>) -> Row {
        Row {
            label,
            sub: None,
            cells: vec![
                Some(self.invoices),
                Some(self.paid),
                share(self.paid, self.invoices),
                Some(self.abandoned),
                Some(self.failed),
                median(&self.waits),
                Some(self.refunded),
                Some(self.disputes),
                Some(self.revenue),
                share(self.revenue, total),
            ],
            list: Some(list),
            children,
        }
    }
}

type ProviderLines = (String, Line, Vec<(String, Line)>);

fn p90(values: &[i64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let i = ((v.len() as f64) * 0.9).ceil() as usize;
    Some(num(v[i.saturating_sub(1).min(v.len() - 1)]))
}

pub async fn build(c: &Ctx<'_>) -> StoreResult<Vec<Group>> {
    let paid_cond = |sql: &mut Sql| {
        format!(
            "p.status IN ('paid', 'partially_refunded', 'refunded') AND {}",
            cond(c, sql)
        )
    };
    let (rows_now, disp, rej, (inv, inv_prev), (paid_s, paid_prev)) = tokio::try_join!(
        invoices(c),
        disputes(c),
        rejected(c),
        both(
            c,
            "payments p",
            "p.created_at",
            |sql| cond(c, sql),
            &[
                "count(*)",
                "count(*) FILTER (WHERE p.status = 'canceled' OR (p.status = 'pending' AND p.expires_at < now()))",
                "count(*) FILTER (WHERE p.status = 'failed')"
            ]
        ),
        both(c, "payments p", "p.created_at", paid_cond, &["count(*)"]),
    )?;
    let now = c.range.now;
    let cur = c.currency.code();
    let mut total = Line::default();
    let mut providers: Vec<ProviderLines> = Vec::new();
    let mut reasons: Vec<(String, String, f64)> = Vec::new();
    for r in &rows_now {
        let money = r.9 == cur;
        total.add(r, now, money);
        let provider = r.0.clone();
        let method = r.1.clone().unwrap_or_default();
        let idx = match providers.iter().position(|p| p.0 == provider) {
            Some(i) => i,
            None => {
                providers.push((provider.clone(), Line::default(), Vec::new()));
                providers.len() - 1
            }
        };
        let entry = &mut providers[idx];
        entry.1.add(r, now, money);
        match entry.2.iter_mut().find(|m| m.0 == method) {
            Some(m) => m.1.add(r, now, money),
            None => {
                let mut l = Line::default();
                l.add(r, now, money);
                entry.2.push((method, l));
            }
        }
        if r.2 == "failed" {
            let reason =
                r.8.clone()
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .unwrap_or_else(|| "Без причины".into());
            let reason: String = reason.chars().take(120).collect();
            match reasons
                .iter_mut()
                .find(|x| x.0 == provider && x.1 == reason)
            {
                Some(x) => x.2 += 1.0,
                None => reasons.push((provider.clone(), reason, 1.0)),
            }
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
    reasons.sort_by(|a, b| b.2.total_cmp(&a.2));
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
        Column {
            title: "Счетов".into(),
            unit: Unit::Count,
        },
        Column {
            title: "Оплачено".into(),
            unit: Unit::Count,
        },
        Column {
            title: "Доля оплаченных".into(),
            unit: Unit::Percent,
        },
        Column {
            title: "Брошено".into(),
            unit: Unit::Count,
        },
        Column {
            title: "Ошибки".into(),
            unit: Unit::Count,
        },
        Column {
            title: "До оплаты".into(),
            unit: Unit::Seconds,
        },
        Column {
            title: "Возвраты".into(),
            unit: Unit::Money,
        },
        Column {
            title: "Споры".into(),
            unit: Unit::Count,
        },
        Column {
            title: "Выручка".into(),
            unit: Unit::Money,
        },
        Column {
            title: "Доля выручки".into(),
            unit: Unit::Percent,
        },
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
        share(paid_s.total(0), inv.total(0)),
    )
    .prev(match (&paid_prev, &inv_prev) {
        (Some(p), Some(i)) => share(p.total(0), i.total(0)),
        _ => None,
    })
    .note(format!(
        "{} из {}",
        paid_s.total(0) as i64,
        inv.total(0) as i64
    ))
    .detail(Detail::Series {
        points: paid_s.points(0),
        prev: prev_points(&paid_prev, 0),
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
            columns: vec![Column {
                title: "Раз".into(),
                unit: Unit::Count,
            }],
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
    let minute = 60;
    let wait_bars = [
        ("До минуты", 0, minute),
        ("1–5 минут", minute, 5 * minute),
        ("5–15 минут", 5 * minute, 15 * minute),
        ("15–60 минут", 15 * minute, 60 * minute),
        ("Дольше часа", 60 * minute, i64::MAX),
    ]
    .iter()
    .map(|(label, lo, hi)| Bar {
        label: (*label).into(),
        value: num(total
            .waits
            .iter()
            .filter(|w| **w >= *lo && **w < *hi)
            .count() as i64),
        list: None,
    })
    .collect();
    let waits = Metric::new(
        "time_to_pay",
        "Время до оплаты",
        Unit::Seconds,
        median(&total.waits),
    )
    .note(match p90(&total.waits) {
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
        columns: vec![Column {
            title: "Раз".into(),
            unit: Unit::Count,
        }],
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
