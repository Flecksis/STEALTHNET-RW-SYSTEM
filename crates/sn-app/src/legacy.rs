use anyhow::{Context, Result, bail};
use sn_services::Services;
use sn_services::legacy::runs::{Progress, Stage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub const LEGACY_URL: &str = "LEGACY_DATABASE_URL";
pub const ARCHIVE_PASSWORD: &str = "LEGACY_ARCHIVE_PASSWORD";

pub fn source_url(vars: &HashMap<String, String>) -> Result<String> {
    let Some(url) = vars
        .get(LEGACY_URL)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    else {
        bail!(
            "укажите файл резервной копии (stealthnet import-legacy <файл>) или задайте {LEGACY_URL} — строку подключения к базе STEALTHNET 7.x"
        );
    };
    if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
        bail!("{LEGACY_URL} должна начинаться с postgres://");
    }
    Ok(url.to_string())
}

fn stage_word(stage: Stage) -> &'static str {
    match stage {
        Stage::Read => "чтение файла",
        Stage::Tariffs => "тарифы",
        Stage::Clients => "клиенты и балансы",
        Stage::Subscriptions => "подписки",
        Stage::Payments => "платежи",
        Stage::Promo => "промокоды",
        Stage::Referrals => "рефералы",
        Stage::Tickets => "обращения",
        Stage::Finish => "сверка",
    }
}

pub async fn import(
    services: &Services,
    vars: &HashMap<String, String>,
    dry_run: bool,
    file: Option<&str>,
) -> Result<()> {
    let (legacy, name, extracted) = match file {
        Some(file) => {
            let path = PathBuf::from(file);
            if !path.is_file() {
                bail!("файл {file} не найден");
            }
            let password = vars
                .get(ARCHIVE_PASSWORD)
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty());
            let read = path.clone();
            let (legacy, extracted) = tokio::task::spawn_blocking(move || {
                sn_services::legacy::read_file(&read, password.as_deref())
            })
            .await
            .context("чтение файла прервалось")?
            .map_err(|err| match err {
                sn_services::legacy::archive::OpenError::NeedsPassword => {
                    anyhow::anyhow!("архив защищён паролем: задайте его в {ARCHIVE_PASSWORD}")
                }
                other => anyhow::anyhow!("{other}"),
            })?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| file.to_string());
            (legacy, name, extracted)
        }
        None => {
            let url = source_url(vars)?;
            let legacy = sn_services::legacy::read_database(&url)
                .await
                .map_err(|err| anyhow::anyhow!(err))?;
            (legacy, "база STEALTHNET 7.x".to_string(), None)
        }
    };
    let notify: sn_services::legacy::Notify = Arc::new(|progress: &Progress| {
        if progress.total > 0 && (progress.done == progress.total || progress.done == 0) {
            println!(
                "  {}: {} из {}",
                stage_word(progress.stage),
                progress.done,
                progress.total
            );
        }
    });
    let result = services
        .legacy()
        .run_cli(legacy, &name, dry_run, notify, &|line| println!("{line}"))
        .await;
    if let Some(path) = extracted {
        let _ = std::fs::remove_file(path);
    }
    match result.context("перенос не выполнен")? {
        Some(outcome) => {
            let c = outcome.counts;
            println!(
                "Перенесено: клиентов {} (новых {}, объединено {}, обновлено {}), подписок {}, тарифов {}, промокодов {}, рефералов {}, платежей {}, обращений {}.",
                c.clients,
                outcome.created,
                outcome.merged,
                outcome.updated,
                c.subscriptions,
                c.tariffs,
                c.promo_codes,
                c.referrals,
                c.payments,
                c.tickets
            );
            if outcome.issues > 0 {
                println!(
                    "Проблем: {} — список в админке: «Настройки → Перенос из другого бота», «Скачать отчёт».",
                    outcome.issues
                );
            }
            Ok(())
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn без_файла_нужен_адрес_старой_базы() {
        let err = source_url(&vars(&[])).unwrap_err().to_string();
        assert!(err.contains(LEGACY_URL));
        assert!(err.contains("файл"));
        let err = source_url(&vars(&[(LEGACY_URL, "mysql://old")]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("postgres://"));
        assert_eq!(
            source_url(&vars(&[(LEGACY_URL, " postgres://old/db ")])).unwrap(),
            "postgres://old/db"
        );
    }
}
