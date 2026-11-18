pub mod groq;
pub mod prompt;
mod routes;
pub mod store;
#[cfg(test)]
mod tests;

use crate::catalog::{config_flag, config_number};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{Module, ModuleConfig, ScheduledTask};
use crate::registry::ModuleRegistry;
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use groq::{ChatMessage, ChatOptions, Completion, Groq, GroqError, MODELS};
use prompt::Verdict;
use sn_domain::access::ClientChannel;
use sn_domain::ids::TicketId;
use sn_domain::jobs::JobPayload;
use sn_domain::module::ModuleId;
use sn_domain::settings::SettingField;
use sn_domain::support::{HandoffReason, TicketChannel};
use sn_store::Db;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use store::{Claimed, Saved, Spent};

pub const ANSWER_TASK: &str = "answer";
pub const RELEASE_TASK: &str = "release";
pub const CLAIM_SECONDS: i64 = 120;
pub const PER_RUN: usize = 20;
pub const INSTRUCTIONS_MAX: usize = 8_000;
pub const DEFAULT_MAX_TOKENS: i64 = 1_000;
pub const DEFAULT_CLIENT_DAILY: i64 = 30;
pub const DEFAULT_DAILY_TOKENS: i64 = 1_000_000;
const MAX_TOKENS_RANGE: (i64, i64) = (200, 8_000);
const CLIENT_DAILY_RANGE: (i64, i64) = (1, 1_000);
const DAILY_TOKENS_RANGE: (i64, i64) = (1_000, 1_000_000_000);

static RUNNING: AtomicBool = AtomicBool::new(false);
static AGAIN: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub client_daily: i64,
    pub daily_tokens: i64,
    pub instructions: Option<String>,
    pub cabinet: bool,
    pub miniapp: bool,
    pub api_url: String,
}

fn bounded(config: &ModuleConfig, key: &str, default: i64, range: (i64, i64)) -> i64 {
    config_number(config, key)
        .unwrap_or(default)
        .clamp(range.0, range.1)
}

impl Settings {
    pub fn from_config(config: &ModuleConfig) -> Self {
        Settings {
            api_key: config.text("api_key").unwrap_or_default().to_string(),
            model: config.text("model").unwrap_or(MODELS[0]).to_string(),
            max_tokens: u32::try_from(bounded(
                config,
                "max_tokens",
                DEFAULT_MAX_TOKENS,
                MAX_TOKENS_RANGE,
            ))
            .unwrap_or(1_000),
            client_daily: bounded(
                config,
                "client_daily",
                DEFAULT_CLIENT_DAILY,
                CLIENT_DAILY_RANGE,
            ),
            daily_tokens: bounded(
                config,
                "daily_tokens",
                DEFAULT_DAILY_TOKENS,
                DAILY_TOKENS_RANGE,
            ),
            instructions: config.text("instructions").map(str::to_string),
            cabinet: config_flag(config, "cabinet").unwrap_or(true),
            miniapp: config_flag(config, "miniapp").unwrap_or(true),
            api_url: config
                .text("api_url")
                .unwrap_or(groq::DEFAULT_URL)
                .to_string(),
        }
    }

