use super::labels::{self, Names};
use super::metric::{Column, Detail, Group, ListRef, Metric, Row, Unit, num, ratio, share};
use super::query::{CLIENT_DIMS, Filters, Sql, rows};
use super::tab::Ctx;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_store::{Store, StoreResult};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SourceStat {
    pub source_type: String,
    pub source_key: String,
    pub medium: String,
    pub campaign: String,
    pub registrations: i64,
    pub trials: i64,
    pub connected: i64,
    pub paid: i64,
    pub revenue: i64,
    pub revenue_all: i64,
    pub kept_base: i64,
    pub kept: i64,
    pub clicks: i64,
    pub since: Option<DateTime<Utc>>,
}

impl SourceStat {
    fn add(&mut self, o: &SourceStat) {
        self.registrations += o.registrations;
        self.trials += o.trials;
        self.connected += o.connected;
        self.paid += o.paid;
        self.revenue += o.revenue;
        self.revenue_all += o.revenue_all;
        self.kept_base += o.kept_base;
        self.kept += o.kept;
        self.clicks += o.clicks;
        self.since = match (self.since, o.since) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }

    pub fn conversion(&self) -> Option<f64> {
        share(num(self.paid), num(self.registrations))
    }

    pub fn ltv(&self) -> Option<f64> {
        ratio(num(self.revenue_all), num(self.paid))
    }

    pub fn kept_share(&self) -> Option<f64> {
        share(num(self.kept), num(self.kept_base))
    }
}

type StatRow = (
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
);

type Since = (String, String, String, DateTime<Utc>);

pub async fn trial_on(db: &mut impl sn_store::Db) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT (value ->> 'enabled')::boolean FROM settings WHERE key = 'billing.trial'), false)",
    )
    .fetch_one(db.conn())
    .await?)
}

