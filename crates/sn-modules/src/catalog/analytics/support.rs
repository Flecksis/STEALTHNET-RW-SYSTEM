use super::clients::median;
use super::labels;
use super::metric::{BRow, Bar, Breakdown, Column, Detail, Group, Metric, Row, Unit, num, share};
use super::query::{CLIENT_DIMS, Sql, rows};
use super::tab::{Ctx, both, prev_points, prev_total};
use chrono::{DateTime, Utc};
use sn_store::StoreResult;

type TicketRow = (
    i64,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    bool,
    bool,
);

async fn tickets(c: &Ctx<'_>) -> StoreResult<Vec<TicketRow>> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let f = c.filters.client(&mut sql, "t.client_id");
    let text = format!(
        "SELECT t.id, t.channel, t.created_at, t.closed_at, t.ai_reason, fc.at, \
           (SELECT min(m.created_at) FROM ticket_messages m WHERE m.ticket_id = t.id \
              AND m.author IN ('support', 'ai') AND m.created_at >= fc.at), \
           EXISTS (SELECT 1 FROM ticket_messages m WHERE m.ticket_id = t.id AND m.author = 'support'), \
           EXISTS (SELECT 1 FROM ticket_messages m WHERE m.ticket_id = t.id AND m.author = 'ai') \
         FROM tickets t \
         LEFT JOIN LATERAL (SELECT min(m.created_at) AS at FROM ticket_messages m \
           WHERE m.ticket_id = t.id AND m.author = 'client') fc ON true \
         WHERE t.created_at >= {s} AND t.created_at < {e} AND {f}"
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
    let cond = |sql: &mut Sql| c.filters.client(sql, "t.client_id");
    let (list, (series, prev), (tokens, paying)) = tokio::try_join!(
        tickets(c),
        both(c, "tickets t", "t.created_at", cond, &["count(*)"]),
        extra(c),
    )?;
    let note = c.filters.ignored(&CLIENT_DIMS);
    let mut by_channel: Vec<(String, f64)> = Vec::new();
    let mut replies: Vec<i64> = Vec::new();
    let mut closes: Vec<i64> = Vec::new();
    let (mut answered, mut by_ai) = (0.0, 0.0);
    let mut reasons: Vec<(String, f64)> = Vec::new();
    for (_, ch, created, closed, ai_reason, first_client, first_reply, human, ai) in &list {
        match by_channel.iter_mut().find(|x| x.0 == *ch) {
            Some(x) => x.1 += 1.0,
            None => by_channel.push((ch.clone(), 1.0)),
        }
        if let (Some(a), Some(b)) = (first_client, first_reply) {
            replies.push((*b - *a).num_seconds().max(0));
        }
        if let Some(cl) = closed {
            closes.push((*cl - *created).num_seconds().max(0));
        }
        if *human || *ai {
            answered += 1.0;
            if *ai && !*human {
                by_ai += 1.0;
            }
        }
        if let Some(r) = ai_reason {
            let label = reason(r).to_string();
            match reasons.iter_mut().find(|x| x.0 == label) {
                Some(x) => x.1 += 1.0,
                None => reasons.push((label, 1.0)),
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
    let reply_bars = |values: &[i64]| {
        let m = 60;
        [
            ("До 5 минут", 0, 5 * m),
            ("5–30 минут", 5 * m, 30 * m),
            ("30 минут – 2 часа", 30 * m, 120 * m),
            ("2–12 часов", 120 * m, 720 * m),
            ("Дольше 12 часов", 720 * m, i64::MAX),
        ]
        .iter()
        .map(|(label, lo, hi)| Bar {
            label: (*label).into(),
            value: num(values.iter().filter(|v| **v >= *lo && **v < *hi).count() as i64),
            list: None,
        })
        .collect::<Vec<_>>()
    };
    let first_reply = Metric::new("first_reply", "Первый ответ", Unit::Seconds, median(&replies))
        .note(match p90(&replies) {
            Some(v) => format!("90 % — до {}", labels::duration(v as i64)),
            None => "медиана".into(),
        })
        .detail(Detail::Bars {
            bars: reply_bars(&replies),
            note: Some("От первого сообщения клиента до первого ответа поддержки или ИИ, без учёта рабочих часов".into()),
        });
    let close = Metric::new("close_time", "До закрытия", Unit::Seconds, median(&closes))
        .note("медиана")
        .detail(Detail::Bars {
            bars: reply_bars(&closes),
            note: None,
        });
    let ai_share = Metric::new(
        "ai_share",
        "Закрыл ИИ-чат",
        Unit::Percent,
        share(by_ai, answered),
    )
    .note(format!("{} из {} с ответом", by_ai as i64, answered as i64))
    .hint("Обращения, где ответил только ИИ, без человека")
    .detail(Detail::Table {
        columns: vec![Column {
            title: "Обращений".into(),
            unit: Unit::Count,
        }],
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
