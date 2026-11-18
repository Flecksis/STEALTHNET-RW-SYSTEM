use super::auto::{RuleContent, Trigger};
use super::store::{Problem, Progress};
use super::{ButtonSpec, Channel, Lane};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use sn_domain::ids::{AdminId, ClientId};
use sn_store::{Db, StoreError, StoreResult};
use std::collections::HashMap;

const PROBLEMS: i64 = 6;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RuleRow {
    pub id: i64,
    pub trigger: String,
    pub days: i32,
    pub with_autorenew: bool,
    pub channel: String,
    pub subject: Option<String>,
    pub text: String,
    pub image_id: Option<i64>,
    pub buttons: serde_json::Value,
    pub enabled: bool,
    pub version: i32,
    pub notice: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, trigger, days, with_autorenew, channel, subject, text, image_id, buttons, enabled, \
                       version, notice, checked_at, created_at, updated_at";

pub struct Parsed {
    pub trigger: Trigger,
    pub days: u16,
    pub channel: Channel,
    pub buttons: Vec<ButtonSpec>,
}

pub fn parse(row: &RuleRow) -> StoreResult<Parsed> {
    let trigger = Trigger::parse(&row.trigger)
        .ok_or_else(|| StoreError::Decode(format!("событие правила: {}", row.trigger)))?;
    let channel = Channel::parse(&row.channel)
        .ok_or_else(|| StoreError::Decode(format!("канал правила: {}", row.channel)))?;
    let buttons = serde_json::from_value(row.buttons.clone())
        .map_err(|e| StoreError::Decode(format!("кнопки правила: {e}")))?;
    Ok(Parsed {
        trigger,
        days: u16::try_from(row.days).unwrap_or_default(),
        channel,
        buttons,
    })
}

fn json<T: serde::Serialize>(value: &T) -> StoreResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| StoreError::Invalid(e.to_string()))
}

pub async fn insert(
    db: &mut impl Db,
    content: &RuleContent,
    enabled: bool,
    admin: Option<AdminId>,
) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO auto_broadcasts (trigger, days, with_autorenew, channel, subject, text, image_id, buttons, \
         enabled, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(content.trigger.as_str())
    .bind(i32::from(content.days))
    .bind(content.with_autorenew)
    .bind(content.channel.as_str())
    .bind(content.subject.as_deref())
    .bind(&content.text)
    .bind(content.image_id)
    .bind(json(&content.buttons)?)
    .bind(enabled)
    .bind(admin.map(|a| a.0))
    .fetch_one(db.conn())
    .await?)
}

pub async fn update(
    db: &mut impl Db,
    id: i64,
    content: &RuleContent,
    enabled: bool,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE auto_broadcasts SET \
         version = version + CASE WHEN (channel, subject, text, image_id, buttons) \
             IS DISTINCT FROM ($5, $6, $7, $8, $9) THEN 1 ELSE 0 END, \
         trigger = $2, days = $3, with_autorenew = $4, channel = $5, subject = $6, text = $7, image_id = $8, \
         buttons = $9, enabled = $10, notice = CASE WHEN $10 THEN notice END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(content.trigger.as_str())
    .bind(i32::from(content.days))
    .bind(content.with_autorenew)
    .bind(content.channel.as_str())
    .bind(content.subject.as_deref())
    .bind(&content.text)
    .bind(content.image_id)
    .bind(json(&content.buttons)?)
    .bind(enabled)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn set_enabled(db: &mut impl Db, id: i64, enabled: bool) -> StoreResult<()> {
    sqlx::query(
        "UPDATE auto_broadcasts SET enabled = $2, notice = CASE WHEN $2 THEN notice END WHERE id = $1",
    )
    .bind(id)
    .bind(enabled)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn remove(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query(
        "UPDATE auto_broadcasts SET deleted_at = now(), enabled = false, notice = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn stop_queue(db: &mut impl Db, id: i64) -> StoreResult<u64> {
    Ok(sqlx::query(
        "UPDATE broadcast_recipients r SET state = 'skipped', done_at = now(), \
         error = 'Правило выключено до отправки' \
         FROM broadcasts b WHERE b.id = r.broadcast_id AND b.rule_id = $1 AND b.status = 'sending' \
         AND r.state = 'pending'",
    )
    .bind(id)
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn lock(db: &mut impl Db, id: i64) -> StoreResult<RuleRow> {
    sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {COLUMNS} FROM auto_broadcasts WHERE id = $1 AND deleted_at IS NULL FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("правило"))
}

pub async fn row(db: &mut impl Db, id: i64) -> StoreResult<RuleRow> {
    sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {COLUMNS} FROM auto_broadcasts WHERE id = $1 AND deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("правило"))
}

pub async fn rows(db: &mut impl Db) -> StoreResult<Vec<RuleRow>> {
    Ok(sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {COLUMNS} FROM auto_broadcasts WHERE deleted_at IS NULL ORDER BY id"
    ))
    .fetch_all(db.conn())
    .await?)
}

pub async fn enabled_ids(db: &mut impl Db) -> StoreResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM auto_broadcasts WHERE enabled AND deleted_at IS NULL ORDER BY id",
    )
    .fetch_all(db.conn())
    .await?)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub cases: i64,
    pub unreachable: i64,
    pub progress: Progress,
    pub last_sent_at: Option<DateTime<Utc>>,
}

