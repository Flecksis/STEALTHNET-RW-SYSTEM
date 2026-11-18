use super::{Plan, Progress};
use crate::error::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::ids::AdminId;
use sn_store::Db;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Push,
    Pull,
}

impl Direction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Direction::Push => "push",
            Direction::Pull => "pull",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "push" => Some(Direction::Push),
            "pull" => Some(Direction::Pull),
            _ => None,
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Direction::Push => "Перенос в Remnawave",
            Direction::Pull => "Забор из Remnawave",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Create,
    Fix,
}

impl Mode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Create => "create",
            Mode::Fix => "fix",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "create" => Some(Mode::Create),
            "fix" => Some(Mode::Fix),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Checking,
    Ready,
    Running,
    Paused,
    Done,
    Failed,
    Interrupted,
    Discarded,
}

impl TransferState {
    pub const fn as_str(self) -> &'static str {
        match self {
            TransferState::Checking => "checking",
            TransferState::Ready => "ready",
            TransferState::Running => "running",
            TransferState::Paused => "paused",
            TransferState::Done => "done",
            TransferState::Failed => "failed",
            TransferState::Interrupted => "interrupted",
            TransferState::Discarded => "discarded",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "checking" => TransferState::Checking,
            "ready" => TransferState::Ready,
            "running" => TransferState::Running,
            "paused" => TransferState::Paused,
            "done" => TransferState::Done,
            "interrupted" => TransferState::Interrupted,
            "discarded" => TransferState::Discarded,
            _ => TransferState::Failed,
        }
    }

    pub const fn resumable(self) -> bool {
        matches!(
            self,
            TransferState::Ready
                | TransferState::Paused
                | TransferState::Interrupted
                | TransferState::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferView {
    pub id: Uuid,
    pub direction: Direction,
    pub state: TransferState,
    pub mode: Option<Mode>,
    pub plan: Option<Plan>,
    pub progress: Option<Progress>,
    pub outcome: Option<Progress>,
    pub error: Option<String>,
    pub pause_requested: bool,
    pub notes: i64,
    pub link_changed: i64,
    pub notified_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct TransferRow {
    pub id: i64,
    pub public_id: Uuid,
    pub direction: String,
    pub state: String,
    pub mode: Option<String>,
    pub plan: Option<serde_json::Value>,
    pub progress: Option<serde_json::Value>,
    pub outcome: Option<serde_json::Value>,
    pub error: Option<String>,
    pub pause_requested: bool,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub notes: i64,
    pub link_changed: i64,
    pub notified_at: Option<DateTime<Utc>>,
}

impl TransferRow {
    pub fn direction(&self) -> Direction {
        Direction::parse(&self.direction).unwrap_or_default()
    }

    pub fn state(&self) -> TransferState {
        TransferState::parse(&self.state)
    }

    pub fn mode(&self) -> Option<Mode> {
        self.mode.as_deref().and_then(Mode::parse)
    }

    pub fn plan(&self) -> Option<Plan> {
        self.plan
            .clone()
            .and_then(|v| serde_json::from_value(v).ok())
    }

    pub fn progress(&self) -> Option<Progress> {
        self.progress
            .clone()
            .and_then(|v| serde_json::from_value(v).ok())
    }

    pub fn view(&self) -> TransferView {
        TransferView {
            id: self.public_id,
            direction: self.direction(),
            state: self.state(),
            mode: self.mode(),
            plan: self.plan(),
            progress: self.progress(),
            outcome: self
                .outcome
                .clone()
                .and_then(|v| serde_json::from_value(v).ok()),
            error: self.error.clone(),
            pause_requested: self.pause_requested,
            notes: self.notes,
            link_changed: self.link_changed,
            notified_at: self.notified_at,
            created_at: self.created_at,
            started_at: self.started_at,
            finished_at: self.finished_at,
        }
    }
}

const SELECT: &str = "SELECT t.id, t.public_id, t.direction, t.state, t.mode, t.plan, t.progress, t.outcome, t.error, \
     t.pause_requested, t.created_at, t.started_at, t.finished_at, \
     (SELECT count(*) FROM remna_transfer_notes n WHERE n.transfer_id = t.id) AS notes, \
     (SELECT count(*) FROM remna_transfer_notes n WHERE n.transfer_id = t.id AND n.stage = 'apply' AND n.code = 'link_changed') AS link_changed, \
     (t.outcome->>'notified_at')::timestamptz AS notified_at \
     FROM remna_transfers t";

fn json<T: Serialize>(value: &T) -> ServiceResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| ServiceError::Internal(e.to_string()))
}

pub(crate) fn busy_error() -> ServiceError {
    ServiceError::rejected(
        "remna_sync.busy",
        "Уже идёт проверка или перенос. Дождитесь окончания",
    )
}

fn busy_or(err: sqlx::Error) -> ServiceError {
    if err.as_database_error().and_then(|d| d.constraint()) == Some("remna_transfers_busy_key") {
        busy_error()
    } else {
        ServiceError::from(sn_store::StoreError::from(err))
    }
}

pub(crate) async fn create(
    db: &mut impl Db,
    direction: Direction,
    state: TransferState,
    plan: Option<&Plan>,
    admin: Option<AdminId>,
) -> ServiceResult<TransferRow> {
    let plan = plan.map(json).transpose()?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO remna_transfers (direction, state, plan, created_by, started_by, started_at) \
         VALUES ($1, $2, $3, $4, CASE WHEN $2 = 'running' THEN $4 END, CASE WHEN $2 = 'running' THEN now() END) \
         RETURNING id",
    )
    .bind(direction.as_str())
    .bind(state.as_str())
    .bind(plan)
    .bind(admin.map(|a| a.0))
    .fetch_one(db.conn())
    .await
    .map_err(busy_or)?;
    get_by_id(db, id).await
}

