mod account;
mod buy;
pub mod consent;
mod extras;
pub mod handler;
pub mod login;
pub mod notify;
pub mod photos;
pub mod session;
pub mod stars;
pub mod telegram;
pub mod text;

#[cfg(test)]
mod tests;

use handler::BotCtx;
use sn_domain::settings::keys;
use sn_modules::ModuleError;
use sn_services::{ServiceError, Services};
use sn_store::{Db, StoreError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use handler::handle_update;
pub use notify::NotifyHandler;
pub use telegram::{TelegramApi, TelegramError, Update};

pub const OFFSET_KEY: &str = "telegram.offset";

const SAVE_RETRY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct BotConfig {
    pub api_base: String,
    pub poll_timeout: Duration,
    pub workers: usize,
    pub stars_retry_every: Duration,
    pub stars_reconcile_every: Duration,
}

impl Default for BotConfig {
    fn default() -> Self {
        BotConfig {
            api_base: telegram::DEFAULT_API.into(),
            poll_timeout: Duration::from_secs(25),
            workers: 8,
            stars_retry_every: Duration::from_secs(30),
            stars_reconcile_every: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BotError {
    #[error(transparent)]
    Telegram(#[from] TelegramError),
    #[error(transparent)]
    Service(#[from] ServiceError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Module(#[from] ModuleError),
    #[error("функция бота ещё не реализована: {0}")]
    NotImplemented(&'static str),
}

enum Stop {
    Cancelled,
    TokenChanged,
}

pub struct Bot {
    services: Services,
    config: BotConfig,
}

pub fn shard_for(chat_key: i64, workers: usize) -> usize {
    let workers = workers.max(1) as i64;
    chat_key.rem_euclid(workers) as usize
}

async fn load_offset(services: &Services) -> Result<Option<i64>, StoreError> {
    let mut conn = services.store().conn().await?;
    Ok(conn
        .bot_state()
        .get(OFFSET_KEY)
        .await?
        .and_then(|v| v.as_i64()))
}

async fn save_offset(services: &Services, offset: i64) -> Result<(), StoreError> {
    let mut conn = services.store().conn().await?;
    conn.bot_state()
        .set(OFFSET_KEY, &serde_json::json!(offset))
        .await
}

async fn remember_username(services: &Services, username: &str) -> Result<(), StoreError> {
    let mut conn = services.store().conn().await?;
    let stored = conn.settings().get(keys::BOT_USERNAME).await?;
    if stored.as_ref().and_then(|v| v.as_str()) != Some(username) {
        conn.settings()
            .set(keys::BOT_USERNAME, &serde_json::json!(username), None)
            .await?;
    }
    Ok(())
}

fn spawn_workers(ctx: &BotCtx, count: usize) -> (Vec<mpsc::Sender<Update>>, Vec<JoinHandle<()>>) {
    let mut senders = Vec::with_capacity(count);
    let mut handles = Vec::with_capacity(count);
    for _ in 0..count.max(1) {
        let (tx, mut rx) = mpsc::channel::<Update>(64);
        let ctx = ctx.clone();
        handles.push(tokio::spawn(async move {
            while let Some(update) = rx.recv().await {
                handle_update(&ctx, update).await;
            }
        }));
        senders.push(tx);
    }
    (senders, handles)
}

impl Bot {
    pub fn new(services: Services, config: BotConfig) -> Self {
        Bot { services, config }
    }

    pub async fn run(self, cancel: CancellationToken) -> Result<(), BotError> {
        let mut announced = false;
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let Some(token) = self.services.bot_token() else {
                if !announced {
                    tracing::info!("бот выключен: не задан токен");
                    announced = true;
                }
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(Duration::from_secs(30)) => continue,
                }
            };
            announced = false;
            match self.serve(&token, &cancel).await {
                Ok(Stop::Cancelled) => return Ok(()),
                Ok(Stop::TokenChanged) => {
                    tracing::info!("токен бота сменился, переподключаемся");
                }
                Err(err) => {
                    tracing::warn!(error = %err, "бот не запустился, повтор через 30 секунд");
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                    }
                }
            }
        }
    }

    async fn serve(&self, token: &str, cancel: &CancellationToken) -> Result<Stop, BotError> {
        let api = Arc::new(TelegramApi::new(&self.config.api_base, token));
        let me = api.get_me().await?;
        if let Some(username) = me.username.as_deref()
            && let Err(err) = remember_username(&self.services, username).await
        {
            tracing::warn!(error = %err, "имя бота не сохранено");
        }
        tracing::info!(username = %me.username.clone().unwrap_or_default(), "бот подключён");
        if let Err(err) = api.delete_webhook().await {
            tracing::debug!(error = %err, "вебхук бота не снят");
        }
        if let Err(err) = api
            .set_commands(&[
                ("start", "Главное меню"),
                ("paysupport", "Вопросы по оплате звёздами"),
            ])
            .await
        {
            tracing::debug!(error = %err, "команды бота не обновлены");
        }
        let ctx = BotCtx {
            services: self.services.clone(),
            api: api.clone(),
            username: me.username.clone(),
        };
        let (senders, handles) = spawn_workers(&ctx, self.config.workers);
        let mut offset = load_offset(&self.services).await?;
        let keeper_stop = cancel.child_token();
        let keeper_guard = keeper_stop.clone().drop_guard();
        let keeper = tokio::spawn(
            stars::Keeper::new(
                self.services.clone(),
                &self.config.api_base,
                token,
                me.id,
                self.config.stars_retry_every,
                self.config.stars_reconcile_every,
            )
            .run(keeper_stop),
        );
        let stop = loop {
            if self.services.bot_token().as_deref() != Some(token) {
                break Stop::TokenChanged;
            }
            let polled = tokio::select! {
                _ = cancel.cancelled() => break Stop::Cancelled,
                polled = api.get_updates(offset, self.config.poll_timeout) => polled,
            };
            match polled {
                Ok(updates) if updates.is_empty() => {}
                Ok(updates) => {
                    let payments = stars::from_updates(&updates);
                    if !payments.is_empty()
                        && let Err(err) = stars::record(&self.services, &payments).await
                    {
                        tracing::warn!(
                            error = %err,
                            "оплаты звёздами не сохранены, апдейты будут прочитаны заново"
                        );
                        tokio::select! {
                            _ = cancel.cancelled() => break Stop::Cancelled,
                            _ = tokio::time::sleep(SAVE_RETRY) => {}
                        }
                        continue;
                    }
                    let mut next = offset.unwrap_or(0);
                    for update in updates {
                        next = next.max(update.update_id + 1);
                        if update.pre_checkout_query.is_some() {
                            let ctx = ctx.clone();
                            tokio::spawn(async move { handle_update(&ctx, update).await });
                            continue;
                        }
                        let shard = shard_for(update.chat_key(), senders.len());
                        if senders[shard].send(update).await.is_err() {
                            tracing::warn!("обработчик апдейтов остановился");
                        }
                    }
                    offset = Some(next);
                    if let Err(err) = save_offset(&self.services, next).await {
                        tracing::warn!(error = %err, "смещение апдейтов не сохранено");
                    }
                }
                Err(err) => {
                    let pause = match &err {
                        TelegramError::Api { code: 401, .. } => return Err(err.into()),
                        TelegramError::Api { code: 409, .. } => Duration::from_secs(10),
                        other => Duration::from_secs(other.retry_after().unwrap_or(3)),
                    };
                    tracing::warn!(error = %err, "getUpdates не удался");
                    tokio::select! {
                        _ = cancel.cancelled() => break Stop::Cancelled,
                        _ = tokio::time::sleep(pause) => {}
                    }
                }
            }
        };
        drop(senders);
        drop(keeper_guard);
        for handle in handles {
            if tokio::time::timeout(Duration::from_secs(8), handle)
                .await
                .is_err()
            {
                tracing::warn!("обработчик апдейтов не завершился за 8 секунд");
            }
        }
        if tokio::time::timeout(Duration::from_secs(8), keeper)
            .await
            .is_err()
        {
            tracing::warn!("сверка оплат звёздами не завершилась за 8 секунд");
        }
        Ok(stop)
    }
}
