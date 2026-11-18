use super::labels;
use super::metric::{BRow, Bar, Breakdown, Column, Detail, Group, Metric, Row, Unit, num, share};
use super::query::{CLIENT_DIMS, Sql, rows, single};
use super::tab::{Ctx, both, prev_points, prev_total};
use sn_store::StoreResult;

const MINUTE: i64 = 60;

const EDGES: [(&str, i64, i64); 5] = [
    ("До 5 минут", 0, 5 * MINUTE),
    ("5–30 минут", 5 * MINUTE, 30 * MINUTE),
    ("30 минут – 2 часа", 30 * MINUTE, 120 * MINUTE),
    ("2–12 часов", 120 * MINUTE, 720 * MINUTE),
    ("Дольше 12 часов", 720 * MINUTE, i64::MAX),
];

fn bars_sql(column: &str) -> String {
    EDGES
        .iter()
        .map(|(_, lo, hi)| {
            if *hi == i64::MAX {
                format!("count(*) FILTER (WHERE x.{column} >= {lo})::float8")
            } else {
                format!("count(*) FILTER (WHERE x.{column} >= {lo} AND x.{column} < {hi})::float8")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

struct Stats {
    reply: Option<f64>,
    reply_p90: Option<f64>,
    reply_bars: Vec<f64>,
    close: Option<f64>,
    close_bars: Vec<f64>,
    answered: f64,
    by_ai: f64,
}

async fn stats(c: &Ctx<'_>) -> StoreResult<Stats> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let f = c.filters.client(&mut sql, "t.client_id");
    let reply_bars = bars_sql("reply");
    let close_bars = bars_sql("close");
    let text = format!(
        "WITH x AS MATERIALIZED (SELECT \
           CASE WHEN t.closed_at IS NOT NULL \
             THEN greatest(0, floor(extract(epoch FROM t.closed_at - t.created_at)))::bigint END AS close, \
           (SELECT greatest(0, floor(extract(epoch FROM min(m.created_at) - fc.at)))::bigint FROM ticket_messages m \
              WHERE m.ticket_id = t.id AND m.author IN ('support', 'ai') AND m.created_at >= fc.at) AS reply, \
           EXISTS (SELECT 1 FROM ticket_messages m WHERE m.ticket_id = t.id AND m.author = 'support') AS human, \
           EXISTS (SELECT 1 FROM ticket_messages m WHERE m.ticket_id = t.id AND m.author = 'ai') AS ai \
         FROM tickets t \
         LEFT JOIN LATERAL (SELECT min(m.created_at) AS at FROM ticket_messages m \
           WHERE m.ticket_id = t.id AND m.author = 'client') fc ON true \
         WHERE t.created_at >= {s} AND t.created_at < {e} AND {f}) \
         SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY x.reply), \
           percentile_disc(0.9) WITHIN GROUP (ORDER BY x.reply)::float8, {reply_bars}, \
           percentile_cont(0.5) WITHIN GROUP (ORDER BY x.close), {close_bars}, \
           count(*) FILTER (WHERE x.human OR x.ai)::float8, \
           count(*) FILTER (WHERE x.ai AND NOT x.human)::float8 \
         FROM x"
    );
    let mut conn = c.store.conn().await?;
    let v = single(&mut conn, &sql, &text).await?;
    let at = |i: usize| v.get(i).copied().flatten();
    let zero = |i: usize| at(i).unwrap_or(0.0);
    Ok(Stats {
        reply: at(0),
        reply_p90: at(1),
        reply_bars: (2..7).map(zero).collect(),
        close: at(7),
        close_bars: (8..13).map(zero).collect(),
        answered: zero(13),
        by_ai: zero(14),
    })
}

async fn groups(c: &Ctx<'_>) -> StoreResult<Vec<(i32, Option<String>, Option<String>, f64)>> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let f = c.filters.client(&mut sql, "t.client_id");
    let text = format!(
        "SELECT GROUPING(t.channel, t.ai_reason)::int, t.channel, t.ai_reason, count(*)::float8 \
         FROM tickets t WHERE t.created_at >= {s} AND t.created_at < {e} AND {f} \
         GROUP BY GROUPING SETS ((t.channel), (t.ai_reason))"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

async fn extra(c: &Ctx<'_>) -> StoreResult<(f64, f64)> {
    let mut conn = c.store.conn().await?;
    let tokens: Option<i64> = sqlx::query_scalar(
        "SELECT sum(prompt_tokens + completion_tokens)::bigint FROM ai_support_log WHERE created_at >= $1 AND created_at < $2",
    )
    .bind(c.range.start())
    .bind(c.range.end())
    .fetch_one(sn_store::Db::conn(&mut conn))
    .await?;
    let paying: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT client_id) FROM analytics_spans WHERE starts_at < $2 AND ends_at > $1",
    )
    .bind(c.range.start())
    .bind(c.range.end())
    .fetch_one(sn_store::Db::conn(&mut conn))
    .await?;
    Ok((num(tokens.unwrap_or(0)), num(paying)))
}

fn channel(code: &str) -> &'static str {
    match code {
        "bot" => "Бот",
        "cabinet" => "Кабинет",
        "miniapp" => "Мини-приложение",
        _ => "Не указан",
    }
}

fn reason(code: &str) -> &'static str {
    match code {
        "human" => "Клиент попросил человека",
        "money" => "Вопрос про деньги",
        "unsure" => "ИИ не уверен",
        "no_answer" | "groq" => "ИИ не ответил",
        "client_limit" => "Лимит сообщений клиента",
        "budget" => "Закончился бюджет ИИ",
        "operator" => "Оператор взял себе",
        "disabled" => "ИИ-чат выключен",
        _ => "Другое",
    }
}

pub async fn build(c: &Ctx<'_>) -> StoreResult<Vec<Group>> {
    let cond = |sql: &mut Sql| c.filters.client(sql, "t.client_id");
    let (st, found, (series, prev), (tokens, paying)) = tokio::try_join!(
        stats(c),
        groups(c),
        both(c, "tickets t", "t.created_at", cond, &["count(*)"]),
        extra(c),
    )?;
    let note = c.filters.ignored(&CLIENT_DIMS);
    let mut by_channel: Vec<(String, f64)> = Vec::new();
    let mut reasons: Vec<(String, f64)> = Vec::new();
    for (g, ch, ai_reason, n) in &found {
        if *g == 1 {
            by_channel.push((ch.clone().unwrap_or_default(), *n));
        } else if let Some(r) = ai_reason {
            let label = reason(r).to_string();
            match reasons.iter_mut().find(|x| x.0 == label) {
                Some(x) => x.1 += n,
                None => reasons.push((label, *n)),
            }
        }
    }
    by_channel.sort_by(|a, b| b.1.total_cmp(&a.1));
    reasons.sort_by(|a, b| b.1.total_cmp(&a.1));
    let count = series.total(0);
    let tickets_metric = Metric::new("tickets", "Обращения", Unit::Count, Some(count))
        .prev(prev_total(&prev, 0))
        .note_opt(note.clone())
        .detail(Detail::Series {
            points: series.points(0),
            prev: prev_points(&prev, 0),
            breakdowns: vec![Breakdown {
                key: "channel",
                title: "Откуда",
                rows: by_channel
                    .iter()
                    .map(|(k, v)| BRow {
                        key: k.clone(),
                        label: channel(k).into(),
                        value: *v,
                        count: None,
                        prev: None,
                        list: None,
                    })
                    .collect(),
            }],
            list: None,
        });
    let to_bars = |values: &[f64]| {
        EDGES
            .iter()
            .zip(values)
            .map(|((label, _, _), value)| Bar {
                label: (*label).into(),
                value: *value,
                list: None,
            })
            .collect::<Vec<_>>()
    };
    let first_reply = Metric::new("first_reply", "Первый ответ", Unit::Seconds, st.reply)
        .note(match st.reply_p90 {
            Some(v) => format!("90 % — до {}", labels::duration(v as i64)),
            None => "медиана".into(),
        })
        .detail(Detail::Bars {
            bars: to_bars(&st.reply_bars),
            note: Some("От первого сообщения клиента до первого ответа поддержки или ИИ, без учёта рабочих часов".into()),
        });
    let close = Metric::new("close_time", "До закрытия", Unit::Seconds, st.close)
        .note("медиана")
        .detail(Detail::Bars {
            bars: to_bars(&st.close_bars),
            note: None,
        });
    let ai_share = Metric::new(
        "ai_share",
        "Закрыл ИИ-чат",
        Unit::Percent,
        share(st.by_ai, st.answered),
    )
    .note(format!(
        "{} из {} с ответом",
        st.by_ai as i64, st.answered as i64
    ))
    .hint("Обращения, где ответил только ИИ, без человека")
    .detail(Detail::Table {
        columns: vec![Column::new("Обращений", Unit::Count)],
        rows: reasons
            .iter()
            .map(|(l, n)| Row {
                label: l.clone(),
                sub: Some("передано человеку".into()),
                cells: vec![Some(*n)],
                list: None,
                children: Vec::new(),
            })
            .collect(),
        note: Some("Почему ИИ передал обращение человеку".into()),
    });
    let per_1000 = Metric::new(
        "per_1000",
        "На 1 000 платящих",
        Unit::Count,
        (paying > 0.0).then(|| (count / paying * 1000.0).round()),
    )
    .note(format!("{} платящих", paying as i64))
    .detail(Detail::None);
    let tokens_metric =
        Metric::new("ai_tokens", "Токены ИИ", Unit::Count, Some(tokens)).detail(Detail::None);
    Ok(vec![
        Group::new(None, vec![tickets_metric, per_1000]),
        Group::new(Some("Скорость"), vec![first_reply, close]),
        Group::new(Some("ИИ-чат"), vec![ai_share, tokens_metric]),
    ])
}
