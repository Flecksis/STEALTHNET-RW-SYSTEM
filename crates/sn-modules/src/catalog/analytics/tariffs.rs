use super::labels;
use super::metric::{Column, Detail, Group, ListRef, Metric, Row, Unit, num, ratio};
use super::query::{PAYMENT_DIMS, Sql, grid, rows};
use super::tab::Ctx;
use sn_store::StoreResult;

const SALE: &str =
    "f.kind IN ('new', 'renewal', 'change') AND NOT f.stars AND f.provider <> 'balance'";

type SaleRow = (Option<i64>, Option<i32>, i64, f64);

async fn sales(c: &Ctx<'_>) -> StoreResult<Vec<SaleRow>> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let f = c.filters.facts(&mut sql, "f");
    let text = format!(
        "SELECT f.tariff_id, f.period_days, count(*)::bigint, COALESCE(sum(f.amount_minor), 0)::float8 \
         FROM analytics_payments f WHERE f.paid_at >= {s} AND f.paid_at < {e} AND {SALE} AND {f} \
         GROUP BY 1, 2 ORDER BY 4 DESC"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

async fn changes(c: &Ctx<'_>) -> StoreResult<Vec<(Option<i64>, Option<i64>, i64)>> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let f = c.filters.spans(&mut sql, "sp");
    let text = format!(
        "SELECT (SELECT p.tariff_id FROM analytics_spans p WHERE p.client_id = sp.client_id \
             AND p.starts_at < sp.starts_at ORDER BY p.starts_at DESC LIMIT 1), sp.tariff_id, count(*)::bigint \
         FROM analytics_spans sp WHERE sp.kind = 'change' AND sp.paid_at >= {s} AND sp.paid_at < {e} AND {f} \
         GROUP BY 1, 2 ORDER BY 3 DESC"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

struct Addons {
    devices: (f64, f64),
    traffic: (f64, f64),
}

async fn addons(c: &Ctx<'_>) -> StoreResult<Addons> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let f = c.filters.facts(&mut sql, "f");
    let text = format!(
        "SELECT 0, \
           COALESCE(sum(CASE WHEN f.item = 'options:devices' THEN f.amount_minor ELSE f.addon_minor END) \
             FILTER (WHERE f.item = 'options:devices' OR (f.kind IN ('new', 'renewal', 'change') AND f.addon_minor > 0)), 0)::float8, \
           count(*) FILTER (WHERE f.item = 'options:devices' OR (f.kind IN ('new', 'renewal', 'change') AND f.addon_minor > 0))::float8, \
           COALESCE(sum(f.amount_minor) FILTER (WHERE f.item = 'options:traffic'), 0)::float8, \
           count(*) FILTER (WHERE f.item = 'options:traffic')::float8 \
         FROM analytics_payments f WHERE f.paid_at >= {s} AND f.paid_at < {e} AND NOT f.stars AND {f}"
    );
    let mut conn = c.store.conn().await?;
    let r = grid(&mut conn, &sql, &text, 4).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 4]);
    Ok(Addons {
        devices: (v[0], v[1]),
        traffic: (v[2], v[3]),
    })
}

type PromoRow = (String, String, i64, i64, i64, i64);

async fn promos(c: &Ctx<'_>) -> StoreResult<Vec<PromoRow>> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let fc = c.filters.client(&mut sql, "r.client_id");
    let text = format!(
        "SELECT pc.code, pc.kind, count(*)::bigint, COALESCE(sum(f.promo_minor), 0)::bigint, \
           COALESCE(sum(f.amount_minor), 0)::bigint, COALESCE(sum(pc.days) FILTER (WHERE pc.kind = 'days'), 0)::bigint \
         FROM promo_redemptions r JOIN promo_codes pc ON pc.id = r.promo_id \
         LEFT JOIN analytics_payments f ON f.order_id = r.order_id AND NOT f.stars \
         WHERE r.status = 'used' AND r.used_at >= {s} AND r.used_at < {e} AND {fc} \
         GROUP BY 1, 2 ORDER BY 3 DESC"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

struct Gifts {
    bought: f64,
    bought_sum: f64,
    redeemed: f64,
    canceled: f64,
    expired: f64,
}

async fn gifts(c: &Ctx<'_>) -> StoreResult<Gifts> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let now = sql.time(c.range.now);
    let text = format!(
        "SELECT 0, \
           (SELECT count(*) FROM gifts g WHERE g.created_at >= {s} AND g.created_at < {e})::float8, \
           (SELECT COALESCE(sum(f.amount_minor), 0) FROM analytics_payments f \
              WHERE f.item = 'gifts:gift' AND NOT f.stars AND f.paid_at >= {s} AND f.paid_at < {e})::float8, \
           (SELECT count(*) FROM gifts g WHERE g.redeemed_at >= {s} AND g.redeemed_at < {e})::float8, \
           (SELECT count(*) FROM gifts g WHERE g.canceled_at >= {s} AND g.canceled_at < {e})::float8, \
           (SELECT count(*) FROM gifts g WHERE g.status = 'active' AND g.expires_at >= {s} \
              AND g.expires_at < {e} AND g.expires_at < {now})::float8"
    );
    let mut conn = c.store.conn().await?;
    let r = grid(&mut conn, &sql, &text, 5).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 5]);
    Ok(Gifts {
        bought: v[0],
        bought_sum: v[1],
        redeemed: v[2],
        canceled: v[3],
        expired: v[4],
    })
}

struct Prizes {
    contest_money: f64,
    roulette_money: f64,
    roulette_days: f64,
    spins: f64,
    contest_issued: f64,
}

async fn prizes(c: &Ctx<'_>) -> StoreResult<Prizes> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let cur = sql.text(c.currency.code());
    let text = format!(
        "SELECT 0, \
           (SELECT COALESCE(sum(b.delta_minor), 0) FROM balance_entries b WHERE b.kind = 'contest' AND b.delta_minor > 0 \
              AND b.currency = {cur} AND b.created_at >= {s} AND b.created_at < {e})::float8, \
           (SELECT COALESCE(sum(b.delta_minor), 0) FROM balance_entries b WHERE b.kind = 'roulette' AND b.delta_minor > 0 \
              AND b.currency = {cur} AND b.created_at >= {s} AND b.created_at < {e})::float8, \
           (SELECT COALESCE(sum(r.days), 0) FROM roulette_spins r WHERE r.created_at >= {s} AND r.created_at < {e})::float8, \
           (SELECT count(*) FROM roulette_spins r WHERE r.created_at >= {s} AND r.created_at < {e})::float8, \
           (SELECT count(*) FROM contest_entries ce WHERE ce.prize_state = 'issued' AND ce.prize_at >= {s} AND ce.prize_at < {e})::float8"
    );
    let mut conn = c.store.conn().await?;
    let r = grid(&mut conn, &sql, &text, 5).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 5]);
    Ok(Prizes {
        contest_money: v[0],
        roulette_money: v[1],
        roulette_days: v[2],
        spins: v[3],
        contest_issued: v[4],
    })
}