    pub fn ready(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    pub const fn takes(&self, channel: TicketChannel) -> bool {
        match channel {
            TicketChannel::Cabinet => self.cabinet,
            TicketChannel::Miniapp => self.miniapp,
            TicketChannel::Bot | TicketChannel::Unknown => false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Channels {
    pub cabinet: bool,
    pub miniapp: bool,
}

pub fn channels(registry: &ModuleRegistry) -> Channels {
    if !registry.is_enabled(ModuleId::Ai) {
        return Channels::default();
    }
    let settings = Settings::from_config(&registry.config(ModuleId::Ai));
    if !settings.ready() {
        return Channels::default();
    }
    Channels {
        cabinet: settings.cabinet,
        miniapp: settings.miniapp,
    }
}

pub fn takes_ticket(registry: &ModuleRegistry, channel: TicketChannel) -> bool {
    let open = channels(registry);
    match channel {
        TicketChannel::Cabinet => open.cabinet,
        TicketChannel::Miniapp => open.miniapp,
        TicketChannel::Bot | TicketChannel::Unknown => false,
    }
}

pub fn wake_job(ticket: TicketId) -> JobPayload {
    JobPayload::ModuleTask {
        module: ModuleId::Ai,
        task: ANSWER_TASK.to_string(),
        payload: serde_json::json!({ "ticket_id": ticket }),
    }
}

pub fn day_start(now: DateTime<Utc>, offset: FixedOffset) -> DateTime<Utc> {
    let local = now.with_timezone(&offset).date_naive();
    local
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| offset.from_local_datetime(&midnight).single())
        .map_or(now, |start| start.with_timezone(&Utc))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub answered: usize,
    pub handed_off: usize,
    pub stale: usize,
}

impl Report {
    pub const fn total(&self) -> usize {
        self.answered + self.handed_off + self.stale
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Answered,
    HandedOff(HandoffReason),
    Stale,
}

pub async fn ask(
    groq: &Groq,
    settings: &Settings,
    chat: &[ChatMessage],
) -> Result<Completion, GroqError> {
    let mut options = ChatOptions {
        max_tokens: settings.max_tokens,
        json: true,
        reasoning: true,
    };
    let mut retried = false;
    let mut last = GroqError::Malformed("Groq так и не ответил".into());
    for _ in 0..4 {
        match groq.chat(&settings.model, chat, options).await {
            Ok(completion) => return Ok(completion),
            Err(GroqError::Unsupported(message)) if options.reasoning => {
                options.reasoning = false;
                last = GroqError::Unsupported(message);
            }
            Err(GroqError::JsonFailed) if options.json => {
                options.json = false;
                last = GroqError::JsonFailed;
            }
            Err(err) if err.retryable() && !retried => {
                retried = true;
                let wait = match &err {
                    GroqError::RateLimited {
                        retry_after: Some(seconds),
                    } => (*seconds).min(5),
                    _ => 1,
                };
                tokio::time::sleep(Duration::from_secs(wait)).await;
                last = err;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last)
}

fn model_name(settings: &Settings) -> &'static str {
    MODELS
        .iter()
        .find(|m| **m == settings.model)
        .copied()
        .unwrap_or("другая модель")
}

async fn hand_off(
    ctx: &ModuleContext,
    claimed: &Claimed,
    reason: HandoffReason,
    spent: &Spent,
    error: Option<&str>,
) -> Result<Outcome, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    if store::hand_off(&mut conn, claimed.ticket_id, reason, spent, error).await? {
        tracing::info!(
            ticket = claimed.ticket_id.0,
            reason = reason.as_str(),
            "ИИ-чат передал обращение оператору"
        );
        Ok(Outcome::HandedOff(reason))
    } else {
        store::release(&mut conn, claimed.ticket_id).await?;
        Ok(Outcome::Stale)
    }
}

pub async fn process(
    ctx: &ModuleContext,
    settings: &Settings,
    claimed: &Claimed,
    groq_down: &mut Option<String>,
) -> Result<Outcome, ModuleError> {
    let core = ctx.core.core_settings();
    let now = ctx.core.now();
    let offset = core.remna_offset();
    let since = day_start(now, offset);
    let (messages, subscriptions, bot, client_answers, usage) = {
        let mut conn = ctx.store.conn().await?;
        let messages = conn.tickets().messages(claimed.ticket_id).await?;
        let subscriptions = conn.subscriptions().by_client(claimed.client_id).await?;
        let bot = store::bot_username(&mut conn).await?;
        let client_answers = store::client_answers(&mut conn, claimed.client_id, since).await?;
        let usage = store::usage(&mut conn, since).await?;
        (messages, subscriptions, bot, client_answers, usage)
    };
    let pending = prompt::pending_client_text(&messages);
    if pending.trim().is_empty() {
        return hand_off(
            ctx,
            claimed,
            HandoffReason::Unsure,
            &Spent::NONE,
            Some("в обращении нет вопроса клиента"),
        )
        .await;
    }
    if prompt::asks_for_human(&pending) {
        return hand_off(ctx, claimed, HandoffReason::Human, &Spent::NONE, None).await;
    }
    if prompt::about_money(&pending) {
        return hand_off(ctx, claimed, HandoffReason::Money, &Spent::NONE, None).await;
    }
    if client_answers >= settings.client_daily {
        return hand_off(ctx, claimed, HandoffReason::ClientLimit, &Spent::NONE, None).await;
    }
    if usage.tokens >= settings.daily_tokens {
        return hand_off(ctx, claimed, HandoffReason::Budget, &Spent::NONE, None).await;
    }
    if let Some(error) = groq_down.clone() {
        return hand_off(
            ctx,
            claimed,
            HandoffReason::Groq,
            &Spent::NONE,
            Some(&error),
        )
        .await;
    }
    let channel = match claimed.channel {
        TicketChannel::Miniapp => ClientChannel::Miniapp,
        _ => ClientChannel::Cabinet,
    };
    let facts = {
        let mut conn = ctx.store.conn().await?;
        prompt::facts(&mut conn, &core, channel, bot.as_deref()).await?
    };
    let client = prompt::client_text(&subscriptions, now, offset);
    let mut chat = vec![ChatMessage::system(prompt::system_prompt(
        &facts,
        &client,
        settings.instructions.as_deref(),
    ))];
    chat.extend(prompt::dialogue(&messages));
    let groq = Groq::new(&settings.api_url, &settings.api_key);
    let completion = match ask(&groq, settings, &chat).await {
        Ok(completion) => completion,
        Err(err) => {
            let text = err.to_string();
            tracing::warn!(ticket = claimed.ticket_id.0, error = %text, "Groq не ответил, обращение уходит оператору");
            *groq_down = Some(text.clone());
            return hand_off(ctx, claimed, HandoffReason::Groq, &Spent::NONE, Some(&text)).await;
        }
    };
    let spent = Spent {
        model: model_name(settings),
        prompt_tokens: completion.prompt_tokens,
        completion_tokens: completion.completion_tokens,
    };
    match prompt::parse_answer(&completion.content) {
        None => {
            let error = if completion.finish_reason.as_deref() == Some("length") {
                "модели не хватило токенов на ответ: увеличьте «Токенов на ответ»"
            } else {
                "ответ модели не разобран"
            };
            hand_off(ctx, claimed, HandoffReason::Unsure, &spent, Some(error)).await
        }
        Some(Verdict::Handoff(reason)) => hand_off(ctx, claimed, reason, &spent, None).await,
        Some(Verdict::Reply(reply)) => {
            if let Some(price) = prompt::unknown_price(&reply, &facts) {
                let error = format!("в ответе была цена {price}, которой нет в тарифах");
                return hand_off(ctx, claimed, HandoffReason::Unsure, &spent, Some(&error)).await;
            }
            let mut tx = ctx.store.begin().await?;
            let saved =
                store::answer(&mut tx, claimed, &prompt::clip_reply(&reply), &spent).await?;
            tx.commit().await?;
            Ok(match saved {
                Saved::Answered => Outcome::Answered,
                Saved::Stale => Outcome::Stale,
            })
        }
    }
}

pub async fn answer_pending(ctx: &ModuleContext) -> Result<Report, ModuleError> {
    let mut report = Report::default();
    let mut groq_down = None;
    while report.total() < PER_RUN {
        if !ctx.core.module_enabled(ModuleId::Ai) {
            break;
        }
        let settings = Settings::from_config(&ctx.core.module_config(ModuleId::Ai));
        if !settings.ready() {
            break;
        }
        let claimed = {
            let mut conn = ctx.store.conn().await?;
            store::claim(&mut conn, CLAIM_SECONDS).await?
        };
        let Some(claimed) = claimed else {
            break;
        };
        match process(ctx, &settings, &claimed, &mut groq_down).await? {
            Outcome::Answered => report.answered += 1,
            Outcome::HandedOff(_) => report.handed_off += 1,
            Outcome::Stale => report.stale += 1,
        }
    }
    Ok(report)
}

fn wake(ctx: &ModuleContext) {
    AGAIN.store(true, Ordering::SeqCst);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        loop {
            AGAIN.store(false, Ordering::SeqCst);
            let full = match answer_pending(&ctx).await {
                Ok(report) => {
                    if report.total() > 0 {
                        tracing::info!(
                            answered = report.answered,
                            handed_off = report.handed_off,
                            stale = report.stale,
                            "ИИ-чат разобрал обращения"
                        );
                    }
                    report.total() >= PER_RUN
                }
                Err(err) => {
                    tracing::warn!(error = %err, "ИИ-чат не разобрал обращения");
                    false
                }
            };
            if full || AGAIN.load(Ordering::SeqCst) {
                continue;
            }
            RUNNING.store(false, Ordering::SeqCst);
            if !AGAIN.load(Ordering::SeqCst) || RUNNING.swap(true, Ordering::SeqCst) {
                break;
            }
        }
    });
}

fn number_error(
    config: &ModuleConfig,
    key: &str,
    label: &str,
    range: (i64, i64),
) -> Option<String> {
    let value = config.get(key)?;
    let text = match value {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) if s.trim().is_empty() => return None,
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Null => return None,
        _ => return Some(format!("«{label}» — целое число")),
    };
    match text.parse::<i64>() {
        Ok(n) if (range.0..=range.1).contains(&n) => None,
        Ok(_) => Some(format!("«{label}» — от {} до {}", range.0, range.1)),
        Err(_) => Some(format!("«{label}» — целое число")),
    }
}

pub struct AiModule;

#[async_trait]
impl Module for AiModule {
    fn id(&self) -> ModuleId {
        ModuleId::Ai
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::secret(
                "api_key",
                "API-ключ Groq",
                "console.groq.com → API Keys. Ключ хранится зашифрованным.",
            ),
            SettingField::select(
                "model",
                "Модель",
                "gpt-oss-120b отвечает точнее, gpt-oss-20b — быстрее и дешевле, qwen3.8-27b — предварительная модель Groq.",
                &MODELS,
            )
            .with_default(MODELS[0]),
            SettingField::text(
                "instructions",
                "Инструкция и частые вопросы",
                "Как говорить с клиентами и что отвечать на частые вопросы. Цены, сроки и пробный период ИИ берёт из тарифов сам. До 8000 знаков.",
            )
            .optional()
            .large(),
            SettingField::flag(
                "cabinet",
                "Отвечать в кабинете",
                "Обращения из раздела «Помощь» на сайте.",
            )
            .with_default("true"),
            SettingField::flag(
                "miniapp",
                "Отвечать в мини-приложении",
                "В мини-приложении появится чат поддержки вместо перехода в бота.",
            )
            .with_default("true"),
            SettingField::number(
                "max_tokens",
                "Токенов на ответ",
                "Сколько токенов модель может потратить на один ответ вместе с рассуждением. От 200 до 8000.",
            )
            .with_default("1000"),
            SettingField::number(
                "client_daily",
                "Ответов одному клиенту в сутки",
                "Дальше до конца суток клиенту отвечает оператор.",
            )
            .with_default("30"),
            SettingField::number(
                "daily_tokens",
                "Дневной бюджет, токенов",
                "На всех клиентов за сутки по часам панели Remnawave. Когда бюджет кончится, новые вопросы до конца суток получает оператор.",
            )
            .with_default("1000000"),
            SettingField::url(
                "api_url",
                "Адрес API",
                "Оставьте пустым. Нужен, только если Groq недоступен с сервера панели и запросы идут через ваш прокси.",
            )
            .optional(),
        ]
    }

