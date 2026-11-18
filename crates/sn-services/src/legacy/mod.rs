pub mod apply;
pub mod archive;
pub mod bedolaga;
pub mod check;
pub mod chill;
pub mod chill_file;
pub mod extras;
pub mod invoices;
pub mod json_dump;
pub mod mail;
pub mod matcher;
pub mod minishop;
pub mod model;
pub mod pgcustom;
pub mod pgdump;
pub mod remnashop;
pub mod runs;
pub mod sqlite;
pub mod stealthnet;
pub mod table;
pub mod telegram_shop;

#[cfg(test)]
mod go_tests;
#[cfg(test)]
mod shop_tests;
#[cfg(test)]
mod tests;

use crate::Services;
use crate::bot_runtime::{BotProblem, BotState};
use crate::error::{ServiceError, ServiceResult};
use archive::OpenError;
use model::Legacy;
pub use model::Source;
use runs::{Outcome, Progress, RunState, RunView, Stage};
use serde::Serialize;
use sn_domain::ids::AdminId;
use sn_domain::money::Currency;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use table::{Dump, Table};
use uuid::Uuid;

pub const MAX_UPLOAD: u64 = 2 * 1024 * 1024 * 1024;
pub const IMPORT_DIR_ENV: &str = "SN_IMPORT_DIR";

pub fn wanted(table: &str) -> bool {
    stealthnet::TABLES.contains(&table)
        || bedolaga::TABLES.contains(&table)
        || telegram_shop::TABLES.contains(&table)
        || chill::TABLES.contains(&table)
        || remnashop::TABLES.contains(&table)
        || minishop::TABLES.contains(&table)
}

pub fn import_dir() -> PathBuf {
    std::env::var(IMPORT_DIR_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("stealthnet-import"))
}

pub fn map_dump(dump: &Dump) -> Result<Legacy, OpenError> {
    if stealthnet::looks_like(dump) {
        Ok(stealthnet::map(dump))
    } else if remnashop::looks_like(dump) {
        Ok(remnashop::map(dump))
    } else if minishop::looks_like(dump) {
        Ok(minishop::map(dump))
    } else if bedolaga::looks_like(dump) {
        Ok(bedolaga::map(dump))
    } else if telegram_shop::looks_like(dump) {
        Ok(telegram_shop::map(dump))
    } else if chill::looks_like(dump) {
        Ok(chill::map(dump))
    } else {
        Err(OpenError::Unsupported(
            "В файле нет клиентов ни одного из ботов, из которых умеет переносить панель. Нужна резервная копия базы старого бота".into(),
        ))
    }
}

pub fn read_file(
    path: &Path,
    password: Option<&str>,
) -> Result<(Legacy, Option<PathBuf>), OpenError> {
    let mut extracted = archive::extract_zip(path, password)?;
    let inner = path.with_extension("inner");
    if chill_file::unseal(extracted.as_deref().unwrap_or(path), password, &inner)? {
        extracted = Some(inner);
    }
    let target = extracted.as_deref().unwrap_or(path);
    let dump = archive::open(target, None, &wanted)?;
    Ok((map_dump(&dump)?, extracted))
}

pub async fn read_database(url: &str) -> Result<Legacy, String> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(url)
        .await
        .map_err(|e| format!("старая база не подключилась: {e}"))?;
    let mut dump = Dump {
        format: "database",
        ..Dump::default()
    };
    for name in stealthnet::TABLES {
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('public.' || quote_ident($1)) IS NOT NULL")
                .bind(name)
                .fetch_one(&pool)
                .await
                .map_err(|e| format!("старая база не читается: {e}"))?;
        if !exists {
            continue;
        }
        let rows: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT row_to_json(t)::text FROM public.\"{name}\" t"
        ))
        .fetch_all(&pool)
        .await
        .map_err(|e| format!("таблица {name} не читается: {e}"))?;
        let mut table = Table::default();
        for row in rows {
            let Ok(serde_json::Value::Object(map)) =
                serde_json::from_str::<serde_json::Value>(&row)
            else {
                continue;
            };
            table.push_named(map.into_iter().map(|(k, v)| {
                let text = match v {
                    serde_json::Value::Null => None,
                    serde_json::Value::Bool(true) => Some("t".to_string()),
                    serde_json::Value::Bool(false) => Some("f".to_string()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    serde_json::Value::String(s) => Some(s),
                    other => Some(other.to_string()),
                };
                (k, text)
            }));
        }
        dump.tables.insert(name.to_string(), table);
    }
    pool.close().await;
    if !stealthnet::looks_like(&dump) {
        return Err("в базе нет таблиц STEALTHNET 7.x".into());
    }
    Ok(stealthnet::map(&dump))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BotLink {
    Ok,
    Conflict,
    Webhook,
    NoToken,
    Unauthorized,
    Offline,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportView {
    pub remna: bool,
    pub currency: Currency,
    pub clients: i64,
    pub bot: BotLink,
    pub run: Option<RunView>,
    pub history: Vec<RunView>,
}

pub struct LegacyOps<'a> {
    services: &'a Services,
}