pub async fn stats(
    store: &Store,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    now: DateTime<Utc>,
    filters: &Filters,
) -> StoreResult<Vec<SourceStat>> {
    let mut sql = Sql::default();
    let s = sql.time(start);
    let e = sql.time(end);
    let n = sql.time(now - Duration::days(30));
    let f = filters.source_cond(&mut sql, "ac");
    let text = format!(
        "WITH cohort AS ( \
           SELECT ac.client_id, ac.source_type, ac.source_key, ac.utm_medium, ac.utm_campaign, ac.first_paid_at, \
             ac.trial_at, ac.connected_at, ac.first_island_at, ac.kept30 \
           FROM analytics_clients ac WHERE ac.created_at >= {s} AND ac.created_at < {e} AND NOT ac.deleted AND {f}), \
         money AS ( \
           SELECT p.client_id, \
             COALESCE(sum(p.amount_minor - p.refunded_minor) FILTER (WHERE p.paid_at >= {s} AND p.paid_at < {e}), 0) AS in_range, \
             COALESCE(sum(p.amount_minor - p.refunded_minor), 0) AS total \
           FROM analytics_payments p JOIN cohort USING (client_id) \
           WHERE NOT p.stars AND p.provider <> 'balance' GROUP BY p.client_id) \
         SELECT c.source_type, c.source_key, c.utm_medium, c.utm_campaign, count(*)::bigint, \
           count(c.trial_at)::bigint, count(c.connected_at)::bigint, \
           count(*) FILTER (WHERE c.first_paid_at IS NOT NULL)::bigint, \
           COALESCE(sum(m.in_range), 0)::bigint, COALESCE(sum(m.total), 0)::bigint, \
           count(*) FILTER (WHERE c.first_island_at <= {n})::bigint, \
           count(*) FILTER (WHERE c.first_island_at <= {n} AND c.kept30)::bigint \
         FROM cohort c LEFT JOIN money m USING (client_id) \
         GROUP BY 1, 2, 3, 4"
    );
    let mut conn = store.conn().await?;
    let found: Vec<StatRow> = rows(&mut conn, &sql, &text).await?;
    let mut out: Vec<SourceStat> = found
        .into_iter()
        .map(
            |(t, k, m, c, regs, trials, connected, paid, rev, all, base, kept)| SourceStat {
                source_type: t,
                source_key: k,
                medium: m,
                campaign: c,
                registrations: regs,
                trials,
                connected,
                paid,
                revenue: rev,
                revenue_all: all,
                kept_base: base,
                kept,
                clicks: 0,
                since: None,
            },
        )
        .collect();
    let utm_only = matches!(
        filters.source,
        None | Some(super::query::SourceFilter::Utm { .. })
    );
    if utm_only {
        let clicks: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT source, medium, campaign, sum(clicks)::bigint FROM utm_clicks \
             WHERE at >= $1 AND at < $2 GROUP BY 1, 2, 3",
        )
        .bind(start)
        .bind(end)
        .fetch_all(sn_store::Db::conn(&mut conn))
        .await?;
        for (source, medium, campaign, n) in clicks {
            if let Some(super::query::SourceFilter::Utm {
                source: want,
                campaign: camp,
            }) = &filters.source
                && (want != &source || camp.as_ref().is_some_and(|c| c != &campaign))
            {
                continue;
            }
            match out.iter_mut().find(|s| {
                s.source_type == "utm"
                    && s.source_key == source
                    && s.medium == medium
                    && s.campaign == campaign
            }) {
                Some(s) => s.clicks += n,
                None => out.push(SourceStat {
                    source_type: "utm".into(),
                    source_key: source,
                    medium,
                    campaign,
                    clicks: n,
                    ..SourceStat::default()
                }),
            }
        }
    }
    if utm_only {
        let links: Vec<Since> = sqlx::query_as(
            "SELECT source, COALESCE(medium, ''), COALESCE(campaign, ''), min(created_at) FROM utm_links \
             WHERE archived_at IS NULL GROUP BY 1, 2, 3",
        )
        .fetch_all(sn_store::Db::conn(&mut conn))
        .await?;
        let first: Vec<Since> = sqlx::query_as(
            "SELECT source_key, utm_medium, utm_campaign, min(created_at) FROM analytics_clients \
             WHERE source_type = 'utm' AND NOT deleted GROUP BY 1, 2, 3",
        )
        .fetch_all(sn_store::Db::conn(&mut conn))
        .await?;
        let find = |list: &[Since], s: &SourceStat| {
            list.iter()
                .find(|x| x.0 == s.source_key && x.1 == s.medium && x.2 == s.campaign)
                .map(|x| x.3)
        };
        for st in out.iter_mut().filter(|st| st.source_type == "utm") {
            st.since = find(&links, st).or_else(|| find(&first, st));
        }
        for (source, medium, campaign, created) in links {
            if let Some(super::query::SourceFilter::Utm {
                source: want,
                campaign: camp,
            }) = &filters.source
                && (want != &source || camp.as_ref().is_some_and(|c| c != &campaign))
            {
                continue;
            }
            let known = out.iter().any(|st| {
                st.source_type == "utm"
                    && st.source_key == source
                    && st.medium == medium
                    && st.campaign == campaign
            });
            if !known {
                out.push(SourceStat {
                    source_type: "utm".into(),
                    source_key: source,
                    medium,
                    campaign,
                    since: Some(created),
                    ..SourceStat::default()
                });
            }
        }
    }
    out.sort_by(|a, b| {
        b.registrations
            .cmp(&a.registrations)
            .then(b.clicks.cmp(&a.clicks))
            .then(a.source_key.cmp(&b.source_key))
    });
    Ok(out)
}