fn row(label: String, sub: Option<String>, cells: Vec<Option<f64>>, list: Option<ListRef>) -> Row {
    Row {
        label,
        sub,
        cells,
        list,
        children: Vec::new(),
    }
}

pub async fn build(c: &Ctx<'_>) -> StoreResult<Vec<Group>> {
    let (sold, moved, add, promo, gift, prize) = tokio::try_join!(
        sales(c),
        changes(c),
        addons(c),
        promos(c),
        gifts(c),
        prizes(c)
    )?;
    let (from, to) = (c.range.dates.from, c.range.dates.to);
    let note = c.filters.ignored(&PAYMENT_DIMS);
    let mut tariff_rows: Vec<Row> = Vec::new();
    for (tariff, period, n, sum) in &sold {
        let key = tariff.map_or_else(|| "none".into(), |t| t.to_string());
        let child = row(
            labels::period(period.map(i64::from)),
            None,
            vec![Some(num(*n)), Some(*sum), ratio(*sum, num(*n))],
            Some(
                ListRef::payments("revenue")
                    .slice(format!("tariff:{key}"))
                    .dates(from, to),
            ),
        );
        match tariff_rows
            .iter_mut()
            .find(|r| r.label == c.names.tariff(*tariff))
        {
            Some(r) => {
                let count = r.cells[0].unwrap_or(0.0) + num(*n);
                let total = r.cells[1].unwrap_or(0.0) + sum;
                r.cells = vec![Some(count), Some(total), ratio(total, count)];
                r.children.push(child);
            }
            None => {
                let mut parent = row(
                    c.names.tariff(*tariff),
                    None,
                    vec![Some(num(*n)), Some(*sum), ratio(*sum, num(*n))],
                    Some(
                        ListRef::payments("revenue")
                            .slice(format!("tariff:{key}"))
                            .dates(from, to),
                    ),
                );
                parent.children.push(child);
                tariff_rows.push(parent);
            }
        }
    }
    tariff_rows.sort_by(|a, b| {
        b.cells[1]
            .unwrap_or(0.0)
            .total_cmp(&a.cells[1].unwrap_or(0.0))
    });
    let total_sales: f64 = sold.iter().map(|r| num(r.2)).sum();
    let sales_metric = Metric::new(
        "sales",
        "Продажи по тарифам",
        Unit::Count,
        Some(total_sales),
    )
    .note("новые, продления и смена тарифа")
    .note_opt(note.clone())
    .detail(Detail::Table {
        columns: vec![
            Column::new("Продаж", Unit::Count),
            Column::new("Сумма", Unit::Money),
            Column::new("Средняя цена", Unit::Money),
        ],
        rows: tariff_rows,
        note: Some("Средняя цена — после скидок. Нажмите на тариф, чтобы увидеть сроки.".into()),
    });
    let total_moves: f64 = moved.iter().map(|r| num(r.2)).sum();
    let changes_metric = Metric::new(
        "changes",
        "Переходы между тарифами",
        Unit::Count,
        Some(total_moves),
    )
    .detail(Detail::Table {
        columns: vec![Column::new("Переходов", Unit::Count)],
        rows: moved
            .iter()
            .map(|(a, b, n)| {
                row(
                    format!("{} → {}", c.names.tariff(*a), c.names.tariff(*b)),
                    None,
                    vec![Some(num(*n))],
                    None,
                )
            })
            .collect(),
        note: None,
    });
    let devices = Metric::new(
        "addon_devices",
        "Доп. устройства",
        Unit::Money,
        Some(add.devices.0),
    )
    .note(format!("{} покупок", add.devices.1 as i64))
    .hint("Отдельные покупки устройств и устройства, добавленные при покупке подписки")
    .detail(Detail::None);
    let traffic = Metric::new(
        "addon_traffic",
        "Пакеты трафика",
        Unit::Money,
        Some(add.traffic.0),
    )
    .note(format!("{} покупок", add.traffic.1 as i64))
    .detail(Detail::None);
    let uses: f64 = promo.iter().map(|p| num(p.2)).sum();
    let cost: f64 = promo.iter().map(|p| num(p.3)).sum();
    let promo_metric = Metric::new("promocodes", "Промокоды", Unit::Count, Some(uses))
        .note(format!(
            "скидок на {}",
            labels::money_short(cost, c.currency)
        ))
        .detail(Detail::Table {
            columns: vec![
                Column::new("Применений", Unit::Count),
                Column::new("Скидка", Unit::Money),
                Column::new("Выручка с ними", Unit::Money),
                Column::new("Дней подарено", Unit::Days),
            ],
            rows: promo
                .iter()
                .map(|(code, kind, n, disc, rev, days)| {
                    row(
                        code.clone(),
                        Some(match kind.as_str() {
                            "percent" => "скидка в процентах".into(),
                            "fixed" => "скидка суммой".into(),
                            _ => "дни подписки".into(),
                        }),
                        vec![
                            Some(num(*n)),
                            Some(num(*disc)),
                            Some(num(*rev)),
                            Some(num(*days)),
                        ],
                        None,
                    )
                })
                .collect(),
            note: None,
        });
    let gifts_metric = Metric::new("gifts_sold", "Подарки", Unit::Count, Some(gift.bought))
        .note(format!(
            "на {}",
            labels::money_short(gift.bought_sum, c.currency)
        ))
        .detail(Detail::Table {
            columns: vec![Column::new("Подарков", Unit::Count)],
            rows: vec![
                row("Куплено".into(), None, vec![Some(gift.bought)], None),
                row("Активировано".into(), None, vec![Some(gift.redeemed)], None),
                row("Отменено".into(), None, vec![Some(gift.canceled)], None),
                row(
                    "Истекли неактивированными".into(),
                    None,
                    vec![Some(gift.expired)],
                    None,
                ),
            ],
            note: None,
        });
    let prize_money = prize.contest_money + prize.roulette_money;
    let prizes_metric = Metric::new(
        "prizes",
        "Призы конкурсов и рулетки",
        Unit::Money,
        Some(prize_money),
    )
    .note(format!(
        "и {} подписки",
        labels::count(prize.roulette_days as i64, "день", "дня", "дней")
    ))
    .detail(Detail::Table {
        columns: vec![
            Column::new("Деньгами", Unit::Money),
            Column::new("Призов или дней", Unit::Count),
        ],
        rows: vec![
            row(
                "Конкурсы".into(),
                Some("деньги на баланс · выдано призов".into()),
                vec![Some(prize.contest_money), Some(prize.contest_issued)],
                None,
            ),
            row(
                "Рулетка".into(),
                Some("деньги на баланс · дней подписки".into()),
                vec![Some(prize.roulette_money), Some(prize.roulette_days)],
                None,
            ),
            row(
                "Вращений рулетки".into(),
                None,
                vec![None, Some(prize.spins)],
                None,
            ),
        ],
        note: Some("Дни подписки в деньги не пересчитаны".into()),
    });
    Ok(vec![
        Group::new(None, vec![sales_metric, changes_metric]),
        Group::new(Some("Допы"), vec![devices, traffic]),
        Group::new(
            Some("Промо, подарки, призы"),
            vec![promo_metric, gifts_metric, prizes_metric],
        ),
    ])
}