    fn validate_config(&self, config: &ModuleConfig) -> Result<(), ModuleError> {
        let schema = self.settings_schema();
        let mut problems: Vec<String> = sn_domain::settings::missing_required(&schema, &config.0)
            .into_iter()
            .map(|key| {
                let label = schema
                    .iter()
                    .find(|f| f.key == key)
                    .map_or(key, |f| f.label);
                format!("не заполнено поле «{label}»")
            })
            .collect();
        for (key, label, range) in [
            ("max_tokens", "Токенов на ответ", MAX_TOKENS_RANGE),
            (
                "client_daily",
                "Ответов одному клиенту в сутки",
                CLIENT_DAILY_RANGE,
            ),
            (
                "daily_tokens",
                "Дневной бюджет, токенов",
                DAILY_TOKENS_RANGE,
            ),
        ] {
            problems.extend(number_error(config, key, label, range));
        }
        if config
            .text("instructions")
            .is_some_and(|text| text.chars().count() > INSTRUCTIONS_MAX)
        {
            problems.push(format!("инструкция длиннее {INSTRUCTIONS_MAX} знаков"));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(ModuleError::Invalid(problems.join("; ")))
        }
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: ANSWER_TASK,
            every: Duration::from_secs(60),
        }]
    }

    fn cleanup_tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: RELEASE_TASK,
            every: Duration::from_secs(120),
        }]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        match task {
            ANSWER_TASK => {
                wake(ctx);
                Ok(())
            }
            RELEASE_TASK => {
                if !ctx.core.module_enabled(ModuleId::Ai) {
                    let mut conn = ctx.store.conn().await?;
                    let released = store::release_all(&mut conn).await?;
                    if released > 0 {
                        tracing::info!(
                            released,
                            "ИИ-чат выключен, его обращения отданы операторам"
                        );
                    }
                }
                Ok(())
            }
            other => Err(ModuleError::Invalid(format!("у модуля нет задачи {other}"))),
        }
    }
}
