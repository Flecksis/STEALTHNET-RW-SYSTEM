mod cli;
mod config;
mod legacy;
mod watch;

use anyhow::{Context, Result, bail};
use cli::Command;
use config::{Config, LogFormat};
use sn_bot::{Bot, BotConfig, NotifyHandler};
use sn_domain::jobs::JobKind;
use sn_modules::ModuleRegistry;
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

fn worker(services: Services) -> Worker {
    Worker::new(services, WorkerConfig::default()).with_kind_handler(
        JobKind::Notify,
        Arc::new(NotifyHandler::new(&BotConfig::default())),
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
        payments: Arc::new(ProviderRegistry::builtin()),
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
    let cancel = CancellationToken::new();
    let signals = cancel.clone();
    tokio::spawn(async move {
        wait_for_shutdown().await;
        tracing::info!("получен сигнал остановки");
        signals.cancel();
    });
    let mut background = vec![tokio::spawn(watch::settings_changes(
        services.clone(),
        cancel.clone(),
    ))];
    if config.run_worker {
        background.push(tokio::spawn(worker(services.clone()).run(cancel.clone())));
    }
    if config.run_bot {
        let bot = Bot::new(services.clone(), BotConfig::default());
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
    if let Command::ImportLegacy { dry_run } = command {
        return legacy::import(&std::env::vars().collect(), dry_run);
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
        Command::Serve => serve(config, store).await,
        Command::ImportLegacy { .. } | Command::GenSecretKey | Command::Version | Command::Help => {
            Ok(())
        }
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
            println!("stealthnet {}", env!("CARGO_PKG_VERSION"));
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
        Err(err) => {
            tracing::error!(error = %format!("{err:#}"), "остановка с ошибкой");
            eprintln!("ошибка: {err:#}");
            ExitCode::FAILURE
        }
    }
}
