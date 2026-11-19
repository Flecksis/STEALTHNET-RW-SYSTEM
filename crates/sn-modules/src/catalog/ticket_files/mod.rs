mod kind;
mod strip;
#[cfg(test)]
mod tests;

pub use kind::{
    ALLOWED_TEXT, FileType, GIF, JPEG, PDF, PNG, Refusal, TEXT, TYPES, WEBP, allowed_name,
    by_extension, by_mime, detect, file_name, sniff,
};
pub use strip::{Cleaned, clean};

use crate::catalog::config_number;
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{Module, ModuleConfig, ScheduledTask};
use async_trait::async_trait;
use serde::Serialize;
use sn_domain::module::ModuleId;
use sn_domain::settings::SettingField;
use sn_store::Db;
use std::time::Duration;

pub const MB: i64 = 1024 * 1024;
pub const GB: i64 = 1024 * MB;
pub const DEFAULT_MAX_MB: i64 = 10;
pub const MAX_MB: i64 = 20;
pub const DEFAULT_MAX_FILES: i64 = 5;
pub const MAX_FILES: i64 = 10;
pub const DEFAULT_CLIENT_MB: i64 = 100;
pub const MAX_CLIENT_MB: i64 = 10_000;
pub const DEFAULT_TOTAL_GB: i64 = 5;
pub const MAX_TOTAL_GB: i64 = 1_000;
pub const DEFAULT_KEEP_DAYS: i64 = 180;
pub const MAX_KEEP_DAYS: i64 = 3650;
pub const TELEGRAM_DOWNLOAD_LIMIT: i64 = 20 * MB;
pub const PENDING_HOURS: i64 = 24;
pub const PURGE_TASK: &str = "purge";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub max_bytes: i64,
    pub max_files: usize,
    #[serde(skip)]
    pub client_bytes: i64,
    #[serde(skip)]
    pub total_bytes: i64,
    #[serde(skip)]
    pub keep_days: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bytes: DEFAULT_MAX_MB * MB,
            max_files: DEFAULT_MAX_FILES as usize,
            client_bytes: DEFAULT_CLIENT_MB * MB,
            total_bytes: DEFAULT_TOTAL_GB * GB,
            keep_days: DEFAULT_KEEP_DAYS,
        }
    }
}

fn bounded(
    config: &ModuleConfig,
    key: &str,
    default: i64,
    range: (i64, i64),
    label: &str,
) -> Result<i64, String> {
    let value = match config.get(key) {
        None | Some(serde_json::Value::Null) => return Ok(default),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => return Ok(default),
        Some(_) => {
            config_number(config, key).ok_or_else(|| format!("{label}: нужно целое число"))?
        }
    };
    if value < range.0 || value > range.1 {
        return Err(format!("{label}: от {} до {}", range.0, range.1));
    }
    Ok(value)
}

impl Limits {
    pub fn parse(config: &ModuleConfig) -> Result<Limits, String> {
        let max_mb = bounded(
            config,
            "max_mb",
            DEFAULT_MAX_MB,
            (1, MAX_MB),
            "Размер файла",
        )?;
        let max_files = bounded(
            config,
            "max_files",
            DEFAULT_MAX_FILES,
            (1, MAX_FILES),
            "Файлов в сообщении",
        )?;
        let client_mb = bounded(
            config,
            "client_mb",
            DEFAULT_CLIENT_MB,
            (1, MAX_CLIENT_MB),
            "Место для файлов одного клиента",
        )?;
        let total_gb = bounded(
            config,
            "total_gb",
            DEFAULT_TOTAL_GB,
            (1, MAX_TOTAL_GB),
            "Место для всех файлов",
        )?;
        let keep_days = bounded(
            config,
            "keep_days",
            DEFAULT_KEEP_DAYS,
            (0, MAX_KEEP_DAYS),
            "Срок хранения",
        )?;
        Ok(Limits {
            max_bytes: max_mb * MB,
            max_files: usize::try_from(max_files).unwrap_or(1),
            client_bytes: client_mb * MB,
            total_bytes: total_gb * GB,
            keep_days,
        })
    }

    pub fn of(config: &ModuleConfig) -> Limits {
        Limits::parse(config).unwrap_or_default()
    }

    pub const fn max_mb(&self) -> i64 {
        self.max_bytes / MB
    }

    pub const fn daily_files(&self) -> i64 {
        self.max_files as i64 * 20
    }
}

pub struct TicketFilesModule;

#[async_trait]
impl Module for TicketFilesModule {
    fn id(&self) -> ModuleId {
        ModuleId::TicketFiles
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::number(
                "max_mb",
                "Размер одного файла, МБ",
                "От 1 до 20. Бот Telegram не скачивает файлы больше 20 МБ",
            )
            .with_default("10"),
            SettingField::number(
                "max_files",
                "Файлов в одном сообщении",
                "От 1 до 10",
            )
            .with_default("5"),
            SettingField::number(
                "client_mb",
                "Место для файлов одного клиента, МБ",
                "От 1 до 10000. Когда файлы клиента займут столько, новые от него не принимаются, пока старые не удалятся по сроку хранения",
            )
            .with_default("100"),
            SettingField::number(
                "total_gb",
                "Место для всех файлов обращений, ГБ",
                "От 1 до 1000. Когда файлы займут столько, клиенты не смогут прикладывать новые, пока старые не удалятся. Файлы поддержки принимаются",
            )
            .with_default("5"),
            SettingField::number(
                "keep_days",
                "Хранить файлы закрытых обращений, дней",
                "0 — хранить бессрочно. Через столько дней после закрытия обращения его файлы удаляются, переписка остаётся",
            )
            .with_default("180"),
        ]
    }

    fn validate_config(&self, config: &ModuleConfig) -> Result<(), ModuleError> {
        Limits::parse(config)
            .map(|_| ())
            .map_err(ModuleError::Invalid)
    }

    fn cleanup_tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: PURGE_TASK,
            every: Duration::from_secs(3600),
        }]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        if task != PURGE_TASK {
            return Err(ModuleError::Invalid(format!("у модуля нет задачи {task}")));
        }
        let limits = Limits::of(&ctx.core.module_config(ModuleId::TicketFiles));
        let mut conn = ctx.store.conn().await?;
        let pending = conn.ticket_files().purge_pending(PENDING_HOURS).await?;
        let expired = if limits.keep_days > 0 {
            conn.ticket_files().purge_closed(limits.keep_days).await?
        } else {
            0
        };
        if pending > 0 || expired > 0 {
            tracing::info!(pending, expired, "файлы обращений удалены");
        }
        Ok(())
    }
}