type CaseRow = (i64, i64, i64);
type LaneRow = (i64, i64, i64, i64, i64, i64, Option<DateTime<Utc>>);

pub async fn tallies(db: &mut impl Db, ids: &[i64]) -> StoreResult<HashMap<i64, Tally>> {
    let mut out: HashMap<i64, Tally> = HashMap::new();
    let cases: Vec<CaseRow> = sqlx::query_as(
        "SELECT rule_id, count(*), count(*) FILTER (WHERE NOT queued) FROM auto_broadcast_cases \
         WHERE rule_id = ANY($1) GROUP BY rule_id",
    )
    .bind(ids)
    .fetch_all(db.conn())
    .await?;
    for (id, total, unreachable) in cases {
        let entry = out.entry(id).or_default();
        entry.cases = total;
        entry.unreachable = unreachable;
    }
    let lanes: Vec<LaneRow> = sqlx::query_as(
        "SELECT b.rule_id, count(*) FILTER (WHERE r.state = 'sent'), count(*) FILTER (WHERE r.state = 'failed'), \
         count(*) FILTER (WHERE r.state = 'blocked'), count(*) FILTER (WHERE r.state = 'skipped'), \
         count(*) FILTER (WHERE r.state IN ('pending', 'sending')), max(r.done_at) FILTER (WHERE r.state = 'sent') \
         FROM broadcasts b JOIN broadcast_recipients r ON r.broadcast_id = b.id \
         WHERE b.rule_id = ANY($1) GROUP BY b.rule_id",
    )
    .bind(ids)
    .fetch_all(db.conn())
    .await?;
    for (id, sent, failed, blocked, skipped, pending, last) in lanes {
        let entry = out.entry(id).or_default();
        entry.progress = Progress {
            total: sent + failed + blocked + skipped + pending,
            sent,
            failed,
            blocked,
            skipped,
            pending,
        };
        entry.last_sent_at = last;
    }
    Ok(out)
}

pub async fn problems(db: &mut impl Db, id: i64) -> StoreResult<Vec<Problem>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT channel, reason, count(*) FROM ( \
             SELECT r.channel, CASE WHEN r.channel = 'email' \
                 THEN regexp_replace(r.error, '<?[^[:space:]<>@]+@[^[:space:]<>]+>?', '<адрес>', 'g') \
                 ELSE r.error END AS reason \
             FROM broadcast_recipients r JOIN broadcasts b ON b.id = r.broadcast_id \
             WHERE b.rule_id = $1 AND r.state = 'failed' AND r.error IS NOT NULL) f \
         GROUP BY channel, reason ORDER BY count(*) DESC, channel, reason LIMIT $2",
    )
    .bind(id)
    .bind(PROBLEMS)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(channel, error, count)| {
            Lane::parse(&channel).map(|channel| Problem {
                channel,
                error,
                count,
            })
        })
        .collect())
}

