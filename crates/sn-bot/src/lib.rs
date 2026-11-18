mod account;
pub mod alerts;
pub mod broadcast;
mod buy;
pub mod consent;
pub mod contests;
mod extras;
mod gifts;
pub mod gram_wallet;
pub mod handler;
pub mod login;
pub mod media;
pub mod notify;
pub mod photos;
pub mod session;
pub mod stars;
pub mod telegram;
pub mod text;
pub mod ui;

#[cfg(test)]
mod tests;

use handler::BotCtx;
use sn_domain::settings::keys;
use sn_modules::ModuleError;
use sn_services::{BotProblem, BotState, ServiceError, Services};
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
const IDLE_RECHECK: Duration = Duration::from_secs(30);
const RETRY_PAUSE: Duration = Duration::from_secs(30);
const WEBHOOK_PAUSE: Duration = Duration::from_secs(120);
const BAD_TOKEN_PAUSE: Duration = Duration::from_secs(600);
const UNREACHABLE_AFTER: u32 = 2;

#[derive(Debug, Clone)]
pub struct BotConfig {
    pub api_base: String,
    pub poll_timeout: Duration,
    pub workers: usize,
    pub stars_retry_every: Duration,
    pub stars_reconcile_every: Duration,
    pub alerts_tick: Duration,
    pub alerts_every: Duration,
}

