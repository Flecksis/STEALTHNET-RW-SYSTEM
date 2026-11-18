use super::model::{Skip, Source, TariffMode};
use crate::error::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::ids::AdminId;
use sn_domain::money::Currency;
use sn_store::Db;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Uploaded,
    Checking,
    NeedsPassword,
    Ready,
    Failed,
    Running,
    Done,
    Interrupted,
    Discarded,
}

impl RunState {
    pub const fn as_str(self) -> &'static str {
        match self {
            RunState::Uploaded => "uploaded",
            RunState::Checking => "checking",
            RunState::NeedsPassword => "needs_password",
            RunState::Ready => "ready",
            RunState::Failed => "failed",
            RunState::Running => "running",
            RunState::Done => "done",
            RunState::Interrupted => "interrupted",
            RunState::Discarded => "discarded",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "uploaded" => RunState::Uploaded,
            "checking" => RunState::Checking,
            "needs_password" => RunState::NeedsPassword,
            "ready" => RunState::Ready,
            "running" => RunState::Running,
            "done" => RunState::Done,
            "interrupted" => RunState::Interrupted,
            "discarded" => RunState::Discarded,
            _ => RunState::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub clients: u64,
    pub subscriptions: u64,
    pub tariffs: u64,
    pub promo_codes: u64,
    pub referrals: u64,
    pub payments: u64,
    pub tickets: u64,
    #[serde(default)]
    pub admins: u64,
    #[serde(default)]
    pub gifts: u64,
    #[serde(default)]
    pub auto_rules: u64,
    #[serde(default)]
    pub withdrawals: u64,
    #[serde(default)]
    pub settings: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Balances {
    pub clients: u64,
    pub total_minor: i64,
    pub currency: Currency,
    #[serde(default)]
    pub large_clients: u64,
    #[serde(default)]
    pub large_total_minor: i64,
    #[serde(default)]
    pub large_from_minor: Option<i64>,
    #[serde(default)]
    pub large_by_tariff: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrencyPlan {
    pub source: Currency,
    pub install: Currency,
    pub switch: bool,
    pub blocked: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemnaPlan {
    pub connected: bool,
    pub error: Option<String>,
    pub users: u64,
    pub found: u64,
    pub missing: u64,
    pub ambiguous: u64,
    #[serde(default)]
    pub shared: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Existing {
    pub clients: u64,
    pub matched: u64,
    pub again: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub detected: Source,
    pub dumped_at: Option<DateTime<Utc>>,
    pub counts: Counts,
    pub balances: Balances,
    pub currency: CurrencyPlan,
    pub remna: RemnaPlan,
    pub existing: Existing,
    pub tariff_mode: TariffMode,
    pub skipped: Vec<Skip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail: Option<super::mail::MailPlan>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOptions {
    #[serde(default)]
    pub replace_mail: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Read,
    Tariffs,
    Clients,
    Subscriptions,
    Payments,
    Promo,
    Referrals,
    Tickets,
    Extras,
    Finish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub stage: Stage,
    pub done: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub counts: Counts,
    pub created: u64,
    pub merged: u64,
    pub updated: u64,
    pub issues: u64,
    pub currency_switched: bool,
    pub balance_total_minor: i64,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileView {
    pub name: String,
    pub size: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub id: Uuid,
    pub source: Source,
    pub state: RunState,
    pub file: FileView,
    pub preview: Option<Preview>,
    pub progress: Option<Progress>,
    pub outcome: Option<Outcome>,
    pub error: Option<String>,
    pub issues: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: i64,
    pub public_id: Uuid,
    pub source: Source,
    pub state: RunState,
    pub file_name: String,
    pub file_size: i64,
    pub file_path: Option<PathBuf>,
    pub preview: Option<Preview>,
    pub created_by: Option<AdminId>,
}

#[derive(sqlx::FromRow)]
struct Raw {
    id: i64,
    public_id: Uuid,
    source: String,
    state: String,
    file_name: String,
    file_size: i64,
    file_path: Option<String>,
    preview: Option<serde_json::Value>,
    progress: Option<serde_json::Value>,
    outcome: Option<serde_json::Value>,
    error: Option<String>,
    created_by: Option<i64>,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    email: Option<String>,
    issues: i64,
}

const SELECT: &str = "SELECT r.id, r.public_id, r.source, r.state, r.file_name, r.file_size, r.file_path, \
    r.preview, r.progress, r.outcome, r.error, r.created_by, r.created_at, r.started_at, r.finished_at, a.email, \
    (SELECT count(*) FROM import_issues i WHERE i.run_id = r.id) AS issues \
    FROM import_runs r LEFT JOIN admins a ON a.id = r.created_by";

fn decode<T: serde::de::DeserializeOwned>(value: Option<serde_json::Value>) -> Option<T> {
    value.and_then(|v| serde_json::from_value(v).ok())
}

fn source_of(raw: &str) -> Source {
    Source::parse(raw).unwrap_or(Source::Stealthnet)
}

fn row_of(raw: Raw) -> (RunRow, RunView) {
    let source = source_of(&raw.source);
    let state = RunState::parse(&raw.state);
    let preview: Option<Preview> = decode(raw.preview);
    let row = RunRow {
        id: raw.id,
        public_id: raw.public_id,
        source,
        state,
        file_name: raw.file_name.clone(),
        file_size: raw.file_size,
        file_path: raw.file_path.map(PathBuf::from),
        preview: preview.clone(),
        created_by: raw.created_by.map(AdminId),
    };
    let view = RunView {
        id: raw.public_id,
        source,
        state,
        file: FileView {
            name: raw.file_name,
            size: raw.file_size,
        },
        preview,
        progress: decode(raw.progress),
        outcome: decode(raw.outcome),
        error: raw.error,
        issues: raw.issues,
        created_at: raw.created_at,
        started_at: raw.started_at,
        finished_at: raw.finished_at,
        by: raw.email,
    };
    (row, view)
}

pub async fn create(
    db: &mut impl Db,
    source: Source,
    file_name: &str,
    file_size: i64,
    path: &std::path::Path,
    admin: Option<AdminId>,
) -> ServiceResult<RunRow> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO import_runs (source, state, file_name, file_size, file_path, created_by) \
         VALUES ($1, 'checking', $2, $3, $4, $5) RETURNING id",
    )
    .bind(source.as_str())
    .bind(file_name)
    .bind(file_size)
    .bind(path.to_string_lossy().to_string())
    .bind(admin.map(|a| a.0))
    .fetch_one(db.conn())
    .await?;
    Ok(get_by_id(db, id).await?.0)
}

pub async fn get_by_id(db: &mut impl Db, id: i64) -> ServiceResult<(RunRow, RunView)> {
    let raw: Option<Raw> = sqlx::query_as(&format!("{SELECT} WHERE r.id = $1"))
        .bind(id)
        .fetch_optional(db.conn())
        .await?;
    raw.map(row_of).ok_or(ServiceError::NotFound("перенос"))
}

pub async fn get(db: &mut impl Db, public_id: Uuid) -> ServiceResult<(RunRow, RunView)> {
    let raw: Option<Raw> = sqlx::query_as(&format!("{SELECT} WHERE r.public_id = $1"))
        .bind(public_id)
        .fetch_optional(db.conn())
        .await?;
    raw.map(row_of).ok_or(ServiceError::NotFound("перенос"))
}

pub async fn current(db: &mut impl Db) -> ServiceResult<Option<(RunRow, RunView)>> {
    let raw: Option<Raw> = sqlx::query_as(&format!("{SELECT} ORDER BY r.id DESC LIMIT 1"))
        .fetch_optional(db.conn())
        .await?;
    Ok(raw
        .map(row_of)
        .filter(|(row, _)| row.state != RunState::Discarded))
}

pub async fn history(db: &mut impl Db, limit: i64) -> ServiceResult<Vec<RunView>> {
    let raws: Vec<Raw> = sqlx::query_as(&format!(
        "{SELECT} WHERE r.state = 'done' ORDER BY r.id DESC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    Ok(raws.into_iter().map(|r| row_of(r).1).collect())
}

pub async fn running(db: &mut impl Db) -> ServiceResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM import_runs WHERE state IN ('running', 'checking') ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(db.conn())
    .await?)
}

pub async fn set_state(
    db: &mut impl Db,
    id: i64,
    state: RunState,
    error: Option<&str>,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE import_runs SET state = $2, error = $3, \
         started_at = CASE WHEN $2 = 'running' THEN now() ELSE started_at END, \
         finished_at = CASE WHEN $2 IN ('done', 'failed', 'interrupted') AND state = 'running' THEN now() ELSE finished_at END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(state.as_str())
    .bind(error)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn set_file(
    db: &mut impl Db,
    id: i64,
    path: Option<&std::path::Path>,
) -> ServiceResult<()> {
    sqlx::query("UPDATE import_runs SET file_path = $2 WHERE id = $1")
        .bind(id)
        .bind(path.map(|p| p.to_string_lossy().to_string()))
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn set_source(db: &mut impl Db, id: i64, source: Source) -> ServiceResult<()> {
    sqlx::query("UPDATE import_runs SET source = $2 WHERE id = $1")
        .bind(id)
        .bind(source.as_str())
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn set_preview(db: &mut impl Db, id: i64, preview: &Preview) -> ServiceResult<()> {
    sqlx::query("UPDATE import_runs SET preview = $2, state = 'ready', error = NULL WHERE id = $1")
        .bind(id)
        .bind(serde_json::to_value(preview).map_err(|e| ServiceError::Internal(e.to_string()))?)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn set_progress(db: &mut impl Db, id: i64, progress: &Progress) -> ServiceResult<()> {
    sqlx::query("UPDATE import_runs SET progress = $2 WHERE id = $1")
        .bind(id)
        .bind(serde_json::to_value(progress).map_err(|e| ServiceError::Internal(e.to_string()))?)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn finish(db: &mut impl Db, id: i64, outcome: &Outcome) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE import_runs SET state = 'done', outcome = $2, error = NULL, finished_at = now(), file_path = NULL WHERE id = $1",
    )
    .bind(id)
    .bind(serde_json::to_value(outcome).map_err(|e| ServiceError::Internal(e.to_string()))?)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn discard_open(db: &mut impl Db) -> ServiceResult<Vec<PathBuf>> {
    let paths: Vec<Option<String>> = sqlx::query_scalar(
        "WITH old AS (SELECT id, file_path FROM import_runs \
         WHERE state IN ('uploaded', 'needs_password', 'ready', 'failed', 'interrupted') FOR UPDATE) \
         UPDATE import_runs r SET state = 'discarded', file_path = NULL FROM old WHERE r.id = old.id \
         RETURNING old.file_path",
    )
    .fetch_all(db.conn())
    .await?;
    Ok(paths.into_iter().flatten().map(PathBuf::from).collect())
}

pub async fn recover(db: &mut impl Db) -> ServiceResult<u64> {
    let interrupted = sqlx::query(
        "UPDATE import_runs SET state = 'interrupted', finished_at = now(), \
         error = 'Перенос прервался: сервер перезапускался' WHERE state = 'running'",
    )
    .execute(db.conn())
    .await?
    .rows_affected();
    sqlx::query(
        "UPDATE import_runs SET state = 'failed', error = 'Проверка прервалась: сервер перезапускался. Загрузите файл ещё раз' \
         WHERE state = 'checking'",
    )
    .execute(db.conn())
    .await?;
    Ok(interrupted)
}

#[derive(Debug, Clone)]
pub struct IssueRow {
    pub stage: &'static str,
    pub code: String,
    pub entity: String,
    pub old_id: Option<String>,
    pub who: Option<String>,
    pub details: String,
}

pub async fn clear_issues(db: &mut impl Db, id: i64, stage: &str) -> ServiceResult<()> {
    sqlx::query("DELETE FROM import_issues WHERE run_id = $1 AND stage = $2")
        .bind(id)
        .bind(stage)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn add_issues(db: &mut impl Db, id: i64, issues: &[IssueRow]) -> ServiceResult<()> {
    for chunk in issues.chunks(1000) {
        let stage: Vec<&str> = chunk.iter().map(|i| i.stage).collect();
        let code: Vec<&str> = chunk.iter().map(|i| i.code.as_str()).collect();
        let entity: Vec<&str> = chunk.iter().map(|i| i.entity.as_str()).collect();
        let old: Vec<Option<&str>> = chunk.iter().map(|i| i.old_id.as_deref()).collect();
        let who: Vec<Option<&str>> = chunk.iter().map(|i| i.who.as_deref()).collect();
        let details: Vec<&str> = chunk.iter().map(|i| i.details.as_str()).collect();
        sqlx::query(
            "INSERT INTO import_issues (run_id, stage, code, entity, old_id, who, details) \
             SELECT $1, * FROM unnest($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[])",
        )
        .bind(id)
        .bind(&stage)
        .bind(&code)
        .bind(&entity)
        .bind(&old)
        .bind(&who)
        .bind(&details)
        .execute(db.conn())
        .await?;
    }
    Ok(())
}

type IssueRaw = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
);

pub async fn issues(db: &mut impl Db, id: i64) -> ServiceResult<Vec<IssueRow>> {
    let rows: Vec<IssueRaw> = sqlx::query_as(
        "SELECT stage, code, entity, old_id, who, details FROM import_issues WHERE run_id = $1 \
         ORDER BY CASE WHEN code LIKE 'balance%' THEN 0 WHEN stage = 'apply' THEN 1 ELSE 2 END, id",
    )
    .bind(id)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(stage, code, entity, old_id, who, details)| IssueRow {
            stage: if stage == "apply" { "apply" } else { "check" },
            code,
            entity,
            old_id,
            who,
            details,
        })
        .collect())
}