pub async fn sending_notices(db: &mut impl Db, id: i64) -> StoreResult<Vec<String>> {
    let rows: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT notice, mail_notice FROM broadcasts WHERE rule_id = $1 AND status = 'sending' ORDER BY id",
    )
    .bind(id)
    .fetch_all(db.conn())
    .await?;
    let mut out: Vec<String> = Vec::new();
    for notice in rows
        .into_iter()
        .flat_map(|(bot, mail)| [bot, mail])
        .flatten()
    {
        if !out.contains(&notice) {
            out.push(notice);
        }
    }
    Ok(out)
}

pub async fn set_checked(
    db: &mut impl Db,
    id: i64,
    at: DateTime<Utc>,
    notice: Option<&str>,
) -> StoreResult<()> {
    sqlx::query("UPDATE auto_broadcasts SET checked_at = $2, notice = $3 WHERE id = $1")
        .bind(id)
        .bind(at)
        .bind(notice)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub struct Bounds {
    pub now: DateTime<Utc>,
    pub from: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

impl Bounds {
    pub fn of(trigger: Trigger, days: u16, now: DateTime<Utc>, window: Duration) -> Self {
        let span = Duration::days(i64::from(days));
        let until = match trigger {
            Trigger::Expiring => now + span,
            _ => now - span,
        };
        Bounds {
            now,
            from: until - window,
            until,
        }
    }
}

fn hour_key(column: &str) -> String {
    format!("to_char({column} AT TIME ZONE 'UTC', 'YYYYMMDDHH24')")
}

const BOUGHT: &str = "(EXISTS (SELECT 1 FROM orders o WHERE o.client_id = {c} AND o.paid_at IS NOT NULL \
                      AND o.kind IN ('new', 'renewal', 'change')) \
                      OR EXISTS (SELECT 1 FROM subscriptions p WHERE p.client_id = {c} AND p.kind = 'paid'))";

fn bought(client: &str) -> String {
    BOUGHT.replace("{c}", client)
}

const OTHER_LIVE: &str = "EXISTS (SELECT 1 FROM subscriptions y WHERE y.client_id = s.client_id AND y.id <> s.id \
                          AND y.ended_at IS NULL AND y.remna_missing_at IS NULL AND y.expire_at > $1 \
                          AND (y.status IS NULL OR y.status IN ('active', 'limited')))";

const ENDED_BY_TIME: &str = "(s.ended_at IS NULL OR s.ended_at >= s.expire_at) \
                             AND (s.remna_missing_at IS NULL OR s.remna_missing_at >= s.expire_at) \
                             AND (s.status IS NULL OR s.status <> 'disabled')";

fn cases_sql(trigger: Trigger) -> String {
    match trigger {
        Trigger::Expiring => format!(
            "SELECT s.client_id, 'expiring:' || s.id || ':' || {key} AS case_key, \
             s.expire_at - make_interval(days => $4) AS due_at \
             FROM subscriptions s \
             WHERE s.expire_at > $2 AND s.expire_at <= $3 AND s.expire_at > $1 \
             AND s.ended_at IS NULL AND s.remna_missing_at IS NULL \
             AND (s.status IS NULL OR s.status IN ('active', 'limited')) \
             AND ($5 OR NOT s.autorenew_enabled)",
            key = hour_key("s.expire_at")
        ),
        Trigger::Expired => format!(
            "SELECT s.client_id, 'expired:' || s.id || ':' || {key} AS case_key, \
             s.expire_at + make_interval(days => $4) AS due_at \
             FROM subscriptions s \
             WHERE s.expire_at > $2 AND s.expire_at <= $3 AND s.kind <> 'trial' AND {ENDED_BY_TIME} \
             AND NOT {OTHER_LIVE}",
            key = hour_key("s.expire_at")
        ),
        Trigger::TrialEnded => format!(
            "SELECT s.client_id, 'trial:' || s.id || ':' || {key} AS case_key, \
             s.expire_at + make_interval(days => $4) AS due_at \
             FROM subscriptions s \
             WHERE s.expire_at > $2 AND s.expire_at <= $3 AND s.kind = 'trial' AND {ENDED_BY_TIME} \
             AND NOT {OTHER_LIVE} AND NOT {bought}",
            key = hour_key("s.expire_at"),
            bought = bought("s.client_id")
        ),
        Trigger::NoPurchase => format!(
            "SELECT c.id AS client_id, 'registered:' || c.id AS case_key, \
             c.created_at + make_interval(days => $4) AS due_at \
             FROM clients c \
             WHERE c.created_at > $2 AND c.created_at <= $3 AND c.deleted_at IS NULL \
             AND c.registration_source NOT IN ('import', 'migration') AND NOT {bought}",
            bought = bought("c.id")
        ),
        Trigger::NotConnected => "SELECT s.client_id, 'connect:' || s.id AS case_key, \
             p.paid_at + make_interval(days => $4) AS due_at \
             FROM subscriptions s \
             JOIN LATERAL (SELECT min(o.paid_at) AS paid_at FROM orders o WHERE o.subscription_id = s.id \
                 AND o.paid_at IS NOT NULL AND o.kind IN ('new', 'renewal', 'change')) p ON true \
             WHERE p.paid_at > $2 AND p.paid_at <= $3 AND s.first_connected_at IS NULL \
             AND s.ended_at IS NULL AND s.remna_missing_at IS NULL AND s.expire_at > $1 \
             AND (s.status IS NULL OR s.status IN ('active', 'limited'))"
            .into(),
    }
}

fn found_sql(trigger: Trigger) -> String {
    format!(
        "WITH found AS ({cases}) \
         SELECT f.client_id, f.case_key, f.due_at, tg.value AS telegram, \
             c.bot_unreachable AS stopped, em.value AS email, \
             EXISTS (SELECT 1 FROM broadcast_unsubscribes u WHERE u.email = em.value) AS unsubscribed \
         FROM found f \
         JOIN clients c ON c.id = f.client_id AND c.deleted_at IS NULL AND c.status = 'active' \
         LEFT JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = c.id \
             AND i.kind = 'telegram' AND i.value ~ '^[0-9]{{1,15}}$' ORDER BY i.id LIMIT 1) tg ON true \
         LEFT JOIN LATERAL (SELECT lower(i.value) AS value FROM client_identities i WHERE i.client_id = c.id \
             AND i.kind = 'email' AND i.value LIKE '%_@_%' ORDER BY i.id LIMIT 1) em ON true \
         WHERE NOT EXISTS (SELECT 1 FROM auto_broadcast_cases k WHERE k.rule_id = $6 AND k.case_key = f.case_key)",
        cases = cases_sql(trigger)
    )
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Found {
    pub client_id: i64,
    pub case_key: String,
    pub due_at: DateTime<Utc>,
    pub telegram: Option<String>,
    pub stopped: bool,
    pub email: Option<String>,
    pub unsubscribed: bool,
}

impl Found {
    pub fn chat(&self) -> Option<i64> {
        if self.stopped {
            return None;
        }
        self.telegram
            .as_deref()
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| *v > 0)
    }

    pub fn mailbox(&self) -> Option<&str> {
        if self.unsubscribed {
            return None;
        }
        self.email.as_deref()
    }
}

pub struct Probe {
    pub trigger: Trigger,
    pub days: u16,
    pub with_autorenew: bool,
    pub rule: Option<i64>,
}

pub async fn found(
    db: &mut impl Db,
    probe: &Probe,
    bounds: &Bounds,
    limit: i64,
) -> StoreResult<Vec<Found>> {
    Ok(sqlx::query_as::<_, Found>(&format!(
        "SELECT * FROM ({}) q ORDER BY q.due_at, q.case_key LIMIT $7",
        found_sql(probe.trigger)
    ))
    .bind(bounds.now)
    .bind(bounds.from)
    .bind(bounds.until)
    .bind(i32::from(probe.days))
    .bind(probe.with_autorenew)
    .bind(probe.rule)
    .bind(limit)
    .fetch_all(db.conn())
    .await?)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counted {
    pub cases: i64,
    pub recipients: i64,
    pub stopped: i64,
    pub emails: i64,
    pub unsubscribed: i64,
}

pub async fn count(db: &mut impl Db, probe: &Probe, bounds: &Bounds) -> StoreResult<Counted> {
    let (cases, recipients, stopped, emails, unsubscribed): (i64, i64, i64, i64, i64) =
        sqlx::query_as(&format!(
            "SELECT count(*), \
             count(*) FILTER (WHERE q.telegram IS NOT NULL AND NOT q.stopped), \
             count(*) FILTER (WHERE q.telegram IS NOT NULL AND q.stopped), \
             count(*) FILTER (WHERE q.email IS NOT NULL AND NOT q.unsubscribed), \
             count(*) FILTER (WHERE q.email IS NOT NULL AND q.unsubscribed) \
             FROM ({}) q",
            found_sql(probe.trigger)
        ))
        .bind(bounds.now)
        .bind(bounds.from)
        .bind(bounds.until)
        .bind(i32::from(probe.days))
        .bind(probe.with_autorenew)
        .bind(probe.rule)
        .fetch_one(db.conn())
        .await?;
    Ok(Counted {
        cases,
        recipients,
        stopped,
        emails,
        unsubscribed,
    })
}

pub async fn batch(db: &mut impl Db, rule: i64, day: NaiveDate, version: i32) -> StoreResult<i64> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM broadcasts WHERE rule_id = $1 AND rule_day = $2 AND rule_version = $3 FOR UPDATE",
    )
    .bind(rule)
    .bind(day)
    .bind(version)
    .fetch_optional(db.conn())
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    Ok(sqlx::query_scalar(
        "INSERT INTO broadcasts (text, image_id, buttons, audience, channel, subject, status, rule_id, rule_day, \
         rule_version) \
         SELECT a.text, a.image_id, a.buttons, '{\"kind\": \"all\"}'::jsonb, a.channel, a.subject, 'done', a.id, $2, $3 \
         FROM auto_broadcasts a WHERE a.id = $1 RETURNING id",
    )
    .bind(rule)
    .bind(day)
    .bind(version)
    .fetch_one(db.conn())
    .await?)
}

pub struct NewCase<'a> {
    pub key: &'a str,
    pub client_id: ClientId,
    pub due_at: DateTime<Utc>,
    pub queued: bool,
}

