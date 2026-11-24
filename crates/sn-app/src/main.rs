mod backups;
mod cli;
mod config;
mod legacy;
mod remna;
mod updates;
mod watch;

use anyhow::{Context, Result, bail};
use cli::Command;
use config::{Config, LogFormat};
use sn_bot::{Bot, BotConfig, NotifyHandler};
use sn_domain::jobs::JobKind;
use sn_modules::catalog::broadcast;
use sn_modules::{BotSender, ModuleId, ModuleRegistry};
use sn_payments::ProviderRegistry;
use sn_server::AppState;
use sn_services::{ServiceDeps, Services, SystemClock};
use sn_store::{SecretBox, Store, StoreOptions};
use sn_worker::{Worker, WorkerConfig};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

fn init_logging(config: &Config) {
    let filter = EnvFilter::try_new(&config.log_filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    let installed = match config.log_format {
        LogFormat::Text => builder.try_init(),
        LogFormat::Json => builder.json().try_init(),
    };
    if installed.is_err() {
        eprintln!("логирование уже настроено");
    }
}

fn read_password() -> Result<String> {
    if let Ok(password) = std::env::var("SN_ADMIN_PASSWORD")
        && !password.trim().is_empty()
    {
        return Ok(password.trim().to_string());
    }
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("пароль не прочитан из stdin")?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();
    if password.is_empty() {
        bail!("пустой пароль: передайте его через SN_ADMIN_PASSWORD или stdin");
    }
    Ok(password)
}

fn module_registry() -> Result<ModuleRegistry> {
    ModuleRegistry::new(sn_modules::catalog::all()).context("реестр модулей не собран")
}

fn bot_config(config: &Config) -> BotConfig {
    BotConfig {
        api_base: config.telegram_api.clone(),
        workers: config.bot_concurrency,
        ..BotConfig::default()
    }
}

fn worker(services: Services, config: &Config) -> Worker {
    Worker::new(services, WorkerConfig::default()).with_kind_handler(
        JobKind::Notify,
        Arc::new(NotifyHandler::new(&bot_config(config))),
    )
}

fn build_services(config: &Config, store: Store) -> Result<Services> {
    let secrets = config
        .secret_key
        .as_deref()
        .map(SecretBox::from_base64)
        .transpose()
        .context("SN_SECRET_KEY")?;
    if secrets.is_none() {
        tracing::warn!(
            "SN_SECRET_KEY не задан: ключи платёжных систем и токены из админки не сохранятся"
        );
    }
    Ok(Services::new(ServiceDeps {
        store,
        payments: Arc::new(ProviderRegistry::with_telegram_api(&config.telegram_api)),
        modules: Arc::new(module_registry()?),
        secrets,
        clock: Arc::new(SystemClock),
        env: config.env.clone(),
    }))
}

async fn wait_for_shutdown() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

async fn serve(config: Config, store: Store) -> Result<()> {
    for warning in &config.warnings {
        tracing::warn!("{warning}");
    }
    if config.migrate_on_start {
        store.migrate().await.context("миграции не применились")?;
    } else if store.pending_migrations().await? > 0 {
        bail!("в базе не применены миграции: выполните `stealthnet migrate`");
    }
    let services = build_services(&config, store)?;
    services
        .reload()
        .await
        .context("настройки из базы не загрузились")?;
    services.set_update_config(Some(config.updates.clone()));
    services.set_backup_env(Some(backups::env(&config)));
    if let Err(err) = services.legacy().recover().await {
        tracing::warn!(error = %err, "состояние переноса из старого бота не восстановлено");
    }
    if let Err(err) = services.remna_transfer().recover().await {
        tracing::warn!(error = %err, "состояние синхронизации с Remnawave не восстановлено");
    }
    match broadcast::seed_presets(&services.module_context()).await {
        Ok(seeded) if !seeded.created.is_empty() || !seeded.covered.is_empty() => {
            tracing::info!(
                created = ?seeded.created,
                covered = ?seeded.covered,
                enabled = seeded.enabled,
                "стандартные авторассылки разобраны"
            );
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "стандартные авторассылки не добавлены"),
    }
    let cancel = CancellationToken::new();
    let signals = cancel.clone();
    tokio::spawn(async move {
        wait_for_shutdown().await;
        tracing::info!("получен сигнал остановки");
        signals.cancel();
    });
    let mut background = vec![
        tokio::spawn(watch::settings_changes(services.clone(), cancel.clone())),
        tokio::spawn(updates::checks(services.clone(), cancel.clone())),
        tokio::spawn(backups::schedule(services.clone(), cancel.clone())),
    ];
    let sender: Arc<dyn BotSender> = Arc::new(sn_bot::broadcast::TelegramSender::new(
        services.clone(),
        &config.telegram_api,
    ));
    services.set_bot_sender(Some(sender.clone()));
    if config.run_worker {
        background.push(tokio::spawn(
            worker(services.clone(), &config).run(cancel.clone()),
        ));
        background.push(tokio::spawn(
            sn_services::ext_modules::Deliverer::new(services.clone())
                .run(cancel.clone().cancelled_owned()),
        ));
        let ctx = services.module_context();
        let registry = services.modules().clone();
        background.push(tokio::spawn(
            broadcast::Engine::new(
                ctx.clone(),
                broadcast::Sender::Bot(sender),
                move || registry.is_enabled(ModuleId::Broadcast),
                broadcast::EngineConfig::default(),
            )
            .run(cancel.clone().cancelled_owned()),
        ));
        if let Some(mail) = ctx.core.mail_sender() {
            let registry = services.modules().clone();
            background.push(tokio::spawn(
                broadcast::Engine::new(
                    ctx,
                    broadcast::Sender::Mail(mail),
                    move || registry.is_enabled(ModuleId::Broadcast),
                    broadcast::EngineConfig::mail(),
                )
                .run(cancel.clone().cancelled_owned()),
            ));
        }
    }
    if config.run_bot {
        background.push(tokio::spawn(
            sn_bot::photos::PhotoKeeper::new(
                services.clone(),
                &config.telegram_api,
                sn_bot::photos::ROUND_EVERY,
            )
            .run(cancel.clone()),
        ));
        let bot = Bot::new(services.clone(), bot_config(&config));
        let token = cancel.clone();
        background.push(tokio::spawn(async move {
            if let Err(err) = bot.run(token).await {
                tracing::error!(error = %err, "бот остановился с ошибкой");
            }
        }));
    }
    let state = AppState::new(config.server.clone(), services);
    let served = sn_server::serve(state, cancel.clone()).await;
    cancel.cancel();
    for task in background {
        if tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .is_err()
        {
            tracing::warn!("фоновая задача не остановилась за 10 секунд");
        }
    }
    served.context("HTTP-сервер остановился с ошибкой")
}

async fn run(command: Command) -> Result<()> {
    let config = Config::from_env()?;
    init_logging(&config);
    if let Command::ImportLegacy { file: None, .. } = &command {
        legacy::source_url(&std::env::vars().collect())?;
    }
    let store = Store::connect(
        &config.database_url,
        StoreOptions {
            max_connections: config.db_max_connections,
            ..StoreOptions::default()
        },
    )
    .await
    .context("база данных недоступна")?;
    match command {
        Command::Migrate => {
            store.migrate().await.context("миграции не применились")?;
            println!("Миграции применены");
            Ok(())
        }
        Command::CreateAdmin { email } => {
            if store.pending_migrations().await? > 0 {
                bail!("сначала выполните `stealthnet migrate`");
            }
            let password = tokio::task::spawn_blocking(read_password).await??;
            let services = build_services(&config, store)?;
            let (id, created) = services.auth().upsert_owner(&email, &password).await?;
            if created {
                println!("Владелец создан: {email} (id {id})");
            } else {
                println!("Пароль владельца {email} обновлён, старые сессии закрыты");
            }
            Ok(())
        }
        Command::UnlockLogin { email, addresses } => {
            if store.pending_migrations().await? > 0 {
                bail!("сначала выполните `stealthnet migrate`");
            }
            let services = build_services(&config, store)?;
            let unlock = services
                .auth()
                .unlock_admin_login(&email, &addresses)
                .await?;
            for line in cli::unlock_report(&unlock) {
                println!("{line}");
            }
            Ok(())
        }
        Command::Serve => serve(config, store).await,
        Command::ImportLegacy { dry_run, file } => {
            if store.pending_migrations().await? > 0 {
                bail!("в базе не применены миграции: выполните `stealthnet migrate`");
            }
            let services = build_services(&config, store)?;
            services
                .reload()
                .await
                .context("настройки из базы не загрузились")?;
            legacy::import(
                &services,
                &std::env::vars().collect(),
                dry_run,
                file.as_deref(),
            )
            .await
        }
        Command::Remna { pull, dry_run, fix } => {
            if store.pending_migrations().await? > 0 {
                bail!("в базе не применены миграции: выполните `stealthnet migrate`");
            }
            let services = build_services(&config, store)?;
            services
                .reload()
                .await
                .context("настройки из базы не загрузились")?;
            remna::run(&services, pull, dry_run, fix).await
        }
        Command::Backup | Command::Restore { .. } => {
            if store.pending_migrations().await? > 0 {
                bail!("в базе не применены миграции: выполните `stealthnet migrate`");
            }
            let services = build_services(&config, store)?;
            services
                .reload()
                .await
                .context("настройки из базы не загрузились")?;
            services.set_update_config(Some(config.updates.clone()));
            services.set_backup_env(Some(backups::env(&config)));
            match command {
                Command::Restore {
                    file,
                    check,
                    yes,
                    password_stdin,
                } => backups::restore(&services, &file, check, yes, password_stdin).await,
                _ => backups::backup(&services).await,
            }
        }
        Command::GenSecretKey | Command::Version | Command::Help => Ok(()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let command = match cli::parse(&args) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("{err:#}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Help => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            let build = sn_services::updates::BuildInfo::new(
                std::env::var("SN_BUILD_COMMIT").ok().as_deref(),
                std::env::var("SN_BUILD_DATE").ok().as_deref(),
            )
            .with_label(std::env::var("SN_BUILD_VERSION").ok().as_deref());
            println!(
                "stealthnet {}",
                build.label.as_deref().unwrap_or(&build.version)
            );
            if let Some(commit) = build.commit {
                println!("сборка {commit}");
            }
            if let Some(at) = build.built_at {
                println!("собрана {}", at.to_rfc3339());
            }
            return ExitCode::SUCCESS;
        }
        Command::GenSecretKey => {
            println!("{}", SecretBox::generate_key_base64());
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("не удалось запустить рантайм: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(command)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.downcast_ref::<backups::NeedPassword>().is_some() => {
            eprintln!("{err:#}");
            ExitCode::from(3)
        }
        Err(err) => {
            tracing::error!(error = %format!("{err:#}"), "остановка с ошибкой");
            eprintln!("ошибка: {err:#}");
            ExitCode::FAILURE
        }
    }
}