pub fn age(since: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<String> {
    let days = (now - since?).num_days().max(0);
    let text = format!("идёт {}", labels::count(days, "день", "дня", "дней"));
    Some(if days < 30 {
        format!("{text} · рано судить")
    } else {
        text
    })
}

const TYPES: [&str; 5] = ["utm", "promo", "ref", "gift", "none"];

fn columns(trials: bool) -> Vec<Column> {
    let mut out = vec![
        Column::new("Перешли", Unit::Count),
        Column::new("Пришли", Unit::Count),
    ];
    if trials {
        out.push(Column::new("Попробовали", Unit::Count).share_of(1));
    }
    out.extend([
        Column::new("Подключились", Unit::Count).share_of(1),
        Column::new("Оплатили", Unit::Count).share_of(1),
        Column::new("Выручка за период", Unit::Money),
        Column::new("LTV", Unit::Money),
        Column::new("Платят через месяц", Unit::Percent),
    ]);
    out
}

fn cells(s: &SourceStat, with_clicks: bool, trials: bool) -> Vec<Option<f64>> {
    let mut out = vec![
        with_clicks.then(|| num(s.clicks)),
        Some(num(s.registrations)),
    ];
    if trials {
        out.push(Some(num(s.trials)));
    }
    out.extend([
        Some(num(s.connected)),
        Some(num(s.paid)),
        Some(num(s.revenue)),
        s.ltv(),
        s.kept_share(),
    ]);
    out
}

fn slice_of(s: &SourceStat, by_campaign: bool) -> String {
    match s.source_type.as_str() {
        "utm" if by_campaign => format!("utm:{}/{}", s.source_key, s.campaign),
        "utm" => format!("utm:{}", s.source_key),
        "promo" => format!("promo:{}", s.source_key),
        other => other.to_string(),
    }
}

pub fn rows_for(
    stats: &[SourceStat],
    kind: Option<&str>,
    names: &Names,
    c: &Ctx<'_>,
    trials: bool,
) -> Vec<Row> {
    let mut groups: Vec<(String, String, SourceStat, Vec<SourceStat>)> = Vec::new();
    for s in stats
        .iter()
        .filter(|s| kind.is_none_or(|k| s.source_type == k))
    {
        let key = (
            s.source_type.clone(),
            if s.source_type == "utm" || s.source_type == "promo" {
                s.source_key.clone()
            } else {
                String::new()
            },
        );
        match groups.iter_mut().find(|g| g.0 == key.0 && g.1 == key.1) {
            Some(g) => {
                g.2.add(s);
                g.3.push(s.clone());
            }
            None => {
                let mut total = s.clone();
                total.medium.clear();
                total.campaign.clear();
                groups.push((key.0, key.1, total, vec![s.clone()]));
            }
        }
    }
    groups.sort_by(|a, b| {
        b.2.registrations
            .cmp(&a.2.registrations)
            .then(b.2.clicks.cmp(&a.2.clicks))
    });
    let (from, to) = (c.range.dates.from, c.range.dates.to);
    groups
        .into_iter()
        .map(|(t, k, total, parts)| {
            let utm = t == "utm";
            let children =
                if utm && (parts.len() > 1 || parts.iter().any(|p| !p.campaign.is_empty())) {
                    parts
                        .iter()
                        .map(|p| Row {
                            label: if p.campaign.is_empty() {
                                "без кампании".into()
                            } else {
                                p.campaign.clone()
                            },
                            sub: join_sub(
                                (!p.medium.is_empty()).then(|| p.medium.clone()),
                                age(p.since, c.range.now),
                            ),
                            cells: cells(p, true, trials),
                            list: Some(
                                ListRef::clients("source")
                                    .slice(slice_of(p, true))
                                    .dates(from, to),
                            ),
                            children: Vec::new(),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            Row {
                label: names.source(&t, &k),
                sub: utm.then(|| {
                    join_sub(Some("UTM-метка".into()), age(total.since, c.range.now))
                        .unwrap_or_default()
                }),
                cells: cells(&total, utm, trials),
                list: Some(
                    ListRef::clients("source")
                        .slice(slice_of(&total, false))
                        .dates(from, to),
                ),
                children,
            }
        })
        .collect()
}

fn join_sub(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a} · {b}")),
        (a, b) => a.or(b),
    }
}

pub async fn build(c: &Ctx<'_>) -> StoreResult<Vec<Group>> {
    let prev = async {
        match c.range.prev_window() {
            Some((s, e)) => Ok(Some(stats(c.store, s, e, c.range.now, &c.filters).await?)),
            None => StoreResult::Ok(None),
        }
    };
    let (now, before) = tokio::try_join!(
        stats(
            c.store,
            c.range.start(),
            c.range.end(),
            c.range.now,
            &c.filters
        ),
        prev
    )?;
    let note = c.filters.ignored(&CLIENT_DIMS);
    let sum = |list: &[SourceStat], kind: Option<&str>| {
        let mut t = SourceStat::default();
        for s in list
            .iter()
            .filter(|s| kind.is_none_or(|k| s.source_type == k))
        {
            t.add(s);
        }
        t
    };
    let all = sum(&now, None);
    let all_prev = before.as_ref().map(|b| sum(b, None));
    let trials = {
        let mut conn = c.store.conn().await?;
        trial_on(&mut conn).await? || now.iter().any(|s| s.trials > 0)
    };
    let table = |kind: Option<&str>| {
        Detail::Table {
        columns: columns(trials),
        rows: rows_for(&now, kind, &c.names, c, trials),
        note: Some(
            "Клиенты, пришедшие за период, и что они сделали потом: взяли пробный период, подключили VPN, оплатили. Доли — от пришедших. Перешли — нажали «Старт» в боте, открыли мини-приложение или сайт по ссылке с меткой. Метке меньше 30 дней — рано судить."
                .into(),
        ),
    }
    };
    let mut metrics = vec![
        Metric::new(
            "sources",
            "Все источники",
            Unit::Count,
            Some(num(all.registrations)),
        )
        .prev(all_prev.as_ref().map(|p| num(p.registrations)))
        .note("регистраций")
        .note_opt(note.clone())
        .detail(table(None)),
        Metric::new(
            "conversion",
            "Конверсия в оплату",
            Unit::Percent,
            all.conversion(),
        )
        .prev(all_prev.as_ref().and_then(SourceStat::conversion))
        .note(format!("{} из {}", all.paid, all.registrations))
        .detail(table(None)),
        Metric::new(
            "connected",
            "Подключились",
            Unit::Percent,
            share(num(all.connected), num(all.registrations)),
        )
        .prev(
            all_prev
                .as_ref()
                .and_then(|p| share(num(p.connected), num(p.registrations))),
        )
        .note(format!("{} из {}", all.connected, all.registrations))
        .hint("Пришедшие за период, у которых хотя бы одна подписка уже подключалась к VPN")
        .detail(table(None)),
    ];
    let mut by_type = Vec::new();
    for kind in TYPES {
        let t = sum(&now, Some(kind));
        let tp = before.as_ref().map(|b| sum(b, Some(kind)));
        let key: &'static str = match kind {
            "utm" => "utm",
            "promo" => "promo_links",
            "ref" => "referrals",
            "gift" => "gifts",
            _ => "unlabeled",
        };
        by_type.push(
            Metric::new(
                key,
                labels::source_type(kind),
                Unit::Count,
                Some(num(t.registrations)),
            )
            .prev(tp.map(|p| num(p.registrations)))
            .note(t.conversion().map_or_else(
                || "регистраций".into(),
                |c| format!("конверсия {}", super::metric::pct_text(c)),
            ))
            .detail(table(Some(kind))),
        );
    }
    let clicks: i64 = now.iter().map(|s| s.clicks).sum();
    let first_click = {
        let mut conn = c.store.conn().await?;
        let first: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT min(at) FROM utm_clicks")
            .fetch_one(sn_store::Db::conn(&mut conn))
            .await?;
        first
    };
    metrics.push(
        Metric::new(
            "clicks",
            "Переходы по меткам",
            Unit::Count,
            Some(num(clicks)),
        )
        .note(match first_click {
            Some(at) => format!(
                "считаются с {}",
                labels::date_text(super::query::local_day(at, c.range.offset))
            ),
            None => "считаются с обновления".into(),
        })
        .detail(table(Some("utm"))),
    );
    Ok(vec![
        Group::new(None, metrics),
        Group::new(Some("По видам источников"), by_type),
    ])
}
