use super::labels;
use super::metric::{
    BRow, Bar, Breakdown, Column, Detail, Group, ListRef, Metric, Row, Unit, num, share,
};
use super::query::{CLIENT_DIMS, SUBSCRIPTION_DIMS, Sql, grid, rows};
use super::snapshots;
use super::tab::{Ctx, Window, both, prev_points, prev_total};
use chrono::{DateTime, Duration, Utc};
use sn_store::StoreResult;

const GB: i64 = 1_073_741_824;

type GroupRow = (i32, Option<String>, Option<String>, Option<String>, i64);

async fn reg_groups(c: &Ctx<'_>, w: &Window) -> StoreResult<Vec<GroupRow>> {
    let mut sql = Sql::default();
    let s = sql.time(w.start());
    let e = sql.time(w.end());
    let f = c.filters.source_cond(&mut sql, "ac");
    let text = format!(
        "SELECT GROUPING(ac.reg_channel, ac.login_kind, ac.source_type)::int, ac.reg_channel, ac.login_kind, \
           ac.source_type, count(*)::bigint FROM analytics_clients ac \
         WHERE ac.created_at >= {s} AND ac.created_at < {e} AND NOT ac.deleted AND {f} \
         GROUP BY GROUPING SETS ((ac.reg_channel), (ac.login_kind), (ac.source_type))"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

fn reg_breakdowns(c: &Ctx<'_>, now: &[GroupRow], before: Option<&Vec<GroupRow>>) -> Vec<Breakdown> {
    let sets: [(i32, &'static str, &'static str); 3] = [
        (3, "channel", "Где"),
        (5, "login", "Вход"),
        (6, "source", "Источник"),
    ];
    sets.iter()
        .map(|(set, key, title)| {
            let pick = |rows: &[GroupRow]| -> Vec<(String, String, i64)> {
                let mut out: Vec<(String, String, i64)> = Vec::new();
                for (g, ch, login, src, n) in rows {
                    if g != set {
                        continue;
                    }
                    let (k, l) = match *key {
                        "channel" => {
                            let v = ch.clone().unwrap_or_default();
                            (v.clone(), labels::registration(&v).to_string())
                        }
                        "login" => {
                            let v = login.clone().unwrap_or_default();
                            (v.clone(), labels::login(&v).to_string())
                        }
                        _ => {
                            let v = src.clone().unwrap_or_default();
                            (v.clone(), labels::source_type(&v).to_string())
                        }
                    };
                    match out.iter_mut().find(|x| x.1 == l) {
                        Some(x) => x.2 += n,
                        None => out.push((k, l, *n)),
                    }
                }
                out.sort_by(|a, b| b.2.cmp(&a.2));
                out
            };
            let prev = before.map(|b| pick(b));
            Breakdown {
                key,
                title,
                rows: pick(now)
                    .into_iter()
                    .map(|(k, l, n)| BRow {
                        prev: prev
                            .as_ref()
                            .map(|p| p.iter().find(|x| x.1 == l).map_or(0.0, |x| num(x.2))),
                        list: Some(
                            ListRef::clients("registered")
                                .slice(format!("{key}:{k}"))
                                .dates(c.range.dates.from, c.range.dates.to),
                        ),
                        key: k,
                        label: l,
                        value: num(n),
                        count: None,
                    })
                    .collect(),
            }
        })
        .collect()
}

pub struct Trials {
    pub total: f64,
    pub bought: f64,
    pub waits: Vec<i64>,
}

pub async fn trials(c: &Ctx<'_>, start: DateTime<Utc>, end: DateTime<Utc>) -> StoreResult<Trials> {
    let mut sql = Sql::default();
    let s = sql.time(start);
    let e = sql.time(end);
    let f = c.filters.client(&mut sql, "o.client_id");
    let text = format!(
        "SELECT o.paid_at, (SELECT min(sp.paid_at) FROM analytics_spans sp \
           WHERE sp.client_id = o.client_id AND sp.paid_at >= o.paid_at) \
         FROM orders o WHERE o.kind = 'trial' AND o.status IN ('paid', 'fulfilled', 'review') \
           AND o.paid_at >= {s} AND o.paid_at < {e} AND {f}"
    );
    let mut conn = c.store.conn().await?;
    let found: Vec<(DateTime<Utc>, Option<DateTime<Utc>>)> = rows(&mut conn, &sql, &text).await?;
    let waits: Vec<i64> = found
        .iter()
        .filter_map(|(t, b)| b.map(|b| (b - *t).num_seconds()))
        .collect();
    Ok(Trials {
        total: num(i64::try_from(found.len()).unwrap_or(0)),
        bought: num(i64::try_from(waits.len()).unwrap_or(0)),
        waits,
    })
}

pub fn median(values: &[i64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let mid = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        (num(v[mid - 1]) + num(v[mid])) / 2.0
    } else {
        num(v[mid])
    })
}

fn wait_bars(waits: &[i64]) -> Vec<Bar> {
    let day = 86_400;
    let edges: [(&str, i64, i64); 5] = [
        ("В тот же день", 0, day),
        ("1–3 дня", day, 4 * day),
        ("4–7 дней", 4 * day, 8 * day),
        ("8–30 дней", 8 * day, 31 * day),
        ("Позже", 31 * day, i64::MAX),
    ];
    edges
        .iter()
        .map(|(label, lo, hi)| Bar {
            label: (*label).into(),
            value: num(waits.iter().filter(|w| **w >= *lo && **w < *hi).count() as i64),
            list: None,
        })
        .collect()
}

type SubRow = (
    String,
    Option<i64>,
    Option<i32>,
    Option<i32>,
    Option<i64>,
    Option<i64>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    i64,
);

async fn active_subs(c: &Ctx<'_>) -> StoreResult<Vec<SubRow>> {
    let mut sql = Sql::default();
    let now = sql.time(c.range.now);
    let t = c.filters.tariff_cond(&mut sql, "s.tariff_id");
    let f = c.filters.client(&mut sql, "s.client_id");
    let text = format!(
        "SELECT s.kind, s.tariff_id, s.devices_count, s.hwid_limit, s.traffic_used_bytes, s.traffic_limit_bytes, \
           s.first_connected_at, s.online_at, s.client_id \
         FROM subscriptions s \
         WHERE s.ended_at IS NULL AND COALESCE(s.status, 'active') IN ('active', 'limited') \
           AND (s.expire_at IS NULL OR s.expire_at > {now}) AND {t} AND {f}"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

struct Wallets {
    main: (f64, f64),
    referral: (f64, f64),
    blocked_now: f64,
    blocked_in: f64,
    bot: f64,
    en: f64,
    ru: f64,
}

async fn wallets(c: &Ctx<'_>) -> StoreResult<Wallets> {
    let mut sql = Sql::default();
    let cur = sql.text(c.currency.code());
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let fb = c.filters.client(&mut sql, "b.client_id");
    let fc = c.filters.client(&mut sql, "cl.id");
    let text = format!(
        "SELECT 0, \
           (SELECT COALESCE(sum(b.amount_minor), 0) FROM client_balances b WHERE b.wallet = 'main' AND b.currency = {cur} AND {fb})::float8, \
           (SELECT count(*) FROM client_balances b WHERE b.wallet = 'main' AND b.currency = {cur} AND b.amount_minor > 0 AND {fb})::float8, \
           (SELECT COALESCE(sum(b.amount_minor), 0) FROM client_balances b WHERE b.wallet = 'referral' AND b.currency = {cur} AND {fb})::float8, \
           (SELECT count(*) FROM client_balances b WHERE b.wallet = 'referral' AND b.currency = {cur} AND b.amount_minor > 0 AND {fb})::float8, \
           (SELECT count(*) FROM clients cl WHERE cl.status = 'blocked' AND cl.deleted_at IS NULL AND {fc})::float8, \
           (SELECT count(*) FROM clients cl WHERE cl.blocked_at >= {s} AND cl.blocked_at < {e} AND {fc})::float8, \
           (SELECT count(*) FROM clients cl WHERE cl.bot_unreachable AND cl.deleted_at IS NULL AND {fc})::float8, \
           (SELECT count(*) FROM clients cl WHERE cl.locale = 'en' AND cl.deleted_at IS NULL AND {fc})::float8, \
           (SELECT count(*) FROM clients cl WHERE cl.locale = 'ru' AND cl.deleted_at IS NULL AND {fc})::float8"
    );
    let mut conn = c.store.conn().await?;
    let r = grid(&mut conn, &sql, &text, 9).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 9]);
    Ok(Wallets {
        main: (v[0], v[1]),
        referral: (v[2], v[3]),
        blocked_now: v[4],
        blocked_in: v[5],
        bot: v[6],
        en: v[7],
        ru: v[8],
    })
}

fn bars(values: &[(&str, f64)], _set: &str, _c: &Ctx<'_>) -> Vec<Bar> {
    values
        .iter()
        .map(|(label, value)| Bar {
            label: (*label).into(),
            value: *value,
            list: None,
        })
        .collect()
}

pub async fn build(c: &Ctx<'_>) -> StoreResult<Vec<Group>> {
    let now_w = c.now();
    let prev_w = c.before();
    let groups_prev = async {
        match &prev_w {
            Some(w) => Ok(Some(reg_groups(c, w).await?)),
            None => StoreResult::Ok(None),
        }
    };
    let trials_prev = async {
        match c.range.prev_window() {
            Some((s, e)) => Ok(Some(trials(c, s, e).await?)),
            None => StoreResult::Ok(None),
        }
    };
    let (
        (regs, regs_prev),
        (tr, tr_prev),
        groups_now,
        groups_before,
        trial,
        trial_before,
        subs,
        wal,
    ) = tokio::try_join!(
        both(
            c,
            "analytics_clients ac",
            "ac.created_at",
            |sql| format!("NOT ac.deleted AND {}", c.filters.source_cond(sql, "ac")),
            &["count(*)"]
        ),
        both(
            c,
            "orders o",
            "o.paid_at",
            |sql| format!(
                "o.kind = 'trial' AND o.status IN ('paid', 'fulfilled', 'review') AND {}",
                c.filters.client(sql, "o.client_id")
            ),
            &["count(*)"]
        ),
        reg_groups(c, &now_w),
        groups_prev,
        trials(c, c.range.start(), c.range.end()),
        trials_prev,
        active_subs(c),
        wallets(c),
    )?;
    let history = snapshots::series(
        c,
        &[
            "subs.paid",
            "subs.trial",
            "subs.gift",
            "subs.promo",
            "subs.manual",
            "subs.custom",
        ],
    )
    .await?;
    let client_note = c.filters.ignored(&CLIENT_DIMS);
    let sub_note = c.filters.ignored(&SUBSCRIPTION_DIMS);
    let registrations = Metric::new(
        "registrations",
        "Регистрации",
        Unit::Count,
        Some(regs.total(0)),
    )
    .prev(prev_total(&regs_prev, 0))
    .note_opt(client_note.clone())
    .detail(Detail::Series {
        points: regs.points(0),
        prev: prev_points(&regs_prev, 0),
        breakdowns: reg_breakdowns(c, &groups_now, groups_before.as_ref()),
        list: Some(ListRef::clients("registered").dates(c.range.dates.from, c.range.dates.to)),
    });
    let trials_metric = Metric::new("trials", "Пробные периоды", Unit::Count, Some(tr.total(0)))
        .prev(prev_total(&tr_prev, 0))
        .note_opt(client_note.clone())
        .detail(Detail::Series {
            points: tr.points(0),
            prev: prev_points(&tr_prev, 0),
            breakdowns: Vec::new(),
            list: Some(ListRef::clients("trials").dates(c.range.dates.from, c.range.dates.to)),
        });
    let bought = Metric::new(
        "trial_bought",
        "Купили после пробного",
        Unit::Percent,
        share(trial.bought, trial.total),
    )
    .prev(trial_before.as_ref().and_then(|t| share(t.bought, t.total)))
    .note(format!("{} из {}", trial.bought as i64, trial.total as i64))
    .sub()
    .detail(Detail::Bars {
        bars: wait_bars(&trial.waits),
        note: Some("Сколько прошло от пробного периода до первой покупки".into()),
    });
    let wait = Metric::new(
        "trial_wait",
        "Время до покупки",
        Unit::Seconds,
        median(&trial.waits),
    )
    .prev(trial_before.as_ref().and_then(|t| median(&t.waits)))
    .note("медиана")
    .sub()
    .detail(Detail::Bars {
        bars: wait_bars(&trial.waits),
        note: Some("Сколько прошло от пробного периода до первой покупки".into()),
    });
    let mut by_kind: Vec<(String, f64)> = Vec::new();
    let mut by_tariff: Vec<(Option<i64>, f64, f64)> = Vec::new();
    let mut per_client: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    let mut devices = [0.0_f64; 6];
    let mut at_limit = 0.0;
    let mut traffic = [0.0_f64; 5];
    let mut near = 0.0;
    let mut never = 0.0;
    let mut idle7 = 0.0;
    let mut idle30 = 0.0;
    for (kind, tariff, dev, limit, used, cap, first, online, client) in &subs {
        match by_kind.iter_mut().find(|k| k.0 == *kind) {
            Some(k) => k.1 += 1.0,
            None => by_kind.push((kind.clone(), 1.0)),
        }
        let paid = if kind == "paid" { 1.0 } else { 0.0 };
        match by_tariff.iter_mut().find(|t| t.0 == *tariff) {
            Some(t) => {
                t.1 += 1.0;
                t.2 += paid;
            }
            None => by_tariff.push((*tariff, 1.0, paid)),
        }
        *per_client.entry(*client).or_insert(0) += 1;
        let d = dev.unwrap_or(0);
        let slot = match d {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 3,
            4 | 5 => 4,
            _ => 5,
        };
        devices[slot] += 1.0;
        if let Some(l) = limit
            && *l > 0
            && d >= *l
        {
            at_limit += 1.0;
        }
        let u = used.unwrap_or(0);
        let tslot = if u < GB {
            0
        } else if u < 10 * GB {
            1
        } else if u < 50 * GB {
            2
        } else if u < 200 * GB {
            3
        } else {
            4
        };
        traffic[tslot] += 1.0;
        if let Some(cap) = cap
            && *cap > 0
            && u * 10 >= cap * 9
        {
            near += 1.0;
        }
        match (first, online) {
            (None, _) => never += 1.0,
            (Some(_), Some(o)) => {
                if *o < c.range.now - Duration::days(7) {
                    idle7 += 1.0;
                }
                if *o < c.range.now - Duration::days(30) {
                    idle30 += 1.0;
                }
            }
            (Some(_), None) => {}
        }
    }
    by_kind.sort_by(|a, b| b.1.total_cmp(&a.1));
    by_tariff.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total_subs = num(i64::try_from(subs.len()).unwrap_or(0));
    let multi: f64 = per_client.values().filter(|n| **n >= 2).count() as f64;
    let mut multi_bars = [0.0_f64; 3];
    for n in per_client.values() {
        multi_bars[usize::try_from((*n).clamp(1, 3) - 1).unwrap_or(0)] += 1.0;
    }
    let mut tariff_rows: Vec<Row> = Vec::new();
    for (tariff, all, paid) in &by_tariff {
        tariff_rows.push(Row {
            label: c.names.tariff(*tariff),
            sub: None,
            cells: vec![Some(*paid), Some(*all)],
            list: Some(ListRef::clients("active").slice(format!(
                "tariff:{}",
                tariff.map_or_else(|| "none".into(), |t| t.to_string())
            ))),
            children: Vec::new(),
        });
    }
    let active = Metric::new(
        "active_subs",
        "Активные подписки",
        Unit::Count,
        Some(total_subs),
    )
    .note("сейчас")
    .detail(Detail::Series {
        points: history.0,
        prev: Vec::new(),
        breakdowns: vec![Breakdown {
            key: "kind",
            title: "Вид",
            rows: by_kind
                .iter()
                .map(|(k, v)| BRow {
                    key: k.clone(),
                    label: labels::subscription_kind(k).into(),
                    value: *v,
                    count: None,
                    prev: None,
                    list: Some(ListRef::clients("active").slice(format!("kind:{k}"))),
                })
                .collect(),
        }],
        list: Some(ListRef::clients("active")),
    })
    .hint(
        history
            .1
            .unwrap_or_else(|| "По дням — с первого ночного снимка после обновления".into()),
    );
    let tariffs_metric = Metric::new(
        "subs_tariffs",
        "По тарифам",
        Unit::Count,
        Some(by_tariff.iter().map(|t| t.2).sum()),
    )
    .note("платных")
    .sub()
    .note_opt(sub_note.clone())
    .detail(Detail::Table {
        columns: vec![
            Column {
                title: "Платных".into(),
                unit: Unit::Count,
            },
            Column {
                title: "Всего".into(),
                unit: Unit::Count,
            },
        ],
        rows: tariff_rows,
        note: None,
    });
    let multi_metric = Metric::new("multi", "Несколько подписок", Unit::Count, Some(multi))
        .note("клиентов")
        .sub()
        .detail(Detail::Bars {
            bars: bars(
                &[
                    ("1 подписка", multi_bars[0]),
                    ("2 подписки", multi_bars[1]),
                    ("3 и больше", multi_bars[2]),
                ],
                "multi",
                c,
            ),
            note: None,
        });
    let devices_metric = Metric::new(
        "devices",
        "Упёрлись в лимит устройств",
        Unit::Count,
        Some(at_limit),
    )
    .note(format!("из {}", total_subs as i64))
    .detail(Detail::Bars {
        bars: bars(
            &[
                ("Нет данных", devices[0]),
                ("1 устройство", devices[1]),
                ("2", devices[2]),
                ("3", devices[3]),
                ("4–5", devices[4]),
                ("6 и больше", devices[5]),
            ],
            "devices",
            c,
        ),
        note: Some(
            "Сколько устройств подключено к активным подпискам — по данным Remnawave".into(),
        ),
    });
    let traffic_metric = Metric::new(
        "traffic",
        "Близко к лимиту трафика",
        Unit::Count,
        Some(near),
    )
    .note("90 % и больше")
    .detail(Detail::Bars {
        bars: bars(
            &[
                ("До 1 ГБ", traffic[0]),
                ("1–10 ГБ", traffic[1]),
                ("10–50 ГБ", traffic[2]),
                ("50–200 ГБ", traffic[3]),
                ("Больше 200 ГБ", traffic[4]),
            ],
            "traffic",
            c,
        ),
        note: Some("Расход за текущий цикл сброса трафика, как его считает Remnawave".into()),
    });
    let never_metric = Metric::new("never", "Ни разу не подключались", Unit::Count, Some(never))
        .note_opt(Some(format!(
            "не заходили 7 дней: {}, 30 дней: {}",
            idle7 as i64, idle30 as i64
        )))
        .detail(Detail::Bars {
            bars: bars(
                &[
                    ("Ни разу", never),
                    ("Не заходили 7 дней", idle7),
                    ("Не заходили 30 дней", idle30),
                ],
                "idle",
                c,
            ),
            note: None,
        });
    let balances = Metric::new(
        "balances",
        "Деньги на балансах",
        Unit::Money,
        Some(wal.main.0),
    )
    .note(format!(
        "у {}",
        labels::count(wal.main.1 as i64, "клиента", "клиентов", "клиентов")
    ))
    .detail(Detail::Table {
        columns: vec![
            Column {
                title: "Сумма".into(),
                unit: Unit::Money,
            },
            Column {
                title: "Клиентов".into(),
                unit: Unit::Count,
            },
        ],
        rows: vec![
            Row {
                label: "Основной баланс".into(),
                sub: None,
                cells: vec![Some(wal.main.0), Some(wal.main.1)],
                list: Some(ListRef::clients("balance")),
                children: Vec::new(),
            },
            Row {
                label: "Реферальный баланс".into(),
                sub: None,
                cells: vec![Some(wal.referral.0), Some(wal.referral.1)],
                list: Some(ListRef::clients("referral_balance")),
                children: Vec::new(),
            },
        ],
        note: None,
    });
    let blocked = Metric::new(
        "blocked",
        "Заблокированы",
        Unit::Count,
        Some(wal.blocked_now),
    )
    .note(format!("+{} за период", wal.blocked_in as i64))
    .detail(Detail::Bars {
        bars: bars(
            &[
                ("Заблокированы сейчас", wal.blocked_now),
                ("За период", wal.blocked_in),
            ],
            "blocked",
            c,
        ),
        note: None,
    });
    let bot = Metric::new(
        "bot_blocked",
        "Заблокировали бота",
        Unit::Count,
        Some(wal.bot),
    )
    .hint("Бот не может им написать: рассылки до них не дойдут")
    .detail(Detail::None);
    let lang = Metric::new("english", "Английский язык", Unit::Count, Some(wal.en))
        .note(format!("русский — {}", wal.ru as i64))
        .detail(Detail::Bars {
            bars: vec![
                Bar {
                    label: "Русский".into(),
                    value: wal.ru,
                    list: None,
                },
                Bar {
                    label: "Английский".into(),
                    value: wal.en,
                    list: None,
                },
            ],
            note: None,
        });
    Ok(vec![
        Group::new(None, vec![registrations]),
        Group::new(Some("Пробные"), vec![trials_metric, bought, wait]),
        Group::new(
            Some("Подписки сейчас"),
            vec![
                active,
                tariffs_metric,
                multi_metric,
                devices_metric,
                traffic_metric,
                never_metric,
            ],
        ),
        Group::new(Some("Клиенты сейчас"), vec![balances, blocked, bot, lang]),
    ])
}