pub async fn record_cases(
    db: &mut impl Db,
    rule: i64,
    batch: Option<i64>,
    cases: &[NewCase<'_>],
) -> StoreResult<Vec<String>> {
    let keys: Vec<&str> = cases.iter().map(|c| c.key).collect();
    let clients: Vec<i64> = cases.iter().map(|c| c.client_id.0).collect();
    let due: Vec<DateTime<Utc>> = cases.iter().map(|c| c.due_at).collect();
    let queued: Vec<bool> = cases.iter().map(|c| c.queued).collect();
    Ok(sqlx::query_scalar(
        "INSERT INTO auto_broadcast_cases (rule_id, case_key, client_id, due_at, broadcast_id, queued) \
         SELECT $1, t.k, t.c, t.d, CASE WHEN t.q THEN $2::bigint END, t.q \
         FROM unnest($3::text[], $4::bigint[], $5::timestamptz[], $6::bool[]) AS t(k, c, d, q) \
         ON CONFLICT DO NOTHING RETURNING case_key",
    )
    .bind(rule)
    .bind(batch)
    .bind(&keys)
    .bind(&clients)
    .bind(&due)
    .bind(&queued)
    .fetch_all(db.conn())
    .await?)
}

pub async fn queue_bot(db: &mut impl Db, batch: i64, rows: &[(ClientId, i64)]) -> StoreResult<u64> {
    if rows.is_empty() {
        return Ok(0);
    }
    let clients: Vec<i64> = rows.iter().map(|(c, _)| c.0).collect();
    let chats: Vec<i64> = rows.iter().map(|(_, chat)| *chat).collect();
    Ok(sqlx::query(
        "INSERT INTO broadcast_recipients (broadcast_id, channel, client_id, chat_id) \
         SELECT $1, 'bot', t.c, t.chat FROM unnest($2::bigint[], $3::bigint[]) AS t(c, chat) \
         ON CONFLICT DO NOTHING",
    )
    .bind(batch)
    .bind(&clients)
    .bind(&chats)
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn queue_mail(
    db: &mut impl Db,
    batch: i64,
    rows: &[(ClientId, String)],
) -> StoreResult<u64> {
    if rows.is_empty() {
        return Ok(0);
    }
    let clients: Vec<i64> = rows.iter().map(|(c, _)| c.0).collect();
    let emails: Vec<&str> = rows.iter().map(|(_, e)| e.as_str()).collect();
    Ok(sqlx::query(
        "INSERT INTO broadcast_recipients (broadcast_id, channel, client_id, email, unsubscribe) \
         SELECT $1, 'email', t.c, t.e, gen_random_uuid() FROM unnest($2::bigint[], $3::text[]) AS t(c, e) \
         ON CONFLICT DO NOTHING",
    )
    .bind(batch)
    .bind(&clients)
    .bind(&emails)
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn open_batch(db: &mut impl Db, batch: i64, now: DateTime<Utc>) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET status = 'sending', started_at = COALESCE(started_at, $2), finished_at = NULL, \
         total = LEAST((SELECT count(*) FROM broadcast_recipients r WHERE r.broadcast_id = $1), 2147483647)::int \
         WHERE id = $1",
    )
    .bind(batch)
    .bind(now)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn contest_batch(
    db: &mut impl Db,
    message_id: i64,
    text: &str,
    buttons: &serde_json::Value,
) -> StoreResult<i64> {
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM broadcasts WHERE contest_message_id = $1 FOR UPDATE")
            .bind(message_id)
            .fetch_optional(db.conn())
            .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    Ok(sqlx::query_scalar(
        "INSERT INTO broadcasts (text, buttons, audience, channel, status, contest_message_id) \
         VALUES ($1, $2, '{\"kind\": \"all\"}'::jsonb, 'bot', 'done', $3) RETURNING id",
    )
    .bind(text)
    .bind(buttons)
    .bind(message_id)
    .fetch_one(db.conn())
    .await?)
}

pub async fn contest_progress(
    db: &mut impl Db,
    message_ids: &[i64],
) -> StoreResult<HashMap<i64, Progress>> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT b.contest_message_id, r.state, count(*) FROM broadcasts b \
         JOIN broadcast_recipients r ON r.broadcast_id = b.id \
         WHERE b.contest_message_id = ANY($1) GROUP BY b.contest_message_id, r.state",
    )
    .bind(message_ids)
    .fetch_all(db.conn())
    .await?;
    let mut out: HashMap<i64, Progress> = HashMap::new();
    for (id, state, count) in rows {
        let progress = out.entry(id).or_default();
        progress.total += count;
        match state.as_str() {
            "sent" => progress.sent += count,
            "failed" => progress.failed += count,
            "blocked" => progress.blocked += count,
            "skipped" => progress.skipped += count,
            _ => progress.pending += count,
        }
    }
    Ok(out)
}

pub async fn stop_contest(db: &mut impl Db, contest_id: i64, reason: &str) -> StoreResult<u64> {
    Ok(sqlx::query(
        "UPDATE broadcast_recipients r SET state = 'skipped', done_at = now(), error = $2 \
         FROM broadcasts b JOIN contest_messages m ON m.id = b.contest_message_id \
         WHERE b.id = r.broadcast_id AND m.contest_id = $1 AND r.state = 'pending'",
    )
    .bind(contest_id)
    .bind(reason)
    .execute(db.conn())
    .await?
    .rows_affected())
}
