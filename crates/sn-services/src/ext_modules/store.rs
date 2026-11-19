use super::manifest::Manifest;
use crate::error::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use sn_domain::access::{AdminRole, Permission, is_allowed};
use sn_domain::ids::AdminId;
use sn_store::Db;
use sqlx::types::Json;

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleRow {
    pub manifest: Manifest,
    pub enabled: bool,
    pub config: Map<String, Value>,
    pub api_key_id: Option<i64>,
    pub installed_by: AdminId,
    pub installed_by_email: String,
    pub installed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ModuleRow {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }
}

#[derive(sqlx::FromRow)]
struct RawModule {
    manifest: Json<Value>,
    enabled: bool,
    config: Json<Value>,
    api_key_id: Option<i64>,
    installed_by: i64,
    installed_by_email: String,
    installed_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "m.manifest, m.enabled, m.config, m.api_key_id, m.installed_by, \
     a.email AS installed_by_email, m.installed_at, m.updated_at";

fn row(raw: RawModule) -> ServiceResult<ModuleRow> {
    let manifest: Manifest = serde_json::from_value(raw.manifest.0).map_err(|e| {
        ServiceError::Internal(format!("описание внешнего модуля не читается: {e}"))
    })?;
    Ok(ModuleRow {
        manifest,
        enabled: raw.enabled,
        config: raw.config.0.as_object().cloned().unwrap_or_default(),
        api_key_id: raw.api_key_id,
        installed_by: AdminId(raw.installed_by),
        installed_by_email: raw.installed_by_email,
        installed_at: raw.installed_at,
        updated_at: raw.updated_at,
    })
}

pub async fn get(db: &mut impl Db, id: &str) -> ServiceResult<Option<ModuleRow>> {
    let raw: Option<RawModule> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM ext_modules m JOIN admins a ON a.id = m.installed_by WHERE m.id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    raw.map(row).transpose()
}

pub async fn lock(db: &mut impl Db, id: &str) -> ServiceResult<Option<ModuleRow>> {
    let raw: Option<RawModule> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM ext_modules m JOIN admins a ON a.id = m.installed_by \
         WHERE m.id = $1 FOR UPDATE OF m"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    raw.map(row).transpose()
}

pub async fn by_key(db: &mut impl Db, key_id: i64) -> ServiceResult<Option<ModuleRow>> {
    let raw: Option<RawModule> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM ext_modules m JOIN admins a ON a.id = m.installed_by WHERE m.api_key_id = $1"
    ))
    .bind(key_id)
    .fetch_optional(db.conn())
    .await?;
    raw.map(row).transpose()
}

pub async fn list(db: &mut impl Db) -> ServiceResult<Vec<ModuleRow>> {
    let rows: Vec<RawModule> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM ext_modules m JOIN admins a ON a.id = m.installed_by ORDER BY m.installed_at, m.id"
    ))
    .fetch_all(db.conn())
    .await?;
    rows.into_iter().map(row).collect()
}

fn manifest_json(manifest: &Manifest) -> ServiceResult<Value> {
    serde_json::to_value(manifest).map_err(|e| ServiceError::Internal(e.to_string()))
}