pub(crate) async fn get_by_id(db: &mut impl Db, id: i64) -> ServiceResult<TransferRow> {
    sqlx::query_as::<_, TransferRow>(&format!("{SELECT} WHERE t.id = $1"))
        .bind(id)
        .fetch_optional(db.conn())
        .await?
        .ok_or_else(not_found)
}

fn not_found() -> ServiceError {
    ServiceError::rejected(
        "remna_sync.not_found",
        "Проверка не найдена. Запустите новую",
    )
}

pub(crate) async fn get(db: &mut impl Db, public_id: Uuid) -> ServiceResult<TransferRow> {
    sqlx::query_as::<_, TransferRow>(&format!("{SELECT} WHERE t.public_id = $1"))
        .bind(public_id)
        .fetch_optional(db.conn())
        .await?
        .ok_or_else(not_found)
}

pub(crate) async fn current(db: &mut impl Db) -> ServiceResult<Option<TransferRow>> {
    Ok(sqlx::query_as::<_, TransferRow>(&format!(
        "{SELECT} WHERE t.state <> 'discarded' ORDER BY t.id DESC LIMIT 1"
    ))
    .fetch_optional(db.conn())
    .await?)
}

pub(crate) async fn history(db: &mut impl Db, limit: i64) -> ServiceResult<Vec<TransferRow>> {
    Ok(sqlx::query_as::<_, TransferRow>(&format!(
        "{SELECT} WHERE t.state IN ('done', 'failed', 'interrupted', 'paused') AND t.started_at IS NOT NULL \
         ORDER BY t.id DESC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(db.conn())
    .await?)
}

pub(crate) async fn set_plan(db: &mut impl Db, id: i64, plan: &Plan) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE remna_transfers SET plan = $2, state = 'ready', error = NULL WHERE id = $1 AND state = 'checking'",
    )
    .bind(id)
    .bind(json(plan)?)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub(crate) async fn set_state(
    db: &mut impl Db,
    id: i64,
    state: TransferState,
    error: Option<&str>,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE remna_transfers SET state = $2, error = $3, pause_requested = false, \
         finished_at = CASE WHEN $2 IN ('done', 'failed', 'interrupted', 'paused') THEN now() ELSE finished_at END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(state.as_str())
    .bind(error)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub(crate) async fn begin(
    db: &mut impl Db,
    id: i64,
    mode: Mode,
    admin: Option<AdminId>,
) -> ServiceResult<bool> {
    let done = sqlx::query(
        "UPDATE remna_transfers SET state = 'running', mode = $2, error = NULL, pause_requested = false, \
         started_by = COALESCE($3, started_by), started_at = COALESCE(started_at, now()), finished_at = NULL \
         WHERE id = $1 AND state IN ('ready', 'paused', 'interrupted', 'failed') AND plan IS NOT NULL",
    )
    .bind(id)
    .bind(mode.as_str())
    .bind(admin.map(|a| a.0))
    .execute(db.conn())
    .await
    .map_err(busy_or)?;
    Ok(done.rows_affected() == 1)
}

pub(crate) async fn set_progress(
    db: &mut impl Db,
    id: i64,
    progress: &Progress,
) -> ServiceResult<bool> {
    let requested: Option<bool> = sqlx::query_scalar(
        "UPDATE remna_transfers SET progress = $2 WHERE id = $1 RETURNING pause_requested",
    )
    .bind(id)
    .bind(json(progress)?)
    .fetch_optional(db.conn())
    .await?;
    Ok(requested.unwrap_or(false))
}

pub(crate) async fn finish(db: &mut impl Db, id: i64, outcome: &Progress) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE remna_transfers SET state = 'done', outcome = $2, progress = $2, error = NULL, \
         pause_requested = false, finished_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(json(outcome)?)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub(crate) async fn request_pause(db: &mut impl Db, id: i64) -> ServiceResult<bool> {
    let done = sqlx::query(
        "UPDATE remna_transfers SET pause_requested = true WHERE id = $1 AND state = 'running'",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

pub(crate) async fn discard(db: &mut impl Db, id: i64) -> ServiceResult<bool> {
    let done = sqlx::query(
        "UPDATE remna_transfers SET state = 'discarded' WHERE id = $1 AND state IN ('ready', 'failed', 'interrupted', 'paused', 'done')",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

pub(crate) async fn recover(db: &mut impl Db) -> ServiceResult<u64> {
    let checking = sqlx::query(
        "UPDATE remna_transfers SET state = 'failed', error = 'Проверка прервалась при перезапуске сервера. Запустите её снова', \
         finished_at = now() WHERE state = 'checking'",
    )
    .execute(db.conn())
    .await?;
    let running = sqlx::query(
        "UPDATE remna_transfers SET state = 'interrupted', pause_requested = false, finished_at = now() \
         WHERE state = 'running'",
    )
    .execute(db.conn())
    .await?;
    Ok(checking.rows_affected() + running.rows_affected())
}

pub(crate) async fn mark_notified(db: &mut impl Db, id: i64) -> ServiceResult<bool> {
    let done = sqlx::query(
        "UPDATE remna_transfers SET outcome = jsonb_set(COALESCE(outcome, '{}'::jsonb), '{notified_at}', to_jsonb(now())) \
         WHERE id = $1 AND state = 'done' AND NOT (COALESCE(outcome, '{}'::jsonb) ? 'notified_at')",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct NoteRow {
    pub stage: String,
    pub code: String,
    pub client_id: Option<i64>,
    pub subscription_id: Option<i64>,
    pub remna_user_id: Option<i64>,
    pub who: Option<String>,
    pub details: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl NoteRow {
    pub fn new(code: &str, details: impl Into<String>) -> Self {
        NoteRow {
            stage: "check".into(),
            code: code.into(),
            details: details.into(),
            data: serde_json::json!({}),
            ..NoteRow::default()
        }
    }
}

pub(crate) async fn clear_notes(db: &mut impl Db, id: i64, stage: &str) -> ServiceResult<()> {
    sqlx::query("DELETE FROM remna_transfer_notes WHERE transfer_id = $1 AND stage = $2")
        .bind(id)
        .bind(stage)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub(crate) async fn add_notes(db: &mut impl Db, id: i64, notes: &[NoteRow]) -> ServiceResult<()> {
    for chunk in notes.chunks(500) {
        let stages: Vec<&str> = chunk.iter().map(|n| n.stage.as_str()).collect();
        let codes: Vec<&str> = chunk.iter().map(|n| n.code.as_str()).collect();
        let clients: Vec<Option<i64>> = chunk.iter().map(|n| n.client_id).collect();
        let subs: Vec<Option<i64>> = chunk.iter().map(|n| n.subscription_id).collect();
        let users: Vec<Option<i64>> = chunk.iter().map(|n| n.remna_user_id).collect();
        let who: Vec<Option<&str>> = chunk.iter().map(|n| n.who.as_deref()).collect();
        let details: Vec<&str> = chunk.iter().map(|n| n.details.as_str()).collect();
        let data: Vec<serde_json::Value> = chunk
            .iter()
            .map(|n| {
                if n.data.is_null() {
                    serde_json::json!({})
                } else {
                    n.data.clone()
                }
            })
            .collect();
        sqlx::query(
            "INSERT INTO remna_transfer_notes (transfer_id, stage, code, client_id, subscription_id, remna_user_id, who, details, data) \
             SELECT $1, * FROM unnest($2::text[], $3::text[], $4::bigint[], $5::bigint[], $6::bigint[], $7::text[], $8::text[], $9::jsonb[])",
        )
        .bind(id)
        .bind(&stages)
        .bind(&codes)
        .bind(&clients)
        .bind(&subs)
        .bind(&users)
        .bind(&who)
        .bind(&details)
        .bind(&data)
        .execute(db.conn())
        .await?;
    }
    Ok(())
}

pub(crate) async fn notes(db: &mut impl Db, id: i64) -> ServiceResult<Vec<NoteRow>> {
    Ok(sqlx::query_as::<_, NoteRow>(
        "SELECT stage, code, client_id, subscription_id, remna_user_id, who, details, data \
         FROM remna_transfer_notes WHERE transfer_id = $1 ORDER BY stage DESC, id",
    )
    .bind(id)
    .fetch_all(db.conn())
    .await?)
}

pub(crate) async fn link_notes(db: &mut impl Db, id: i64) -> ServiceResult<Vec<NoteRow>> {
    Ok(sqlx::query_as::<_, NoteRow>(
        "SELECT stage, code, client_id, subscription_id, remna_user_id, who, details, data \
         FROM remna_transfer_notes WHERE transfer_id = $1 AND stage = 'apply' AND code = 'link_changed' ORDER BY id",
    )
    .bind(id)
    .fetch_all(db.conn())
    .await?)
}