fn bot_link(state: &BotState) -> BotLink {
    match state {
        BotState::Running { .. } | BotState::Starting => BotLink::Ok,
        BotState::Off | BotState::NoToken => BotLink::NoToken,
        BotState::Failed { problem, .. } => match problem {
            BotProblem::Conflict => BotLink::Conflict,
            BotProblem::Webhook => BotLink::Webhook,
            BotProblem::BadToken => BotLink::Unauthorized,
            _ => BotLink::Offline,
        },
    }
}

fn remove(path: Option<&Path>) {
    if let Some(path) = path {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("inner"));
    }
}

fn open_message(err: &OpenError) -> String {
    match err {
        OpenError::NeedsPassword => "Архив защищён паролем".into(),
        OpenError::WrongPassword => "Пароль не подошёл".into(),
        OpenError::Unsupported(text) | OpenError::Io(text) => text.clone(),
    }
}

async fn blocking_read(
    path: PathBuf,
    password: Option<String>,
) -> Result<(Legacy, Option<PathBuf>), OpenError> {
    tokio::task::spawn_blocking(move || read_file(&path, password.as_deref()))
        .await
        .map_err(|e| OpenError::Io(format!("чтение файла прервалось: {e}")))?
}

pub type Notify = Arc<dyn Fn(&Progress) + Send + Sync>;

impl Services {
    pub fn legacy(&self) -> LegacyOps<'_> {
        LegacyOps { services: self }
    }
}