pub async fn insert(
    db: &mut impl Db,
    manifest: &Manifest,
    api_key_id: i64,
    by: AdminId,
) -> ServiceResult<()> {
    sqlx::query(
        "INSERT INTO ext_modules (id, manifest, events, api_key_id, installed_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $5)",
    )
    .bind(&manifest.id)
    .bind(Json(manifest_json(manifest)?))
    .bind(&manifest.events)
    .bind(api_key_id)
    .bind(by.0)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn update(
    db: &mut impl Db,
    manifest: &Manifest,
    config: &Map<String, Value>,
    by: AdminId,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE ext_modules SET manifest = $2, events = $3, config = $4, updated_by = $5, updated_at = now() \
         WHERE id = $1",
    )
    .bind(&manifest.id)
    .bind(Json(manifest_json(manifest)?))
    .bind(&manifest.events)
    .bind(Json(Value::Object(config.clone())))
    .bind(by.0)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn set_config(
    db: &mut impl Db,
    id: &str,
    config: &Map<String, Value>,
    by: AdminId,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE ext_modules SET config = $2, updated_by = $3, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(Json(Value::Object(config.clone())))
    .bind(by.0)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn set_enabled(
    db: &mut impl Db,
    id: &str,
    enabled: bool,
    by: AdminId,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE ext_modules SET enabled = $2, updated_by = $3, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(enabled)
    .bind(by.0)
    .execute(db.conn())
    .await?;
    if !enabled {
        skip_pending(db, id).await?;
    }
    Ok(())
}

pub async fn skip_pending(db: &mut impl Db, id: &str) -> ServiceResult<u64> {
    let done = sqlx::query(
        "UPDATE ext_module_deliveries SET status = 'skipped', next_attempt_at = NULL \
         WHERE module_id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected())
}

pub async fn set_key(db: &mut impl Db, id: &str, key_id: i64, by: AdminId) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE ext_modules SET api_key_id = $2, updated_by = $3, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(key_id)
    .bind(by.0)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn delete(db: &mut impl Db, id: &str) -> ServiceResult<bool> {
    let done = sqlx::query("DELETE FROM ext_modules WHERE id = $1")
        .bind(id)
        .execute(db.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Active,
    Missing,
    Revoked,
    Expired,
    OwnerDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyInfo {
    pub prefix: Option<String>,
    pub state: KeyState,
    pub owner: Option<String>,
    pub last_used_at: Option<DateTime<Utc>>,
}

pub async fn key_info(
    db: &mut impl Db,
    key_id: Option<i64>,
    now: DateTime<Utc>,
) -> ServiceResult<KeyInfo> {
    let missing = KeyInfo {
        prefix: None,
        state: KeyState::Missing,
        owner: None,
        last_used_at: None,
    };
    let Some(key_id) = key_id else {
        return Ok(missing);
    };
    type Raw = (
        String,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        String,
        Option<DateTime<Utc>>,
    );
    let raw: Option<Raw> = sqlx::query_as(
        "SELECT k.token_prefix, k.revoked_at, k.expires_at, k.last_used_at, a.email, a.disabled_at \
         FROM api_keys k JOIN admins a ON a.id = k.created_by WHERE k.id = $1",
    )
    .bind(key_id)
    .fetch_optional(db.conn())
    .await?;
    let Some((prefix, revoked_at, expires_at, last_used_at, owner, disabled_at)) = raw else {
        return Ok(missing);
    };
    Ok(KeyInfo {
        prefix: Some(prefix),
        state: key_state(revoked_at, expires_at, disabled_at, now),
        owner: Some(owner),
        last_used_at,
    })
}

fn key_state(
    revoked_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    disabled_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> KeyState {
    if revoked_at.is_some() {
        KeyState::Revoked
    } else if expires_at.is_some_and(|at| at <= now) {
        KeyState::Expired
    } else if disabled_at.is_some() {
        KeyState::OwnerDisabled
    } else {
        KeyState::Active
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub state: KeyState,
    pub granted: Vec<Permission>,
}

pub async fn access(db: &mut impl Db, module: &str, now: DateTime<Utc>) -> ServiceResult<Access> {
    type Raw = (
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Vec<String>,
        String,
        Vec<String>,
        Option<DateTime<Utc>>,
    );
    let raw: Option<Raw> = sqlx::query_as(
        "SELECT k.revoked_at, k.expires_at, k.permissions, a.role, a.permissions, a.disabled_at \
         FROM ext_modules m JOIN api_keys k ON k.id = m.api_key_id JOIN admins a ON a.id = k.created_by \
         WHERE m.id = $1",
    )
    .bind(module)
    .fetch_optional(db.conn())
    .await?;
    let Some((revoked_at, expires_at, key_rights, role, admin_rights, disabled_at)) = raw else {
        return Ok(Access {
            state: KeyState::Missing,
            granted: Vec::new(),
        });
    };
    let role = AdminRole::parse(&role)?;
    let parse = |rights: &[String]| -> Vec<Permission> {
        rights
            .iter()
            .filter_map(|p| Permission::parse(p).ok())
            .collect()
    };
    let extra = parse(&admin_rights);
    Ok(Access {
        state: key_state(revoked_at, expires_at, disabled_at, now),
        granted: parse(&key_rights)
            .into_iter()
            .filter(|p| is_allowed(role, &extra, *p))
            .collect(),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Claimed {
    pub id: i64,
    pub event_id: Option<i64>,
    pub kind: String,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
    pub attempts: u32,
}

type ClaimedRow = (i64, Option<i64>, String, Json<Value>, DateTime<Utc>, i32);

fn claimed(rows: Vec<ClaimedRow>) -> Vec<Claimed> {
    rows.into_iter()
        .map(
            |(id, event_id, kind, payload, occurred_at, attempts)| Claimed {
                id,
                event_id,
                kind,
                payload: payload.0,
                occurred_at,
                attempts: u32::try_from(attempts.max(0)).unwrap_or_default(),
            },
        )
        .collect()
}

pub async fn due_modules(db: &mut impl Db, now: DateTime<Utc>) -> ServiceResult<Vec<String>> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT d.module_id FROM ext_module_deliveries d \
         JOIN ext_modules m ON m.id = d.module_id \
         WHERE d.status = 'pending' AND (d.next_attempt_at IS NULL OR d.next_attempt_at <= $1)",
    )
    .bind(now)
    .fetch_all(db.conn())
    .await?;
    Ok(ids)
}

pub async fn claim(
    db: &mut impl Db,
    module: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: i64,
) -> ServiceResult<Vec<Claimed>> {
    let rows: Vec<ClaimedRow> = sqlx::query_as(
        "UPDATE ext_module_deliveries d SET next_attempt_at = $3 \
         WHERE d.id IN (SELECT id FROM ext_module_deliveries \
         WHERE module_id = $1 AND status = 'pending' AND (next_attempt_at IS NULL OR next_attempt_at <= $2) \
         ORDER BY next_attempt_at NULLS FIRST, id LIMIT $4 FOR UPDATE SKIP LOCKED) \
         RETURNING d.id, d.event_id, d.kind, d.payload, d.occurred_at, d.attempts",
    )
    .bind(module)
    .bind(now)
    .bind(lease_until)
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    let mut out = claimed(rows);
    out.sort_by_key(|c| c.id);
    Ok(out)
}

pub async fn enqueue_service(
    db: &mut impl Db,
    module: &str,
    kind: &str,
    payload: &Value,
    lease_until: Option<DateTime<Utc>>,
) -> ServiceResult<Claimed> {
    let rows: Vec<ClaimedRow> = sqlx::query_as(
        "INSERT INTO ext_module_deliveries (module_id, kind, payload, next_attempt_at) VALUES ($1, $2, $3, $4) \
         RETURNING id, event_id, kind, payload, occurred_at, attempts",
    )
    .bind(module)
    .bind(kind)
    .bind(Json(payload))
    .bind(lease_until)
    .fetch_all(db.conn())
    .await?;
    claimed(rows)
        .pop()
        .ok_or_else(|| ServiceError::Internal("доставка не создана".into()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: &'static str,
    pub attempts: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub http_status: Option<u16>,
    pub duration_ms: u32,
    pub error: Option<String>,
    pub response: Option<String>,
}

pub async fn record(
    db: &mut impl Db,
    module: &str,
    delivery: i64,
    at: DateTime<Utc>,
    outcome: &Outcome,
) -> ServiceResult<bool> {
    let updated = sqlx::query(
        "UPDATE ext_module_deliveries SET status = $3, attempts = $4, next_attempt_at = $5, last_attempt_at = $6 \
         WHERE id = $1 AND module_id = $2",
    )
    .bind(delivery)
    .bind(module)
    .bind(outcome.status)
    .bind(i32::try_from(outcome.attempts).unwrap_or(i32::MAX))
    .bind(outcome.next_attempt_at)
    .bind(at)
    .execute(db.conn())
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO ext_module_attempts (delivery_id, module_id, attempt, at, http_status, duration_ms, error, response) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(delivery)
    .bind(module)
    .bind(i32::try_from(outcome.attempts.max(1)).unwrap_or(i32::MAX))
    .bind(at)
    .bind(outcome.http_status.map(i32::from))
    .bind(i32::try_from(outcome.duration_ms).unwrap_or(i32::MAX))
    .bind(outcome.error.as_deref())
    .bind(outcome.response.as_deref())
    .execute(db.conn())
    .await?;
    Ok(true)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptRow {
    pub id: i64,
    pub delivery_id: i64,
    pub kind: String,
    pub event_id: Option<i64>,
    pub attempt: i32,
    pub at: DateTime<Utc>,
    pub http_status: Option<i32>,
    pub duration_ms: i32,
    pub error: Option<String>,
    pub response: Option<String>,
    pub status: String,
    pub next_attempt_at: Option<DateTime<Utc>>,
}

pub async fn attempts(
    db: &mut impl Db,
    module: &str,
    limit: i64,
) -> ServiceResult<Vec<AttemptRow>> {
    type Raw = (
        i64,
        i64,
        String,
        Option<i64>,
        i32,
        DateTime<Utc>,
        Option<i32>,
        i32,
        Option<String>,
        Option<String>,
        String,
        Option<DateTime<Utc>>,
    );
    let rows: Vec<Raw> = sqlx::query_as(
        "SELECT a.id, a.delivery_id, d.kind, d.event_id, a.attempt, a.at, a.http_status, a.duration_ms, \
         a.error, a.response, d.status, d.next_attempt_at \
         FROM ext_module_attempts a JOIN ext_module_deliveries d ON d.id = a.delivery_id \
         WHERE a.module_id = $1 ORDER BY a.id DESC LIMIT $2",
    )
    .bind(module)
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                delivery_id,
                kind,
                event_id,
                attempt,
                at,
                http_status,
                duration_ms,
                error,
                response,
                status,
                next_attempt_at,
            )| AttemptRow {
                id,
                delivery_id,
                kind,
                event_id,
                attempt,
                at,
                http_status,
                duration_ms,
                error,
                response,
                status,
                next_attempt_at,
            },
        )
        .collect())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Stats {
    pub pending: i64,
    pub failed: i64,
    pub last_ok: Option<bool>,
    pub last_at: Option<DateTime<Utc>>,
}

pub async fn stats(db: &mut impl Db, module: &str) -> ServiceResult<Stats> {
    let (pending, failed): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'pending'), count(*) FILTER (WHERE status = 'failed') \
         FROM ext_module_deliveries WHERE module_id = $1 AND kind <> 'ping'",
    )
    .bind(module)
    .fetch_one(db.conn())
    .await?;
    let last: Option<(Option<i32>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT a.http_status, a.at FROM ext_module_attempts a \
         JOIN ext_module_deliveries d ON d.id = a.delivery_id \
         WHERE a.module_id = $1 AND d.kind <> 'ping' ORDER BY a.id DESC LIMIT 1",
    )
    .bind(module)
    .fetch_optional(db.conn())
    .await?;
    Ok(Stats {
        pending,
        failed,
        last_ok: last.map(|(code, _)| code.is_some_and(|c| (200..300).contains(&c))),
        last_at: last.map(|(_, at)| at),
    })
}

pub async fn retry(db: &mut impl Db, module: &str, delivery: i64) -> ServiceResult<bool> {
    let done = sqlx::query(
        "UPDATE ext_module_deliveries SET status = 'pending', attempts = 0, next_attempt_at = NULL \
         WHERE id = $1 AND module_id = $2 AND status IN ('failed', 'skipped') AND kind <> 'ping'",
    )
    .bind(delivery)
    .bind(module)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn retry_failed(db: &mut impl Db, module: &str) -> ServiceResult<u64> {
    let done = sqlx::query(
        "UPDATE ext_module_deliveries SET status = 'pending', attempts = 0, next_attempt_at = NULL \
         WHERE module_id = $1 AND status = 'failed' AND kind <> 'ping'",
    )
    .bind(module)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected())
}

pub async fn cleanup(db: &mut impl Db, before: DateTime<Utc>) -> ServiceResult<u64> {
    let attempts = sqlx::query("DELETE FROM ext_module_attempts WHERE at < $1")
        .bind(before)
        .execute(db.conn())
        .await?;
    let deliveries = sqlx::query(
        "DELETE FROM ext_module_deliveries WHERE status <> 'pending' AND created_at < $1",
    )
    .bind(before)
    .execute(db.conn())
    .await?;
    Ok(attempts.rows_affected() + deliveries.rows_affected())
}
