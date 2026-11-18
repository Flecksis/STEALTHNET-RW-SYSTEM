mod pull;
mod push;
pub mod remna;
pub mod runs;
pub(crate) mod snapshot;
#[cfg(test)]
mod tests;
mod text;

use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::ids::{AdminId, ClientId};
use sn_domain::jobs::JobPayload;
use sn_store::Db;
use std::collections::BTreeMap;
use uuid::Uuid;

pub use remna::Pace;
pub use runs::{Direction, Mode, NoteRow, TransferState, TransferView};

pub const SYNC_PATH: &str = "/admin/settings/remnawave/sync";
pub const EXAMPLES: usize = 5;
pub const AUDIT_STARTED: &str = "remna.transfer.started";
pub const AUDIT_FINISHED: &str = "remna.transfer.finished";
pub const AUDIT_NOTIFIED: &str = "remna.transfer.notified";
pub const LINK_TEMPLATE: &str = "subscription_link";
pub const AUDIT_ENTITY: &str = "remna_transfer";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Example {
    pub who: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    pub count: u64,
    pub examples: Vec<Example>,
}

impl Bucket {
    pub(crate) fn add(&mut self, example: impl FnOnce() -> Example) {
        self.count += 1;
        if self.examples.len() < EXAMPLES {
            self.examples.push(example());
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub code: String,
    pub text: String,
    pub count: u64,
    #[serde(default)]
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemnaSquad {
    pub uuid: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SquadPlan {
    pub old: Uuid,
    pub name: Option<String>,
    pub subscriptions: u64,
    pub tariffs: Vec<String>,
    pub suggested: Option<Uuid>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub direction: Direction,
    pub remna_users: u64,
    pub scanned: u64,
    pub sub_domain: Option<String>,
    pub create: Bucket,
    pub fix: Bucket,
    pub fields: BTreeMap<String, u64>,
    pub same: u64,
    pub relink: Bucket,
    pub attach: Bucket,
    pub matched_by: BTreeMap<String, u64>,
    pub skipped: Vec<Reason>,
    pub link_change: Bucket,
    pub keys_kept: u64,
    pub keys_new: u64,
    pub later_in_remna: u64,
    pub expired: u64,
    pub disabled: u64,
    pub tariff_matched: u64,
    pub tariff_none: u64,
    pub several: u64,
    pub no_contact: u64,
    pub squads: Vec<SquadPlan>,
    pub remna_squads: Vec<RemnaSquad>,
    pub external_dropped: u64,
    pub secrets: bool,
    pub snapshots: u64,
    pub warnings: Vec<String>,
}

impl Plan {
    pub(crate) fn skip(&mut self, code: &str, text: &str, example: Option<String>) {
        let reason = match self.skipped.iter_mut().find(|r| r.code == code) {
            Some(reason) => reason,
            None => {
                self.skipped.push(Reason {
                    code: code.to_string(),
                    text: text.to_string(),
                    count: 0,
                    examples: Vec::new(),
                });
                self.skipped.last_mut().expect("только что добавлено")
            }
        };
        reason.count += 1;
        if let Some(example) = example
            && reason.examples.len() < EXAMPLES
        {
            reason.examples.push(example);
        }
    }

    pub fn skipped_total(&self) -> u64 {
        self.skipped.iter().map(|r| r.count).sum()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub total: u64,
    pub done: u64,
    pub created: u64,
    pub fixed: u64,
    pub relinked: u64,
    pub attached: u64,
    pub clients: u64,
    pub same: u64,
    pub differs: u64,
    pub skipped: u64,
    pub failed: u64,
    pub link_changed: u64,
    pub tariffs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOptions {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub squads: BTreeMap<Uuid, Option<Uuid>>,
    #[serde(default = "yes")]
    pub tariffs: bool,
}

impl Default for StartOptions {
    fn default() -> Self {
        StartOptions {
            mode: Mode::Create,
            squads: BTreeMap::new(),
            tariffs: true,
        }
    }
}

const fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncView {
    pub remna: bool,
    pub secrets: bool,
    pub run: Option<TransferView>,
    pub history: Vec<TransferView>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunEnd {
    Done(Progress),
    Paused(Progress),
    Stopped(Progress, String),
}

pub struct RemnaTransfer<'a> {
    services: &'a Services,
    pace: Pace,
}

impl Services {
    pub fn remna_transfer(&self) -> RemnaTransfer<'_> {
        RemnaTransfer {
            services: self,
            pace: Pace::default(),
        }
    }
}

fn actor(admin: Option<AdminId>) -> Actor {
    admin.map_or(Actor::System, Actor::Admin)
}

fn not_now(text: &str) -> ServiceError {
    ServiceError::rejected("remna_sync.state", text)
}

pub(crate) fn csv_cell(value: &str) -> String {
    let clean = value.replace(['\r', '\n'], " ");
    let clean = if clean.starts_with(['=', '+', '-', '@']) {
        format!("'{clean}")
    } else {
        clean
    };
    if clean.contains([';', '"']) {
        format!("\"{}\"", clean.replace('"', "\"\""))
    } else {
        clean
    }
}

fn outcome_line(direction: Direction, p: &Progress) -> String {
    let mut parts = Vec::new();
    match direction {
        Direction::Push => {
            parts.push(format!("создано {}", p.created));
            if p.fixed > 0 {
                parts.push(format!("исправлено {}", p.fixed));
            }
            if p.differs > 0 {
                parts.push(format!("с расхождениями оставлено {}", p.differs));
            }
        }
        Direction::Pull => {
            parts.push(format!("новых подписок {}", p.attached));
            if p.clients > 0 {
                parts.push(format!("из них с новыми клиентами {}", p.clients));
            }
        }
    }
    if p.relinked > 0 {
        parts.push(format!("привязано заново {}", p.relinked));
    }
    parts.push(format!("без изменений {}", p.same));
    if p.skipped > 0 {
        parts.push(format!("пропущено {}", p.skipped));
    }
    if p.failed > 0 {
        parts.push(format!("ошибок {}", p.failed));
    }
    parts.join(", ")
}

pub(crate) fn alert_text(
    direction: Direction,
    state: TransferState,
    outcome: Option<&Progress>,
    error: Option<&str>,
) -> String {
    let title = match (direction, state) {
        (_, TransferState::Done) => format!("<b>{} закончен</b>", direction.title()),
        (_, TransferState::Paused) => format!("<b>{} остановлен</b>", direction.title()),
        _ => format!("<b>{} прервался</b>", direction.title()),
    };
    let mut lines = vec![title];
    if let Some(outcome) = outcome {
        lines.push(outcome_line(direction, outcome));
        if outcome.link_changed > 0 {
            lines.push(format!(
                "У {} сменилась ссылка подписки — разошлите им новую на странице синхронизации",
                text::count(outcome.link_changed, "клиента", "клиентов", "клиентов")
            ));
        }
    }
    if let Some(error) = error.filter(|e| !e.trim().is_empty()) {
        lines.push(escape(error));
    }
    lines.join("\n")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl<'a> RemnaTransfer<'a> {
    pub fn with_pace(self, pace: Pace) -> Self {
        RemnaTransfer { pace, ..self }
    }

    pub async fn view(&self) -> ServiceResult<SyncView> {
        let mut conn = self.services.store().conn().await?;
        let run = runs::current(&mut conn).await?.map(|row| row.view());
        let history = runs::history(&mut conn, 10)
            .await?
            .iter()
            .map(runs::TransferRow::view)
            .collect();
        Ok(SyncView {
            remna: self.services.remna().is_ok(),
            secrets: self.services.secrets().is_some(),
            run,
            history,
        })
    }

    pub async fn get(&self, id: Uuid) -> ServiceResult<TransferView> {
        let mut conn = self.services.store().conn().await?;
        Ok(runs::get(&mut conn, id).await?.view())
    }

    pub async fn preview(&self, direction: Direction) -> ServiceResult<(Plan, Vec<NoteRow>)> {
        match direction {
            Direction::Push => push::plan(self.services, &self.pace).await,
            Direction::Pull => pull::plan(self.services, &self.pace).await,
        }
    }

    async fn checked(&self, row_id: i64, direction: Direction) -> ServiceResult<()> {
        let result = self.preview(direction).await;
        let mut conn = self.services.store().conn().await?;
        match result {
            Ok((plan, notes)) => {
                runs::clear_notes(&mut conn, row_id, "check").await?;
                runs::add_notes(&mut conn, row_id, &notes).await?;
                runs::set_plan(&mut conn, row_id, &plan).await?;
            }
            Err(err) => {
                let text = format!("Проверка не удалась: {err}");
                runs::set_state(&mut conn, row_id, TransferState::Failed, Some(&text)).await?;
            }
        }
        Ok(())
    }

    async fn open_check(&self, direction: Direction, admin: Option<AdminId>) -> ServiceResult<i64> {
        self.services.remna()?;
        let mut tx = self.services.store().begin().await?;
        sqlx::query("UPDATE remna_transfers SET state = 'discarded' WHERE state = 'ready'")
            .execute(tx.conn())
            .await?;
        let row = runs::create(&mut tx, direction, TransferState::Checking, None, admin).await?;
        tx.commit().await?;
        Ok(row.id)
    }

    pub async fn check_now(
        &self,
        direction: Direction,
        admin: Option<AdminId>,
    ) -> ServiceResult<TransferView> {
        let id = self.open_check(direction, admin).await?;
        self.checked(id, direction).await?;
        let mut conn = self.services.store().conn().await?;
        Ok(runs::get_by_id(&mut conn, id).await?.view())
    }

    pub async fn check(
        &self,
        direction: Direction,
        admin: Option<AdminId>,
    ) -> ServiceResult<TransferView> {
        let id = self.open_check(direction, admin).await?;
        let services = self.services.clone();
        let pace = self.pace;
        tokio::spawn(async move {
            let ops = services.remna_transfer().with_pace(pace);
            if let Err(err) = ops.checked(id, direction).await {
                tracing::warn!(error = %err, "проверка синхронизации с Remnawave не записалась");
            }
        });
        let mut conn = self.services.store().conn().await?;
        Ok(runs::get_by_id(&mut conn, id).await?.view())
    }

    async fn begin(
        &self,
        id: Uuid,
        options: &StartOptions,
        admin: Option<AdminId>,
    ) -> ServiceResult<runs::TransferRow> {
        self.services.remna()?;
        let mut tx = self.services.store().begin().await?;
        let row = runs::get(&mut tx, id).await?;
        if !row.state().resumable() || row.plan.is_none() {
            return Err(not_now(match row.state() {
                TransferState::Running => "Перенос уже идёт",
                TransferState::Checking => "Проверка ещё идёт",
                _ => "Эту проверку уже не запустить. Запустите новую",
            }));
        }
        let mode = match row.direction() {
            Direction::Push => options.mode,
            Direction::Pull => Mode::Create,
        };
        if !runs::begin(&mut tx, row.id, mode, admin).await? {
            return Err(runs::busy_error());
        }
        sqlx::query("UPDATE remna_transfers SET options = $2 WHERE id = $1")
            .bind(row.id)
            .bind(serde_json::to_value(options).map_err(|e| ServiceError::Internal(e.to_string()))?)
            .execute(tx.conn())
            .await?;
        tx.audit()
            .record(&actor(admin).audit(
                AUDIT_STARTED,
                AUDIT_ENTITY,
                Some(row.id),
                serde_json::json!({
                    "direction": row.direction().as_str(),
                    "mode": mode.as_str(),
                    "resumed": row.state() != TransferState::Ready,
                }),
            ))
            .await?;
        tx.commit().await?;
        let mut conn = self.services.store().conn().await?;
        runs::get_by_id(&mut conn, row.id).await
    }

    pub async fn start(
        &self,
        id: Uuid,
        options: StartOptions,
        admin: Option<AdminId>,
    ) -> ServiceResult<TransferView> {
        let row = self.begin(id, &options, admin).await?;
        let services = self.services.clone();
        let pace = self.pace;
        let row_id = row.id;
        tokio::spawn(async move {
            let ops = services.remna_transfer().with_pace(pace);
            if let Err(err) = ops.execute(row_id, admin).await {
                tracing::warn!(error = %err, "перенос между панелью и Remnawave остановился");
            }
        });
        Ok(row.view())
    }

    pub async fn start_now(
        &self,
        id: Uuid,
        options: StartOptions,
        admin: Option<AdminId>,
    ) -> ServiceResult<RunEnd> {
        let row = self.begin(id, &options, admin).await?;
        self.execute(row.id, admin).await
    }

    async fn execute(&self, row_id: i64, admin: Option<AdminId>) -> ServiceResult<RunEnd> {
        let (row, options) = {
            let mut conn = self.services.store().conn().await?;
            let row = runs::get_by_id(&mut conn, row_id).await?;
            let options: Option<serde_json::Value> =
                sqlx::query_scalar("SELECT options FROM remna_transfers WHERE id = $1")
                    .bind(row_id)
                    .fetch_optional(conn.conn())
                    .await?;
            let options: StartOptions = options
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            (row, options)
        };
        let ctl = Control {
            services: self.services,
            row_id,
            base: row.progress().unwrap_or_default(),
        };
        let result = match row.direction() {
            Direction::Push => {
                push::apply(
                    self.services,
                    &self.pace,
                    &ctl,
                    row.mode().unwrap_or_default(),
                    &options,
                    row.plan().unwrap_or_default(),
                )
                .await
            }
            Direction::Pull => pull::apply(self.services, &self.pace, &ctl).await,
        };
        let mut conn = self.services.store().conn().await?;
        let end = match result {
            Ok(end) => end,
            Err(err) => {
                let saved = runs::get_by_id(&mut conn, row_id)
                    .await?
                    .progress()
                    .unwrap_or_else(|| ctl.base());
                RunEnd::Stopped(saved, format!("Перенос остановился: {err}"))
            }
        };
        match &end {
            RunEnd::Done(progress) => runs::finish(&mut conn, row_id, progress).await?,
            RunEnd::Paused(progress) => {
                runs::set_progress(&mut conn, row_id, progress).await?;
                runs::set_state(&mut conn, row_id, TransferState::Paused, None).await?;
            }
            RunEnd::Stopped(progress, text) => {
                runs::set_progress(&mut conn, row_id, progress).await?;
                let text =
                    format!("{text}. Уже сделанное сохранено — продолжите позже, дублей не будет");
                runs::set_state(&mut conn, row_id, TransferState::Failed, Some(&text)).await?;
            }
        }
        let (state, progress) = match &end {
            RunEnd::Done(p) => (TransferState::Done, p),
            RunEnd::Paused(p) => (TransferState::Paused, p),
            RunEnd::Stopped(p, _) => (TransferState::Failed, p),
        };
        conn.audit()
            .record(&actor(admin).audit(
                AUDIT_FINISHED,
                AUDIT_ENTITY,
                Some(row_id),
                serde_json::json!({
                    "direction": row.direction().as_str(),
                    "state": state.as_str(),
                    "progress": progress,
                }),
            ))
            .await?;
        Ok(end)
    }

    pub async fn pause(&self, id: Uuid) -> ServiceResult<TransferView> {
        let mut conn = self.services.store().conn().await?;
        let row = runs::get(&mut conn, id).await?;
        if !runs::request_pause(&mut conn, row.id).await? {
            return Err(not_now("Перенос сейчас не идёт"));
        }
        Ok(runs::get_by_id(&mut conn, row.id).await?.view())
    }

    pub async fn discard(&self, id: Uuid) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let row = runs::get(&mut conn, id).await?;
        if !runs::discard(&mut conn, row.id).await? {
            return Err(not_now(
                "Сейчас идёт проверка или перенос — дождитесь окончания",
            ));
        }
        Ok(())
    }

    pub async fn recover(&self) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let stopped = runs::recover(&mut conn).await?;
        if stopped > 0 {
            tracing::warn!(
                runs = stopped,
                "синхронизация с Remnawave прервалась при перезапуске — её можно продолжить"
            );
        }
        Ok(())
    }

    pub async fn report(&self, id: Uuid) -> ServiceResult<(String, Vec<u8>)> {
        let mut conn = self.services.store().conn().await?;
        let row = runs::get(&mut conn, id).await?;
        let notes = runs::notes(&mut conn, row.id).await?;
        let mut csv = String::from(
            "\u{feff}Этап;Что;Подписка;Клиент;Пользователь Remnawave;Кто;Подробности\r\n",
        );
        for note in &notes {
            let line = [
                if note.stage == "apply" {
                    "перенос"
                } else {
                    "проверка"
                }
                .to_string(),
                note_word(&note.code).to_string(),
                note.subscription_id
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                note.client_id.map(|v| v.to_string()).unwrap_or_default(),
                note.remna_user_id
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                note.who.clone().unwrap_or_default(),
                note.details.clone(),
            ]
            .iter()
            .map(|cell| csv_cell(cell))
            .collect::<Vec<_>>()
            .join(";");
            csv.push_str(&line);
            csv.push_str("\r\n");
        }
        let name = format!(
            "remnawave-{}-{}.csv",
            row.direction().as_str(),
            row.public_id
                .simple()
                .to_string()
                .chars()
                .take(8)
                .collect::<String>()
        );
        Ok((name, csv.into_bytes()))
    }

    pub async fn notify_links(&self, id: Uuid, admin: Option<AdminId>) -> ServiceResult<u64> {
        let mut tx = self.services.store().begin().await?;
        let row = runs::get(&mut tx, id).await?;
        if row.state() != TransferState::Done {
            return Err(not_now("Разослать ссылки можно после окончания переноса"));
        }
        if !runs::mark_notified(&mut tx, row.id).await? {
            return Err(not_now("Новые ссылки уже разосланы"));
        }
        let notes = runs::link_notes(&mut tx, row.id).await?;
        let mut sent = 0u64;
        let mut seen = std::collections::HashSet::new();
        for note in notes {
            let (Some(client), Some(subscription)) = (note.client_id, note.subscription_id) else {
                continue;
            };
            let Some(url) = note
                .data
                .get("url")
                .and_then(|v| v.as_str())
                .filter(|u| !u.is_empty())
            else {
                continue;
            };
            if !seen.insert(subscription) {
                continue;
            }
            tx.jobs()
                .enqueue(
                    &JobPayload::Notify {
                        client_id: ClientId(client),
                        template: LINK_TEMPLATE.into(),
                        params: serde_json::json!({ "url": url, "subscription_id": subscription }),
                    },
                    None,
                )
                .await?;
            sent += 1;
        }
        tx.audit()
            .record(&actor(admin).audit(
                AUDIT_NOTIFIED,
                AUDIT_ENTITY,
                Some(row.id),
                serde_json::json!({ "clients": sent }),
            ))
            .await?;
        tx.commit().await?;
        Ok(sent)
    }
}

fn note_word(code: &str) -> &'static str {
    match code {
        "created" => "создан",
        "fixed" => "исправлен",
        "differs" => "есть расхождения",
        "relinked" => "привязан заново",
        "attached" => "новая подписка",
        "client_created" => "новый клиент",
        "link_changed" => "ссылка сменилась",
        "failed" => "ошибка",
        "later" => "срок в Remnawave позже",
        "tariff_none" => "без тарифа",
        _ => "пропущен",
    }
}

pub(crate) struct Control<'a> {
    services: &'a Services,
    row_id: i64,
    base: Progress,
}

impl Control<'_> {
    pub(crate) fn base(&self) -> Progress {
        self.base.clone()
    }

    pub(crate) async fn save(&self, progress: &Progress, notes: &[NoteRow]) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        if !notes.is_empty() {
            runs::add_notes(&mut conn, self.row_id, notes).await?;
        }
        runs::set_progress(&mut conn, self.row_id, progress).await
    }
}

pub(crate) fn merge_progress(base: &Progress, pass: &Progress) -> Progress {
    Progress {
        total: pass.total,
        done: pass.done,
        created: base.created + pass.created,
        fixed: base.fixed + pass.fixed,
        relinked: base.relinked + pass.relinked,
        attached: base.attached + pass.attached,
        clients: base.clients + pass.clients,
        same: pass.same,
        differs: pass.differs,
        skipped: pass.skipped,
        failed: pass.failed,
        link_changed: base.link_changed + pass.link_changed,
        tariffs: base.tariffs.max(pass.tariffs),
        notified_at: None,
    }
}