impl LegacyOps<'_> {
    pub async fn view(&self) -> ServiceResult<ImportView> {
        let mut conn = self.services.store().conn().await?;
        let run = runs::current(&mut conn).await?.map(|(_, view)| view);
        let history = runs::history(&mut conn, 10).await?;
        let clients: i64 =
            sqlx::query_scalar("SELECT count(*) FROM clients WHERE deleted_at IS NULL")
                .fetch_one(sn_store::Db::conn(&mut conn))
                .await?;
        Ok(ImportView {
            remna: self.services.remna().is_ok(),
            currency: self.services.settings().currency,
            clients,
            bot: bot_link(&self.services.bot_runtime().state()),
            run,
            history,
        })
    }

    pub async fn run(&self, id: Uuid) -> ServiceResult<RunView> {
        let mut conn = self.services.store().conn().await?;
        Ok(runs::get(&mut conn, id).await?.1)
    }

    pub async fn busy(&self) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        Ok(runs::running(&mut conn).await?.is_some())
    }

    pub async fn uploaded(
        &self,
        source: Source,
        file_name: &str,
        size: u64,
        path: &Path,
        admin: Option<AdminId>,
    ) -> ServiceResult<RunView> {
        let mut tx = self.services.store().begin().await?;
        if runs::running(&mut tx).await?.is_some() {
            drop(tx);
            remove(Some(path));
            return Err(ServiceError::rejected(
                "import.busy",
                "Перенос уже идёт. Дождитесь окончания и загрузите файл снова",
            ));
        }
        let stale = runs::discard_open(&mut tx).await?;
        let name: String = file_name.chars().take(200).collect();
        let row = runs::create(
            &mut tx,
            source,
            if name.trim().is_empty() {
                "выгрузка"
            } else {
                name.trim()
            },
            i64::try_from(size).unwrap_or(i64::MAX),
            path,
            admin,
        )
        .await?;
        tx.commit().await?;
        for old in stale {
            remove(Some(&old));
        }
        self.spawn_check(row.id, None);
        let mut conn = self.services.store().conn().await?;
        Ok(runs::get_by_id(&mut conn, row.id).await?.1)
    }

    pub async fn password(&self, id: Uuid, password: String) -> ServiceResult<RunView> {
        let mut conn = self.services.store().conn().await?;
        let (row, _) = runs::get(&mut conn, id).await?;
        if row.state != RunState::NeedsPassword {
            return Err(ServiceError::rejected(
                "import.state",
                "Пароль сейчас не нужен",
            ));
        }
        runs::set_state(&mut conn, row.id, RunState::Checking, None).await?;
        self.spawn_check(row.id, Some(password));
        Ok(runs::get_by_id(&mut conn, row.id).await?.1)
    }

    pub async fn recheck(&self, id: Uuid) -> ServiceResult<RunView> {
        let mut conn = self.services.store().conn().await?;
        let (row, _) = runs::get(&mut conn, id).await?;
        let allowed = matches!(
            row.state,
            RunState::Ready | RunState::Interrupted | RunState::Failed
        );
        if !allowed || row.file_path.is_none() {
            return Err(ServiceError::rejected(
                "import.state",
                "Файла уже нет. Загрузите выгрузку заново",
            ));
        }
        runs::set_state(&mut conn, row.id, RunState::Checking, None).await?;
        self.spawn_check(row.id, None);
        Ok(runs::get_by_id(&mut conn, row.id).await?.1)
    }

    pub async fn discard(&self, id: Uuid) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let (row, _) = runs::get(&mut conn, id).await?;
        if matches!(row.state, RunState::Running | RunState::Checking) {
            return Err(ServiceError::rejected(
                "import.busy",
                "Файл ещё обрабатывается — дождитесь окончания",
            ));
        }
        sqlx::query("UPDATE import_runs SET state = 'discarded', file_path = NULL WHERE id = $1 AND state <> 'done'")
            .bind(row.id)
            .execute(sn_store::Db::conn(&mut conn))
            .await?;
        remove(row.file_path.as_deref());
        Ok(())
    }

    pub async fn start(&self, id: Uuid, admin: Option<AdminId>) -> ServiceResult<RunView> {
        self.start_with(id, admin, runs::StartOptions::default())
            .await
    }

    pub async fn start_with(
        &self,
        id: Uuid,
        admin: Option<AdminId>,
        options: runs::StartOptions,
    ) -> ServiceResult<RunView> {
        let mut conn = self.services.store().conn().await?;
        let (row, _) = runs::get(&mut conn, id).await?;
        let allowed = matches!(row.state, RunState::Ready | RunState::Interrupted)
            || (row.state == RunState::Failed && row.preview.is_some());
        if !allowed || row.file_path.is_none() {
            return Err(ServiceError::rejected(
                "import.state",
                "Этот файл уже нельзя перенести. Загрузите выгрузку заново",
            ));
        }
        if self.services.remna().is_err() {
            return Err(ServiceError::rejected(
                "import.remna_required",
                "Сначала подключите Remnawave в «Настройки → Remnawave» — ту же панель, что у старого бота",
            ));
        }
        let switched = sqlx::query(
            "UPDATE import_runs SET state = 'running', error = NULL, started_at = now(), finished_at = NULL, progress = NULL, \
             options = $2 WHERE id = $1 AND state IN ('ready', 'interrupted', 'failed')",
        )
        .bind(row.id)
        .bind(serde_json::to_value(options).map_err(|e| ServiceError::Internal(e.to_string()))?)
        .execute(sn_store::Db::conn(&mut conn))
        .await;
        match switched {
            Ok(done) if done.rows_affected() == 1 => {}
            Ok(_) => {
                return Err(ServiceError::rejected("import.busy", "Перенос уже запущен"));
            }
            Err(err)
                if err.as_database_error().and_then(|d| d.constraint())
                    == Some("import_runs_running_key") =>
            {
                return Err(ServiceError::rejected(
                    "import.busy",
                    "Уже идёт другой перенос. Дождитесь окончания",
                ));
            }
            Err(err) => return Err(err.into()),
        }
        let services = self.services.clone();
        let run_id = row.id;
        let run_key = row.public_id;
        let path = row.file_path.clone();
        tokio::spawn(async move {
            let result = execute(&services, run_id, run_key, path.clone(), admin, None).await;
            let mut conn = match services.store().conn().await {
                Ok(conn) => conn,
                Err(err) => {
                    tracing::error!(error = %err, "перенос: нет соединения с базой");
                    return;
                }
            };
            match result {
                Ok(outcome) => {
                    if let Err(err) = runs::finish(&mut conn, run_id, &outcome).await {
                        tracing::error!(error = %err, "перенос: итог не записан");
                    }
                    remove(path.as_deref());
                    tracing::info!(clients = outcome.counts.clients, "перенос завершён");
                }
                Err(err) => {
                    tracing::warn!(error = %err, "перенос остановился");
                    let text = format!(
                        "Перенос остановился: {err}. Уже перенесённое сохранено — запустите ещё раз, дублей не будет"
                    );
                    let _ = runs::set_state(&mut conn, run_id, RunState::Failed, Some(&text)).await;
                }
            }
        });
        Ok(runs::get_by_id(&mut conn, row.id).await?.1)
    }

    pub async fn report(&self, id: Uuid) -> ServiceResult<(String, Vec<u8>)> {
        let mut conn = self.services.store().conn().await?;
        let (row, _) = runs::get(&mut conn, id).await?;
        let issues = runs::issues(&mut conn, row.id).await?;
        let mut csv =
            String::from("\u{feff}Этап;Раздел;Номер в старой системе;Кто;Что случилось\r\n");
        for issue in &issues {
            let line = [
                if issue.stage == "apply" {
                    "перенос"
                } else {
                    "проверка"
                },
                entity_word(&issue.entity),
                issue.old_id.as_deref().unwrap_or(""),
                issue.who.as_deref().unwrap_or(""),
                issue.details.as_str(),
            ]
            .iter()
            .map(|cell| csv_cell(cell))
            .collect::<Vec<_>>()
            .join(";");
            csv.push_str(&line);
            csv.push_str("\r\n");
        }
        let name = format!(
            "perenos-{}-{}.csv",
            row.source.as_str(),
            row.public_id
                .simple()
                .to_string()
                .chars()
                .take(8)
                .collect::<String>()
        );
        Ok((name, csv.into_bytes()))
    }

    pub async fn recover(&self) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let interrupted = runs::recover(&mut conn).await?;
        if interrupted > 0 {
            tracing::warn!(
                runs = interrupted,
                "перенос прервался при перезапуске — его можно запустить снова"
            );
        }
        let keep: Vec<String> =
            sqlx::query_scalar("SELECT file_path FROM import_runs WHERE file_path IS NOT NULL")
                .fetch_all(sn_store::Db::conn(&mut conn))
                .await?;
        let keep: HashSet<PathBuf> = keep.into_iter().map(PathBuf::from).collect();
        if let Ok(entries) = std::fs::read_dir(import_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                let base = path.with_extension("upload");
                if !keep.contains(&path) && !keep.contains(&base) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        Ok(())
    }

    fn spawn_check(&self, run_id: i64, password: Option<String>) {
        let services = self.services.clone();
        tokio::spawn(async move {
            if let Err(err) = check_run(&services, run_id, password).await {
                tracing::warn!(error = %err, "проверка выгрузки не удалась");
                if let Ok(mut conn) = services.store().conn().await {
                    let text = format!("Проверка не удалась: {err}");
                    let _ = runs::set_state(&mut conn, run_id, RunState::Failed, Some(&text)).await;
                }
            }
        });
    }

    pub async fn run_cli(
        &self,
        legacy: Legacy,
        file_name: &str,
        dry_run: bool,
        notify: Notify,
        out: &dyn Fn(&str),
    ) -> ServiceResult<Option<Outcome>> {
        let remna = check::fetch_remna(self.services).await;
        let remna_error = remna.as_ref().err().cloned();
        let checked = check::preview(self.services, &legacy, remna).await?;
        print_preview(&checked.preview, out);
        for issue in checked.issues.iter().take(30) {
            out(&format!(
                "  · {}{}",
                issue
                    .who
                    .as_ref()
                    .map(|w| format!("{w}: "))
                    .unwrap_or_default(),
                issue.details
            ));
        }
        if checked.issues.len() > 30 {
            out(&format!("  … и ещё {}", checked.issues.len() - 30));
        }
        if dry_run {
            out("Проверка без записи: в базу ничего не записано.");
            return Ok(None);
        }
        if let Some(err) = remna_error {
            return Err(ServiceError::rejected(
                "import.remna",
                format!("Remnawave не ответила: {err}"),
            ));
        }
        if !checked.preview.remna.connected {
            return Err(ServiceError::rejected(
                "import.remna_required",
                "Remnawave не подключена: задайте её в «Настройки → Remnawave»",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let id: (i64, Uuid) = sqlx::query_as(
            "INSERT INTO import_runs (source, state, file_name, file_size, preview, started_at) \
             VALUES ($1, 'running', $2, 0, $3, now()) RETURNING id, public_id",
        )
        .bind(legacy.source.as_str())
        .bind(file_name)
        .bind(
            serde_json::to_value(&checked.preview)
                .map_err(|e| ServiceError::Internal(e.to_string()))?,
        )
        .fetch_one(sn_store::Db::conn(&mut tx))
        .await
        .map_err(
            |err| match err.as_database_error().and_then(|d| d.constraint()) {
                Some("import_runs_running_key") => {
                    ServiceError::rejected("import.busy", "уже идёт другой перенос")
                }
                _ => ServiceError::from(sn_store::StoreError::from(err)),
            },
        )?;
        runs::add_issues(&mut tx, id.0, &checked.issues).await?;
        tx.commit().await?;
        let admin = owner(self.services).await?;
        let result = apply_legacy(self.services, &legacy, id.0, id.1, admin, Some(notify)).await;
        let mut conn = self.services.store().conn().await?;
        match result {
            Ok(outcome) => {
                runs::finish(&mut conn, id.0, &outcome).await?;
                Ok(Some(outcome))
            }
            Err(err) => {
                let text = format!("Перенос остановился: {err}");
                runs::set_state(&mut conn, id.0, RunState::Failed, Some(&text)).await?;
                Err(err)
            }
        }
    }
}

async fn owner(services: &Services) -> ServiceResult<Option<AdminId>> {
    let mut conn = services.store().conn().await?;
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM admins WHERE role = 'owner' AND disabled_at IS NULL ORDER BY id LIMIT 1",
    )
    .fetch_optional(sn_store::Db::conn(&mut conn))
    .await?;
    Ok(id.map(AdminId))
}

fn entity_word(entity: &str) -> &'static str {
    match entity {
        "client" => "Клиент",
        "subscription" => "Подписка",
        "tariff" => "Тариф",
        "promo" => "Промокод",
        "payment" => "Платёж",
        "ticket" => "Обращение",
        _ => "Прочее",
    }
}

