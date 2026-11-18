use super::{Audience, Counter, Period, Restriction, Rule, SubFacts, TrafficScope};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sn_domain::ids::{AdminId, ClientId, SubscriptionId};
use sn_domain::paging::{Page, PageRequest};
use sn_store::{ClientPerson, Db, StoreError, StoreResult};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct RuleRow {
    id: i64,
    enabled: bool,
    audience: String,
    traffic: String,
    devices_min: Option<i32>,
    period: String,
    limit_bytes: i64,
    per_device: bool,
    warn: bool,
    alert: bool,
    restriction: String,
    squad_uuid: Option<Uuid>,
    note: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn invalid(what: &str, value: &str) -> StoreError {
    StoreError::Invalid(format!("{what}: неизвестное значение {value}"))
}

impl TryFrom<RuleRow> for Rule {
    type Error = StoreError;

    fn try_from(row: RuleRow) -> Result<Self, Self::Error> {
        Ok(Rule {
            id: row.id,
            enabled: row.enabled,
            audience: Audience::parse(&row.audience)
                .ok_or_else(|| invalid("audience", &row.audience))?,
            traffic: TrafficScope::parse(&row.traffic)
                .ok_or_else(|| invalid("traffic", &row.traffic))?,
            devices_min: row.devices_min.and_then(|d| u32::try_from(d).ok()),
            period: Period::parse(&row.period).ok_or_else(|| invalid("period", &row.period))?,
            limit_bytes: u64::try_from(row.limit_bytes).unwrap_or(0),
            per_device: row.per_device,
            warn: row.warn,
            alert: row.alert,
            restriction: Restriction::parse(&row.restriction)
                .ok_or_else(|| invalid("restriction", &row.restriction))?,
            squad: row.squad_uuid,
            note: row.note,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

const RULE_COLUMNS: &str = "id, enabled, audience, traffic, devices_min, period, limit_bytes, per_device, \
     warn, alert, restriction, squad_uuid, note, created_at, updated_at";

pub async fn rules(db: &mut impl Db, only_enabled: bool) -> StoreResult<Vec<Rule>> {
    let rows: Vec<RuleRow> = sqlx::query_as(&format!(
        "SELECT {RULE_COLUMNS} FROM traffic_limit_rules \
         WHERE deleted_at IS NULL AND (enabled OR NOT $1) ORDER BY period, limit_bytes, id"
    ))
    .bind(only_enabled)
    .fetch_all(db.conn())
    .await?;
    rows.into_iter().map(Rule::try_from).collect()
}

pub async fn rule(db: &mut impl Db, id: i64) -> StoreResult<Option<Rule>> {
    let row: Option<RuleRow> = sqlx::query_as(&format!(
        "SELECT {RULE_COLUMNS} FROM traffic_limit_rules WHERE id = $1 AND deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    row.map(Rule::try_from).transpose()
}

pub async fn save_rule(
    db: &mut impl Db,
    id: Option<i64>,
    rule: &Rule,
    admin: Option<AdminId>,
) -> StoreResult<Option<Rule>> {
    let devices = rule
        .devices_min
        .map(|d| i32::try_from(d).unwrap_or(i32::MAX));
    let limit = i64::try_from(rule.limit_bytes).unwrap_or(i64::MAX);
    let row: Option<RuleRow> = match id {
        None => sqlx::query_as(&format!(
            "INSERT INTO traffic_limit_rules (enabled, audience, traffic, devices_min, period, limit_bytes, \
                 per_device, warn, alert, restriction, squad_uuid, note, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING {RULE_COLUMNS}"
        ))
        .bind(rule.enabled)
        .bind(rule.audience.as_str())
        .bind(rule.traffic.as_str())
        .bind(devices)
        .bind(rule.period.as_str())
        .bind(limit)
        .bind(rule.per_device)
        .bind(rule.warn)
        .bind(rule.alert)
        .bind(rule.restriction.as_str())
        .bind(rule.squad)
        .bind(rule.note.as_deref())
        .bind(admin.map(|a| a.0))
        .fetch_optional(db.conn())
        .await?,
        Some(id) => sqlx::query_as(&format!(
            "UPDATE traffic_limit_rules SET enabled = $2, audience = $3, traffic = $4, devices_min = $5, \
                 period = $6, limit_bytes = $7, per_device = $8, warn = $9, alert = $10, restriction = $11, \
                 squad_uuid = $12, note = $13 \
             WHERE id = $1 AND deleted_at IS NULL RETURNING {RULE_COLUMNS}"
        ))
        .bind(id)
        .bind(rule.enabled)
        .bind(rule.audience.as_str())
        .bind(rule.traffic.as_str())
        .bind(devices)
        .bind(rule.period.as_str())
        .bind(limit)
        .bind(rule.per_device)
        .bind(rule.warn)
        .bind(rule.alert)
        .bind(rule.restriction.as_str())
        .bind(rule.squad)
        .bind(rule.note.as_deref())
        .fetch_optional(db.conn())
        .await?,
    };
    row.map(Rule::try_from).transpose()
}

pub async fn delete_rule(db: &mut impl Db, id: i64) -> StoreResult<bool> {
    Ok(sqlx::query(
        "UPDATE traffic_limit_rules SET deleted_at = now(), enabled = false WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .execute(db.conn())
    .await?
    .rows_affected()
        > 0)
}

#[derive(sqlx::FromRow)]
struct FactsRow {
    id: i64,
    client_id: i64,
    kind: String,
    traffic_limit_bytes: Option<i64>,
    hwid_limit: Option<i32>,
}

#[derive(sqlx::FromRow)]
struct CounterRow {
    subscription_id: i64,
    day: NaiveDate,
    day_base: i64,
    week: NaiveDate,
    week_base: i64,
    last_bytes: i64,
    seen_at: DateTime<Utc>,
}

pub async fn facts(
    db: &mut impl Db,
    ids: &[SubscriptionId],
) -> StoreResult<HashMap<SubscriptionId, SubFacts>> {
    let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
    let rows: Vec<FactsRow> = sqlx::query_as(
        "SELECT s.id, s.client_id, s.kind, s.traffic_limit_bytes, s.hwid_limit FROM subscriptions s \
         WHERE s.id = ANY($1) AND s.ended_at IS NULL",
    )
    .bind(&ids)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                SubscriptionId(row.id),
                SubFacts {
                    subscription_id: SubscriptionId(row.id),
                    client_id: ClientId(row.client_id),
                    trial: row.kind == "trial",
                    unlimited: row.traffic_limit_bytes.is_none_or(|l| l <= 0),
                    devices: row
                        .hwid_limit
                        .and_then(|d| u32::try_from(d).ok())
                        .filter(|d| *d > 0),
                },
            )
        })
        .collect())
}

pub async fn counters(
    db: &mut impl Db,
    ids: &[SubscriptionId],
) -> StoreResult<HashMap<SubscriptionId, Counter>> {
    let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
    let rows: Vec<CounterRow> = sqlx::query_as(
        "SELECT subscription_id, day, day_base, week, week_base, last_bytes, seen_at \
         FROM traffic_counters WHERE subscription_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(db.conn())
    .await?;
    let bytes = |v: i64| u64::try_from(v).unwrap_or(0);
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                SubscriptionId(row.subscription_id),
                Counter {
                    day: row.day,
                    day_base: bytes(row.day_base),
                    week: row.week,
                    week_base: bytes(row.week_base),
                    last_bytes: bytes(row.last_bytes),
                    seen_at: row.seen_at,
                },
            )
        })
        .collect())
}

pub async fn save_counters(
    db: &mut impl Db,
    counters: &[(SubscriptionId, Counter)],
) -> StoreResult<()> {
    if counters.is_empty() {
        return Ok(());
    }
    let big = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
    let ids: Vec<i64> = counters.iter().map(|(id, _)| id.0).collect();
    let days: Vec<NaiveDate> = counters.iter().map(|(_, c)| c.day).collect();
    let day_bases: Vec<i64> = counters.iter().map(|(_, c)| big(c.day_base)).collect();
    let weeks: Vec<NaiveDate> = counters.iter().map(|(_, c)| c.week).collect();
    let week_bases: Vec<i64> = counters.iter().map(|(_, c)| big(c.week_base)).collect();
    let lasts: Vec<i64> = counters.iter().map(|(_, c)| big(c.last_bytes)).collect();
    let seen: Vec<DateTime<Utc>> = counters.iter().map(|(_, c)| c.seen_at).collect();
    sqlx::query(
        "INSERT INTO traffic_counters (subscription_id, day, day_base, week, week_base, last_bytes, seen_at) \
         SELECT * FROM unnest($1::bigint[], $2::date[], $3::bigint[], $4::date[], $5::bigint[], $6::bigint[], $7::timestamptz[]) \
         ON CONFLICT (subscription_id) DO UPDATE SET day = EXCLUDED.day, day_base = EXCLUDED.day_base, \
             week = EXCLUDED.week, week_base = EXCLUDED.week_base, last_bytes = EXCLUDED.last_bytes, \
             seen_at = EXCLUDED.seen_at",
    )
    .bind(&ids)
    .bind(&days)
    .bind(&day_bases)
    .bind(&weeks)
    .bind(&week_bases)
    .bind(&lasts)
    .bind(&seen)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub struct NewExceed {
    pub rule: i64,
    pub subscription_id: SubscriptionId,
    pub client_id: ClientId,
    pub period: Period,
    pub window_start: NaiveDate,
    pub window_end: DateTime<Utc>,
    pub limit_bytes: u64,
    pub used_bytes: u64,
    pub warn: bool,
    pub alert: bool,
    pub restriction: Restriction,
    pub squad: Option<Uuid>,
    pub at: DateTime<Utc>,
}

pub async fn upsert_exceed(db: &mut impl Db, new: &NewExceed) -> StoreResult<(i64, bool)> {
    let big = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
    Ok(sqlx::query_as(
        "INSERT INTO traffic_exceeds (rule_id, subscription_id, client_id, period, window_start, window_end, \
             limit_bytes, used_bytes, warn, alert, restriction, squad_uuid, exceeded_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (rule_id, subscription_id, window_start) \
         DO UPDATE SET used_bytes = GREATEST(traffic_exceeds.used_bytes, EXCLUDED.used_bytes) \
         RETURNING id, (xmax = 0)",
    )
    .bind(new.rule)
    .bind(new.subscription_id.0)
    .bind(new.client_id.0)
    .bind(new.period.as_str())
    .bind(new.window_start)
    .bind(new.window_end)
    .bind(big(new.limit_bytes))
    .bind(big(new.used_bytes))
    .bind(new.warn)
    .bind(new.alert)
    .bind(new.restriction.as_str())
    .bind(new.squad)
    .bind(new.at)
    .fetch_one(db.conn())
    .await?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Exceed {
    pub id: i64,
    pub rule_id: i64,
    pub subscription_id: SubscriptionId,
    pub client_id: ClientId,
    pub period: Period,
    pub window_start: NaiveDate,
    pub window_end: DateTime<Utc>,
    pub limit_bytes: u64,
    pub used_bytes: u64,
    pub warn: bool,
    pub alert: bool,
    pub restriction: Restriction,
    pub squad: Option<Uuid>,
    pub previous_squads: Vec<Uuid>,
    pub restrict_tries: i32,
    pub exceeded_at: DateTime<Utc>,
    pub warned_at: Option<DateTime<Utc>>,
    pub restricted_at: Option<DateTime<Utc>>,
    pub restrict_note: Option<String>,
    pub lifted_at: Option<DateTime<Utc>>,
    pub lift_reason: Option<String>,
    pub lift_due_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct ExceedRow {
    id: i64,
    rule_id: i64,
    subscription_id: i64,
    client_id: i64,
    period: String,
    window_start: NaiveDate,
    window_end: DateTime<Utc>,
    limit_bytes: i64,
    used_bytes: i64,
    warn: bool,
    alert: bool,
    restriction: String,
    squad_uuid: Option<Uuid>,
    previous_squads: Option<serde_json::Value>,
    restrict_tries: i32,
    exceeded_at: DateTime<Utc>,
    warned_at: Option<DateTime<Utc>>,
    restricted_at: Option<DateTime<Utc>>,
    restrict_note: Option<String>,
    lifted_at: Option<DateTime<Utc>>,
    lift_reason: Option<String>,
    lift_due_at: Option<DateTime<Utc>>,
}

impl TryFrom<ExceedRow> for Exceed {
    type Error = StoreError;

    fn try_from(row: ExceedRow) -> Result<Self, Self::Error> {
        Ok(Exceed {
            id: row.id,
            rule_id: row.rule_id,
            subscription_id: SubscriptionId(row.subscription_id),
            client_id: ClientId(row.client_id),
            period: Period::parse(&row.period).ok_or_else(|| invalid("period", &row.period))?,
            window_start: row.window_start,
            window_end: row.window_end,
            limit_bytes: u64::try_from(row.limit_bytes).unwrap_or(0),
            used_bytes: u64::try_from(row.used_bytes).unwrap_or(0),
            warn: row.warn,
            alert: row.alert,
            restriction: Restriction::parse(&row.restriction)
                .ok_or_else(|| invalid("restriction", &row.restriction))?,
            squad: row.squad_uuid,
            previous_squads: row
                .previous_squads
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            restrict_tries: row.restrict_tries,
            exceeded_at: row.exceeded_at,
            warned_at: row.warned_at,
            restricted_at: row.restricted_at,
            restrict_note: row.restrict_note,
            lifted_at: row.lifted_at,
            lift_reason: row.lift_reason,
            lift_due_at: row.lift_due_at,
        })
    }
}

const EXCEED_COLUMNS: &str = "e.id, e.rule_id, e.subscription_id, e.client_id, e.period, e.window_start, \
     e.window_end, e.limit_bytes, e.used_bytes, e.warn, e.alert, e.restriction, e.squad_uuid, e.previous_squads, \
     e.restrict_tries, e.exceeded_at, e.warned_at, e.restricted_at, e.restrict_note, e.lifted_at, e.lift_reason, \
     e.lift_due_at";

pub async fn exceed(db: &mut impl Db, id: i64) -> StoreResult<Option<Exceed>> {
    let row: Option<ExceedRow> = sqlx::query_as(&format!(
        "SELECT {EXCEED_COLUMNS} FROM traffic_exceeds e WHERE e.id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    row.map(Exceed::try_from).transpose()
}

pub async fn pending_restrictions(
    db: &mut impl Db,
    now: DateTime<Utc>,
    max_tries: i32,
) -> StoreResult<Vec<Exceed>> {
    let rows: Vec<ExceedRow> = sqlx::query_as(&format!(
        "SELECT {EXCEED_COLUMNS} FROM traffic_exceeds e \
         WHERE e.restriction <> 'none' AND e.restricted_at IS NULL AND e.lifted_at IS NULL \
           AND e.restrict_tries < $2 AND e.window_end > $1 \
         ORDER BY e.id LIMIT 200"
    ))
    .bind(now)
    .bind(max_tries)
    .fetch_all(db.conn())
    .await?;
    rows.into_iter().map(Exceed::try_from).collect()
}

pub async fn due_lifts(
    db: &mut impl Db,
    now: DateTime<Utc>,
    everything: bool,
) -> StoreResult<Vec<Exceed>> {
    let rows: Vec<ExceedRow> = sqlx::query_as(&format!(
        "SELECT {EXCEED_COLUMNS} FROM traffic_exceeds e \
         WHERE e.restricted_at IS NOT NULL AND e.lifted_at IS NULL AND ($2 OR e.lift_due_at <= $1) \
         ORDER BY e.lift_due_at, e.id LIMIT 200"
    ))
    .bind(now)
    .bind(everything)
    .fetch_all(db.conn())
    .await?;
    rows.into_iter().map(Exceed::try_from).collect()
}

pub async fn active_restrictions(
    db: &mut impl Db,
    subscription_id: SubscriptionId,
    except: i64,
) -> StoreResult<Vec<Exceed>> {
    let rows: Vec<ExceedRow> = sqlx::query_as(&format!(
        "SELECT {EXCEED_COLUMNS} FROM traffic_exceeds e \
         WHERE e.subscription_id = $1 AND e.id <> $2 AND e.restricted_at IS NOT NULL AND e.lifted_at IS NULL \
         ORDER BY e.id"
    ))
    .bind(subscription_id.0)
    .bind(except)
    .fetch_all(db.conn())
    .await?;
    rows.into_iter().map(Exceed::try_from).collect()
}

pub async fn hand_over_previous(db: &mut impl Db, from: &Exceed) -> StoreResult<u64> {
    Ok(sqlx::query(
        "UPDATE traffic_exceeds SET previous_squads = $4 \
         WHERE subscription_id = $1 AND id <> $2 AND restriction = 'squad' AND squad_uuid = $3 \
           AND restricted_at IS NOT NULL AND lifted_at IS NULL \
           AND (previous_squads IS NULL OR previous_squads = '[]'::jsonb)",
    )
    .bind(from.subscription_id.0)
    .bind(from.id)
    .bind(from.squad)
    .bind(serde_json::json!(from.previous_squads))
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn mark_restricted(
    db: &mut impl Db,
    id: i64,
    at: DateTime<Utc>,
    due: DateTime<Utc>,
    previous: &[Uuid],
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE traffic_exceeds SET restricted_at = $2, lift_due_at = $3, previous_squads = $4, restrict_note = NULL \
         WHERE id = $1 AND restricted_at IS NULL",
    )
    .bind(id)
    .bind(at)
    .bind(due)
    .bind(serde_json::json!(previous))
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn note_restriction(
    db: &mut impl Db,
    id: i64,
    note: &str,
    failed: bool,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE traffic_exceeds SET restrict_note = $2, \
             restrict_tries = CASE WHEN $3 THEN restrict_tries + 1 ELSE $4 END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(note.chars().take(300).collect::<String>())
    .bind(failed)
    .bind(super::RESTRICT_TRIES)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn mark_warned(db: &mut impl Db, id: i64, at: DateTime<Utc>) -> StoreResult<()> {
    sqlx::query("UPDATE traffic_exceeds SET warned_at = $2 WHERE id = $1 AND warned_at IS NULL")
        .bind(id)
        .bind(at)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn mark_lifted(
    db: &mut impl Db,
    id: i64,
    at: DateTime<Utc>,
    reason: &str,
    admin: Option<AdminId>,
) -> StoreResult<bool> {
    Ok(sqlx::query(
        "UPDATE traffic_exceeds SET lifted_at = $2, lift_reason = $3, lifted_by = $4 \
         WHERE id = $1 AND lifted_at IS NULL",
    )
    .bind(id)
    .bind(at)
    .bind(reason)
    .bind(admin.map(|a| a.0))
    .execute(db.conn())
    .await?
    .rows_affected()
        > 0)
}

pub async fn subscription_live(db: &mut impl Db, id: SubscriptionId) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM subscriptions WHERE id = $1 AND ended_at IS NULL)",
    )
    .bind(id.0)
    .fetch_one(db.conn())
    .await?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceedFilter {
    Current,
    History,
}

impl ExceedFilter {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.map(str::trim).unwrap_or("current") {
            "" | "current" => Some(ExceedFilter::Current),
            "history" => Some(ExceedFilter::History),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExceedItem {
    #[serde(flatten)]
    pub exceed: Exceed,
    pub client: Option<ClientPerson>,
    pub tariff: Option<String>,
    pub rule: Option<String>,
}

pub async fn list(
    db: &mut impl Db,
    filter: ExceedFilter,
    client: Option<ClientId>,
    now: DateTime<Utc>,
    page: PageRequest,
) -> StoreResult<Page<ExceedItem>> {
    let rows: Vec<(ExceedRow, Option<String>)> = {
        let current = filter == ExceedFilter::Current;
        let found: Vec<ExceedRowWithTariff> = sqlx::query_as(&format!(
            "SELECT {EXCEED_COLUMNS}, t.name AS tariff FROM traffic_exceeds e \
             LEFT JOIN subscriptions s ON s.id = e.subscription_id \
             LEFT JOIN tariffs t ON t.id = s.tariff_id \
             WHERE ($1::bigint IS NULL OR e.client_id = $1) \
               AND ($2 = (e.window_end > $3 OR (e.restricted_at IS NOT NULL AND e.lifted_at IS NULL))) \
               AND ($4::bigint IS NULL OR e.id < $4) \
             ORDER BY e.id DESC LIMIT $5"
        ))
        .bind(client.map(|c| c.0))
        .bind(current)
        .bind(now)
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(db.conn())
        .await?;
        found.into_iter().map(|r| (r.row, r.tariff)).collect()
    };
    let rules: HashMap<i64, Rule> = rules(db, false)
        .await?
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
    let mut items = Vec::with_capacity(rows.len());
    for (row, tariff) in rows {
        let exceed = Exceed::try_from(row)?;
        let rule = rules.get(&exceed.rule_id).map(Rule::describe);
        items.push(ExceedItem {
            exceed,
            client: None,
            tariff,
            rule,
        });
    }
    let mut page = Page::from_overfetch(items, page, |i| i.exceed.id);
    let ids: Vec<ClientId> = page.items.iter().map(|i| i.exceed.client_id).collect();
    let people = db.clients().people(&ids).await?;
    for item in &mut page.items {
        item.client = people.get(&item.exceed.client_id).cloned();
    }
    Ok(page)
}

#[derive(sqlx::FromRow)]
struct ExceedRowWithTariff {
    #[sqlx(flatten)]
    row: ExceedRow,
    tariff: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub rules: i64,
    pub current: i64,
    pub restricted: i64,
    pub week: i64,
}

pub async fn counts(db: &mut impl Db, now: DateTime<Utc>) -> StoreResult<Counts> {
    let (rules, current, restricted, week): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM traffic_limit_rules WHERE deleted_at IS NULL AND enabled), \
                (SELECT count(*) FROM traffic_exceeds WHERE window_end > $1), \
                (SELECT count(*) FROM traffic_exceeds WHERE restricted_at IS NOT NULL AND lifted_at IS NULL), \
                (SELECT count(*) FROM traffic_exceeds WHERE exceeded_at > $1 - interval '7 days')",
    )
    .bind(now)
    .fetch_one(db.conn())
    .await?;
    Ok(Counts {
        rules,
        current,
        restricted,
        week,
    })
}

pub async fn offset_minutes(db: &mut impl Db) -> StoreResult<i32> {
    let value = db
        .settings()
        .get(sn_domain::settings::keys::REMNA_UTC_OFFSET)
        .await?;
    Ok(value
        .and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(0)
        .clamp(-1439, 1439))
}

pub async fn remember_scan(db: &mut impl Db, value: &serde_json::Value) -> StoreResult<()> {
    db.bot_state().set(super::LAST_SCAN_KEY, value).await
}

pub async fn last_scan(db: &mut impl Db) -> StoreResult<Option<serde_json::Value>> {
    Ok(db
        .bot_state()
        .get(super::LAST_SCAN_KEY)
        .await?
        .filter(|v| !v.is_null()))
}
