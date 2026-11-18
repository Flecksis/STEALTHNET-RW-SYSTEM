use crate::config::Config;
use anyhow::{Result, bail};
use serde_json::Value;
use sn_services::Services;
use sn_services::backups::archive::ArchiveError;
use sn_services::backups::format::RunKind;
use sn_services::backups::{self, BackupEnv, Preview};
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const RECHECK: Duration = Duration::from_secs(600);
const AFTER_RUN: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct NeedPassword;

impl std::fmt::Display for NeedPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "копия зашифрована: передайте её пароль через SN_BACKUP_PASSWORD или первой строкой stdin с --password-stdin",
        )
    }
}

impl std::error::Error for NeedPassword {}

pub fn env(config: &Config) -> BackupEnv {
    BackupEnv {
        dir: config.backup_dir.clone(),
        host_dir: config.backup_host_dir.clone(),
        telegram_api: config.telegram_api.clone(),
        secret_key: config.secret_key.clone(),
        container: sn_services::server_load::in_container(),
        ..BackupEnv::default()
    }
}

pub async fn schedule(services: Services, cancel: CancellationToken) {
    if let Err(err) = services.backups().mark_stale().await {
        tracing::warn!(error = %err, "прерванные копии не отмечены");
    }
    loop {
        services.backups().cleanup_uploads().await;
        let wait = match services.backups().next_run_in().await {
            Ok(Some(wait)) => wait.min(RECHECK),
            Ok(None) => RECHECK,
            Err(err) => {
                tracing::warn!(error = %err, "не удалось узнать, когда делать резервную копию");
                Duration::from_secs(300)
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = services.backups_runtime().woken() => continue,
            _ = tokio::time::sleep(wait) => {}
        }
        if services.pause_gate().is_paused() {
            continue;
        }
        match services.backups().run_scheduled().await {
            Ok(Some(made)) => {
                tracing::info!(file = %made.name, "резервная копия по расписанию сделана");
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(error = %err, "резервная копия по расписанию не сделана");
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(AFTER_RUN) => {}
        }
    }
}

fn read_password(stdin: bool) -> Result<Option<String>> {
    if let Ok(password) = std::env::var("SN_BACKUP_PASSWORD")
        && !password.is_empty()
    {
        return Ok(Some(password));
    }
    if !stdin {
        return Ok(None);
    }
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();
    Ok((!password.is_empty()).then_some(password))
}

fn keys_line(keys: &str) -> &'static str {
    match keys {
        "same" => "ключ шифрования тот же: ключи платёжек и токен бота сохранятся",
        "from_copy" => {
            "в копии есть ключ шифрования: ключи платёжек и токен бота перешифруются под этот сервер"
        }
        "no_server_key" => {
            "на сервере не задан SN_SECRET_KEY: секреты из копии сохранить не получится"
        }
        "none" => "секретов в копии нет",
        _ => {
            "в копии нет ключа шифрования этого сервера: ключи платёжек, токен бота и другие секреты придётся ввести заново"
        }
    }
}

fn print_preview(services: &Services, preview: &Preview) {
    let offset = services.settings().remna_offset();
    let local = preview.created_at.with_timezone(&offset);
    println!(
        "Копия от {} ({}), {}, версия {}",
        local.format("%d.%m.%Y %H:%M"),
        backups::utc_label(offset),
        backups::kind_title(preview.kind),
        preview.app
    );
    if let Some(brand) = preview.brand.as_deref().filter(|b| !b.is_empty()) {
        match preview.public_url.as_deref() {
            Some(url) => println!("Панель: {brand}, {url}"),
            None => println!("Панель: {brand}"),
        }
    }
    println!("Внутри: {}", backups::counts_line(&preview.counts));
    if let Some(remna) = &preview.remnawave {
        if remna.source == "remnawave" {
            println!(
                "Снимок Remnawave: {} подписок с юзерами Remnawave",
                remna.linked
            );
        } else {
            println!("Снимок подписок по данным панели: {}", remna.linked);
        }
    }
    if preview.same_version {
        println!(
            "Версия базы совпадает с этой панелью ({})",
            preview.current_app
        );
    } else {
        println!(
            "База обновится до версии этой панели ({}): изменений базы — {}",
            preview.current_app, preview.migrations_to_apply
        );
    }
    println!("Ключи: {}", keys_line(preview.keys));
    for warning in &preview.warnings {
        println!("Внимание: {warning}");
    }
}

fn print_result(services: &Services, result: &Value) {
    println!("Готово: база восстановлена из копии.");
    if let Some(before) = result.get("before").and_then(Value::as_str) {
        println!("Копия базы до восстановления: {before}");
    }
    if let Some(warnings) = result.get("warnings").and_then(Value::as_array) {
        for warning in warnings.iter().filter_map(Value::as_str) {
            println!("Внимание: {warning}");
        }
    }
    println!("Пароли администраторов теперь такие, как были в копии.");
    let path = result
        .get("remna_sync_path")
        .and_then(Value::as_str)
        .unwrap_or(backups::REMNA_SYNC_PATH);
    match services.settings().public_url.as_deref() {
        Some(url) => println!(
            "Если Remnawave потеряла пользователей или в ней другие сроки, перенесите клиентов и подписки в Remnawave: {}{path}",
            url.trim_end_matches('/')
        ),
        None => println!(
            "Если Remnawave потеряла пользователей или в ней другие сроки, перенесите клиентов и подписки в Remnawave: Настройки → Remnawave"
        ),
    }
}

pub async fn backup(services: &Services) -> Result<()> {
    let made = services.backups().make_now(RunKind::Manual, None).await?;
    println!("Копия сохранена: {}", made.path.display());
    println!("Размер: {}", backups::size_text(made.size));
    println!("Внутри: {}", backups::counts_line(&made.manifest.counts));
    if made.manifest.secret_key.included {
        println!("Зашифрована паролем копий, ключ шифрования секретов внутри.");
    } else if made.name.ends_with(backups::format::SEALED_EXT) {
        println!("Зашифрована паролем копий.");
    } else {
        println!(
            "Без пароля: ключей платёжек в копии нет. Задать пароль можно в админке: Настройки → Резервные копии."
        );
    }
    Ok(())
}

pub async fn restore(
    services: &Services,
    file: &str,
    check: bool,
    yes: bool,
    password_stdin: bool,
) -> Result<()> {
    let path = PathBuf::from(file);
    if !path.is_file() {
        bail!("файл не найден: {file}");
    }
    let password = tokio::task::spawn_blocking(move || read_password(password_stdin)).await??;
    let probe = path.clone();
    let probe_password = password.clone();
    let inspection = tokio::task::spawn_blocking(move || {
        backups::inspect_file(&probe, probe_password.as_deref())
    })
    .await?;
    let inspection = match inspection {
        Ok(inspection) => inspection,
        Err(ArchiveError::PasswordNeeded) => return Err(NeedPassword.into()),
        Err(err) => bail!("{}", backups::load::archive_error(err)),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string());
    let preview = services.backups().preview_of(&inspection, name)?;
    print_preview(services, &preview);
    if check {
        return Ok(());
    }
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("подтвердите восстановление флагом --yes");
        }
        print!(
            "Заменить текущую базу этой копией? Перед заменой панель сохранит копию текущей базы. [д/Н] "
        );
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        let answer = answer.trim().to_lowercase();
        if !(answer.starts_with('д') || answer.starts_with('y')) {
            println!("Ничего не сделано.");
            return Ok(());
        }
    }
    let result = services.backups().restore_now(&path, password).await?;
    print_result(services, &result);
    Ok(())
}