fn csv_cell(value: &str) -> String {
    let clean = value.replace(['\r', '\n'], " ");
    let risky = clean.starts_with(['=', '+', '-', '@']);
    let clean = if risky { format!("'{clean}") } else { clean };
    if clean.contains([';', '"']) {
        format!("\"{}\"", clean.replace('"', "\"\""))
    } else {
        clean
    }
}

pub fn wrong_panel(preview: &runs::Preview) -> Option<String> {
    let remna = &preview.remna;
    if !remna.connected || remna.found > 0 || preview.counts.subscriptions == 0 {
        return None;
    }
    Some(if remna.users == 0 {
        "В подключённой Remnawave нет ни одного пользователя — похоже, это не та панель, что у старого бота. Проверьте «Настройки → Remnawave»: если перенести сейчас, клиенты перейдут без подписок.".to_string()
    } else {
        format!(
            "Ни одна подписка не нашлась в подключённой Remnawave (пользователей в ней: {}) — похоже, это не та панель, что у старого бота. Проверьте «Настройки → Remnawave»: если перенести сейчас, клиенты перейдут без подписок.",
            remna.users
        )
    })
}

fn print_preview(preview: &runs::Preview, out: &dyn Fn(&str)) {
    let c = &preview.counts;
    out(&format!(
        "Источник: {}. Клиентов {}, подписок {}, тарифов {}, промокодов {}, рефералов {}, платежей {}, обращений {}.",
        preview.detected.title(),
        c.clients,
        c.subscriptions,
        c.tariffs,
        c.promo_codes,
        c.referrals,
        c.payments,
        c.tickets
    ));
    out(&format!(
        "Балансы: {} у {} клиентов ({}).",
        stealthnet::format_minor(preview.balances.total_minor),
        preview.balances.clients,
        preview.balances.currency.code()
    ));
    if let Some(from) = preview.balances.large_from_minor {
        out(&format!(
            "Проверьте до запуска: у {} клиентов баланс больше {} — {}, всего {}. Если это ошибка, исправьте балансы в старой панели и перенесите заново, иначе они перенесутся как есть. Список — в отчёте.",
            preview.balances.large_clients,
            stealthnet::format_minor(from),
            if preview.balances.large_by_tariff {
                "в десять раз дороже самого дорогого тарифа"
            } else {
                "в десять раз больше, чем у 99% клиентов"
            },
            stealthnet::format_minor(preview.balances.large_total_minor)
        ));
    }
    if preview.remna.connected {
        out(&format!(
            "Remnawave: нашлись {} подписок из {}.",
            preview.remna.found, c.subscriptions
        ));
        if let Some(text) = wrong_panel(preview) {
            out(&text);
        }
    } else {
        out("Remnawave не подключена — подписки не проверены.");
    }
    let extras: Vec<String> = [
        (c.admins, "админов"),
        (c.gifts, "подарочных кодов"),
        (c.withdrawals, "заявок на вывод"),
        (c.auto_rules, "авторассылок (выключенными)"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{what} {n}"))
    .collect();
    if c.settings > 0 || !extras.is_empty() {
        let mut parts = Vec::new();
        if c.settings > 0 {
            parts.push("настройки бота и ключи платёжек (при первом переносе)".to_string());
        }
        parts.extend(extras);
        out(&format!("Ещё перенесутся: {}.", parts.join(", ")));
    }
    match &preview.mail {
        Some(mail::MailPlan::Transfer { incoming }) => out(&format!(
            "Почта: перенесётся отправка писем — {incoming}, модуль «Почта» включится."
        )),
        Some(mail::MailPlan::Partial { incoming, reason }) => out(&format!(
            "Почта: в старой панели письма не отправлялись ({reason}). Перенесётся заполненное — {incoming}, модуль «Почта» не включится."
        )),
        Some(mail::MailPlan::Same { .. }) => out("Почта: в этой установке уже те же настройки."),
        Some(mail::MailPlan::Differs { current, incoming }) => out(&format!(
            "Почта: в этой установке уже настроена — {current}, в старой панели — {incoming}. Из командной строки почта не заменяется: \
             чтобы взять старые настройки, перенесите через мастер в админке с отметкой «Заменить настройками старой панели»."
        )),
        Some(mail::MailPlan::Skipped { reason }) => {
            out(&format!("Почта не перенесётся: {reason}."))
        }
        None => {}
    }
    if preview.currency.switch {
        out(&format!(
            "Валюта установки станет {}.",
            preview.currency.source.code()
        ));
    }
    if let Some(text) = &preview.currency.blocked {
        out(text);
    }
    if preview.existing.matched > 0 {
        out(&format!(
            "Совпали с клиентами этой установки по Telegram или почте: {}.",
            preview.existing.matched
        ));
    }
    if preview.existing.again > 0 {
        out(&format!(
            "Уже переносились раньше: {} — обновятся.",
            preview.existing.again
        ));
    }
    for skip in &preview.skipped {
        out(&format!("Не перенесётся: {} {}", skip.count, skip.text));
    }
}

async fn check_run(
    services: &Services,
    run_id: i64,
    password: Option<String>,
) -> ServiceResult<()> {
    let mut conn = services.store().conn().await?;
    let (row, _) = runs::get_by_id(&mut conn, run_id).await?;
    drop(conn);
    let Some(path) = row.file_path.clone() else {
        return Err(ServiceError::rejected("import.no_file", "файла нет"));
    };
    let read = blocking_read(path.clone(), password).await;
    let mut conn = services.store().conn().await?;
    let (legacy, extracted) = match read {
        Ok(read) => read,
        Err(err @ (OpenError::NeedsPassword | OpenError::WrongPassword)) => {
            let text = (err == OpenError::WrongPassword).then(|| open_message(&err));
            runs::set_state(&mut conn, run_id, RunState::NeedsPassword, text.as_deref()).await?;
            return Ok(());
        }
        Err(err) => {
            runs::set_state(
                &mut conn,
                run_id,
                RunState::Failed,
                Some(&open_message(&err)),
            )
            .await?;
            runs::set_file(&mut conn, run_id, None).await?;
            remove(Some(&path));
            return Ok(());
        }
    };
    if let Some(inner) = &extracted {
        runs::set_file(&mut conn, run_id, Some(inner)).await?;
        let _ = std::fs::remove_file(&path);
    }
    if legacy.source != row.source {
        runs::set_source(&mut conn, run_id, legacy.source).await?;
    }
    drop(conn);
    let remna = check::fetch_remna(services).await;
    let checked = check::preview(services, &legacy, remna).await?;
    let mut tx = services.store().begin().await?;
    runs::clear_issues(&mut tx, run_id, "check").await?;
    runs::add_issues(&mut tx, run_id, &checked.issues).await?;
    runs::set_preview(&mut tx, run_id, &checked.preview).await?;
    tx.commit().await?;
    Ok(())
}

async fn execute(
    services: &Services,
    run_id: i64,
    run_key: Uuid,
    path: Option<PathBuf>,
    admin: Option<AdminId>,
    notify: Option<Notify>,
) -> ServiceResult<Outcome> {
    {
        let mut conn = services.store().conn().await?;
        runs::set_progress(
            &mut conn,
            run_id,
            &Progress {
                stage: Stage::Read,
                done: 0,
                total: 1,
            },
        )
        .await?;
    }
    let Some(path) = path else {
        return Err(ServiceError::rejected(
            "import.no_file",
            "файла выгрузки уже нет",
        ));
    };
    let (legacy, _) = blocking_read(path, None)
        .await
        .map_err(|err| ServiceError::rejected("import.read", open_message(&err)))?;
    apply_legacy(services, &legacy, run_id, run_key, admin, notify).await
}

pub async fn apply_legacy(
    services: &Services,
    legacy: &Legacy,
    run_id: i64,
    run_key: Uuid,
    admin: Option<AdminId>,
    notify: Option<Notify>,
) -> ServiceResult<Outcome> {
    let index = match check::fetch_remna(services).await {
        Ok(Some(index)) => index,
        Ok(None) => {
            return Err(ServiceError::rejected(
                "import.remna_required",
                "Remnawave не подключена",
            ));
        }
        Err(err) => {
            return Err(ServiceError::rejected(
                "import.remna",
                format!("Remnawave не ответила: {err}"),
            ));
        }
    };
    let matched = matcher::match_all(legacy, &index, &HashSet::new());
    let plan = check_currency(services, legacy.currency).await?;
    let applier = apply::Applier::new(
        services, legacy, &index, matched, run_id, run_key, admin, plan, notify,
    );
    applier.run().await
}

async fn check_currency(services: &Services, source: Currency) -> ServiceResult<bool> {
    let install = services.settings().currency;
    if install == source {
        return Ok(false);
    }
    let mut conn = services.store().conn().await?;
    let busy: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM payments WHERE status IN ('paid', 'refunded', 'partially_refunded') \
             AND NOT (provider_payload ? 'imported')) \
         OR EXISTS (SELECT 1 FROM client_balances WHERE currency = $1 AND amount_minor > 0 AND wallet = 'main')",
    )
    .bind(install.code())
    .fetch_one(sn_store::Db::conn(&mut conn))
    .await?;
    Ok(!busy)
}