impl Default for BotConfig {
    fn default() -> Self {
        BotConfig {
            api_base: telegram::DEFAULT_API.into(),
            poll_timeout: Duration::from_secs(25),
            workers: 8,
            stars_retry_every: Duration::from_secs(30),
            stars_reconcile_every: Duration::from_secs(60),
            alerts_tick: Duration::from_secs(5),
            alerts_every: Duration::from_secs(60),
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
    #[error("бот подключён вебхуком к другому адресу")]
    Webhook(Option<String>),
    #[error("этот токен уже опрашивает другая программа")]
    Conflict,
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

fn is_webhook_conflict(description: &str) -> bool {
    description.to_ascii_lowercase().contains("webhook")
}

fn webhook_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

fn diagnose(err: &BotError) -> (BotProblem, Option<String>, Duration) {
    match err {
        BotError::Telegram(TelegramError::Api {
            code: 401 | 404, ..
        }) => (BotProblem::BadToken, None, BAD_TOKEN_PAUSE),
        BotError::Telegram(TelegramError::Api {
            code: 409,
            description,
            ..
        }) if is_webhook_conflict(description) => (BotProblem::Webhook, None, WEBHOOK_PAUSE),
        BotError::Telegram(TelegramError::Api { code: 409, .. }) | BotError::Conflict => {
            (BotProblem::Conflict, None, RETRY_PAUSE)
        }
        BotError::Webhook(host) => (BotProblem::Webhook, host.clone(), WEBHOOK_PAUSE),
        BotError::Telegram(TelegramError::Transport(_)) => {
            (BotProblem::Unreachable, None, RETRY_PAUSE)
        }
        BotError::Telegram(TelegramError::Api { description, .. }) => {
            (BotProblem::Telegram, Some(description.clone()), RETRY_PAUSE)
        }
        BotError::Telegram(TelegramError::Decode(_)) => (BotProblem::Telegram, None, RETRY_PAUSE),
        BotError::Service(_)
        | BotError::Store(_)
        | BotError::Module(_)
        | BotError::NotImplemented(_) => (BotProblem::Internal, None, RETRY_PAUSE),
    }
}

fn later(services: &Services, pause: Duration) -> chrono::DateTime<chrono::Utc> {
    services.now() + chrono::Duration::from_std(pause).unwrap_or_else(|_| chrono::Duration::zero())
}

fn offset_for(value: &serde_json::Value, bot: i64) -> Option<i64> {
    match value {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::Object(map) if map.get("bot").and_then(|b| b.as_i64()) == Some(bot) => {
            map.get("offset").and_then(|o| o.as_i64())
        }
        _ => None,
    }
}

async fn load_offset(services: &Services, bot: i64) -> Result<Option<i64>, StoreError> {
    let mut conn = services.store().conn().await?;
    Ok(conn
        .bot_state()
        .get(OFFSET_KEY)
        .await?
        .and_then(|v| offset_for(&v, bot)))
}

async fn save_offset(services: &Services, bot: i64, offset: i64) -> Result<(), StoreError> {
    let mut conn = services.store().conn().await?;
    conn.bot_state()
        .set(
            OFFSET_KEY,
            &serde_json::json!({ "bot": bot, "offset": offset }),
        )
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

const MENU_BUTTON: &str = "bot.menu_button";

async fn stored_menu(services: &Services) -> Option<String> {
    let mut conn = services.store().conn().await.ok()?;
    conn.settings()
        .get(MENU_BUTTON)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(str::to_string))
}

fn desired_menu(services: &Services) -> Option<(String, String)> {
    let url = services.settings().miniapp_url()?;
    let text = services
        .bot_content()
        .content
        .button("menu.btn.app", &[])
        .map(|label| label.text)
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| "Приложение".to_string());
    Some((text, url))
}

async fn sync_menu(api: &TelegramApi, services: &Services, applied: &mut Option<String>) {
    let desired = desired_menu(services);
    let url = desired.as_ref().map(|(_, url)| url.clone());
    if url == *applied {
        return;
    }
    let app = desired
        .as_ref()
        .map(|(text, url)| (text.as_str(), url.as_str()));
    match api.set_menu_button(app).await {
        Ok(_) => {
            *applied = url;
            let saved = match services.store().conn().await {
                Ok(mut conn) => conn
                    .settings()
                    .set(MENU_BUTTON, &serde_json::json!(applied.as_deref()), None)
                    .await
                    .map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            };
            if let Err(err) = saved {
                tracing::warn!(error = %err, "кнопка меню бота обновлена, но не запомнена");
            }
        }
        Err(err) => tracing::debug!(error = %err, "кнопка меню бота не обновлена"),
    }
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
        let runtime = self.services.bot_runtime();
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let Some(token) = self.services.bot_token() else {
                if runtime.state() != BotState::NoToken {
                    tracing::info!("бот выключен: не задан токен");
                    runtime.set(BotState::NoToken);
                }
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    _ = runtime.woken() => {}
                    _ = tokio::time::sleep(IDLE_RECHECK) => {}
                }
                continue;
            };
            runtime.set(BotState::Starting);
            match self.serve(&token, &cancel).await {
                Ok(Stop::Cancelled) => return Ok(()),
                Ok(Stop::TokenChanged) => {
                    tracing::info!("токен бота сменился, переподключаемся");
                }
                Err(err) => {
                    let (problem, detail, pause) = diagnose(&err);
                    runtime.set(BotState::Failed {
                        problem,
                        detail,
                        at: self.services.now(),
                        retry_at: Some(later(&self.services, pause)),
                    });
                    tracing::warn!(error = %err, ?problem, "бот не запустился, повторим позже");
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = runtime.woken() => {}
                        _ = tokio::time::sleep(pause) => {}
                    }
                }
            }
        }
    }

    async fn serve(&self, token: &str, cancel: &CancellationToken) -> Result<Stop, BotError> {
        let runtime = self.services.bot_runtime();
        let takeover = runtime.take_takeover();
        let api = Arc::new(TelegramApi::new(&self.config.api_base, token));
        let me = api.get_me().await?;
        if let Some(username) = me.username.as_deref()
            && let Err(err) = remember_username(&self.services, username).await
        {
            tracing::warn!(error = %err, "имя бота не сохранено");
        }
        match api.get_webhook_info().await {
            Ok(hook) if !hook.url.is_empty() && !takeover => {
                return Err(BotError::Webhook(webhook_host(&hook.url)));
            }
            Ok(_) | Err(_) if takeover => {
                api.delete_webhook().await?;
                tracing::info!("вебхук бота снят: администратор забрал бота сюда");
            }
            Ok(_) => {}
            Err(err) => {
                tracing::debug!(error = %err, "сведения о вебхуке бота не получены");
            }
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
        let mut offset = load_offset(&self.services, me.id).await?;
        tracing::info!(username = %me.username.clone().unwrap_or_default(), "бот подключён");
        let ctx = BotCtx {
            services: self.services.clone(),
            api: api.clone(),
            username: me.username.clone(),
        };
        let (senders, handles) = spawn_workers(&ctx, self.config.workers);
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
        let alerts_stop = cancel.child_token();
        let alerts_guard = alerts_stop.clone().drop_guard();
        let alerts_keeper = tokio::spawn(
            alerts::Keeper::new(
                self.services.clone(),
                api.clone(),
                self.config.alerts_tick,
                self.config.alerts_every,
            )
            .run(alerts_stop),
        );
        let since = self.services.now();
        let running = |at| BotState::Running {
            username: me.username.clone(),
            since,
            last_ok_at: at,
        };
        runtime.set(running(since));
        let mut failures = 0u32;
        let mut menu = stored_menu(&self.services).await;
        let outcome: Result<Stop, BotError> = loop {
            if self.services.bot_token().as_deref() != Some(token) {
                break Ok(Stop::TokenChanged);
            }
            sync_menu(&api, &self.services, &mut menu).await;
            let polled = tokio::select! {
                _ = cancel.cancelled() => break Ok(Stop::Cancelled),
                _ = runtime.woken() => continue,
                polled = api.get_updates(offset, self.config.poll_timeout) => polled,
            };
            match polled {
                Ok(updates) => {
                    failures = 0;
                    runtime.set(running(self.services.now()));
                    if updates.is_empty() {
                        continue;
                    }
                    let payments = stars::from_updates(&updates);
                    if !payments.is_empty()
                        && let Err(err) = stars::record(&self.services, &payments).await
                    {
                        tracing::warn!(
                            error = %err,
                            "оплаты звёздами не сохранены, апдейты будут прочитаны заново"
                        );
                        tokio::select! {
                            _ = cancel.cancelled() => break Ok(Stop::Cancelled),
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
                    if let Err(err) = save_offset(&self.services, me.id, next).await {
                        tracing::warn!(error = %err, "смещение апдейтов не сохранено");
                    }
                }
                Err(err) => {
                    match &err {
                        TelegramError::Api {
                            code: 401 | 404, ..
                        } => break Err(err.into()),
                        TelegramError::Api {
                            code: 409,
                            description,
                            ..
                        } if is_webhook_conflict(description) => {
                            break Err(BotError::Webhook(None));
                        }
                        TelegramError::Api { code: 409, .. } => break Err(BotError::Conflict),
                        _ => {}
                    }
                    failures += 1;
                    let pause = Duration::from_secs(err.retry_after().unwrap_or(3));
                    if failures >= UNREACHABLE_AFTER {
                        let (problem, detail) = match &err {
                            TelegramError::Transport(_) => (BotProblem::Unreachable, None),
                            TelegramError::Api { description, .. } => {
                                (BotProblem::Telegram, Some(description.clone()))
                            }
                            TelegramError::Decode(_) => (BotProblem::Telegram, None),
                        };
                        runtime.set(BotState::Failed {
                            problem,
                            detail,
                            at: self.services.now(),
                            retry_at: Some(later(&self.services, pause)),
                        });
                    }
                    tracing::warn!(error = %err, "getUpdates не удался");
                    tokio::select! {
                        _ = cancel.cancelled() => break Ok(Stop::Cancelled),
                        _ = tokio::time::sleep(pause) => {}
                    }
                }
            }
        };
        drop(senders);
        drop(keeper_guard);
        drop(alerts_guard);
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
        if tokio::time::timeout(Duration::from_secs(8), alerts_keeper)
            .await
            .is_err()
        {
            tracing::warn!("уведомления администраторам не остановились за 8 секунд");
        }
        outcome
    }
}
