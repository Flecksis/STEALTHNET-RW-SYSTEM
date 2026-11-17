use crate::BotConfig;
use crate::telegram::{Keyboard, TelegramApi, TelegramError};
use crate::text::{self, escape};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use sn_domain::client::IdentityKind;
use sn_domain::format;
use sn_domain::ids::ClientId;
use sn_domain::jobs::{JobPayload, retry_delay};
use sn_domain::money::Money;
use sn_modules::catalog::referral;
use sn_services::Services;
use sn_store::{Db, StoreError};
use sn_worker::{CoreHandlers, JobHandler, JobResult};
use std::sync::{Arc, Mutex};

pub mod templates {
    pub const TEXT: &str = "text";
    pub const PAYMENT_PAID: &str = "payment_paid";
    pub const TOPUP_DONE: &str = "topup_done";
    pub const SUBSCRIPTION_READY: &str = "subscription_ready";
    pub const SUBSCRIPTION_RENEWED: &str = "subscription_renewed";
    pub const SUBSCRIPTION_EXPIRING: &str = "subscription_expiring";
    pub const AUTORENEW_UPCOMING: &str = "autorenew_upcoming";
    pub const AUTORENEW_FAILED: &str = "autorenew_failed";
    pub const AUTORENEW_DISABLED: &str = "autorenew_disabled";
    pub const SUPPORT_REPLY: &str = "support_reply";
    pub const ORDER_FULFILLED: &str = "order.fulfilled";
    pub const AUTORENEW_REMINDER: &str = "autorenew.reminder";
    pub const AUTORENEW_CHARGE_FAILED: &str = "autorenew.failed";
    pub const AUTORENEW_TURNED_OFF: &str = "autorenew.disabled";
    pub const REFERRAL_CREDIT: &str = sn_modules::catalog::referral::TEMPLATE_CREDIT;
    pub const WITHDRAWAL_PAID: &str = sn_modules::catalog::referral::TEMPLATE_WITHDRAWAL_PAID;
    pub const WITHDRAWAL_REJECTED: &str =
        sn_modules::catalog::referral::TEMPLATE_WITHDRAWAL_REJECTED;
    pub const REPEAT_CREDITED: &str = sn_services::inbox::REPEAT_CREDITED_TEMPLATE;
    pub const REPEAT_REVIEW: &str = sn_services::inbox::REPEAT_REVIEW_TEMPLATE;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub keyboard: Keyboard,
}

fn param(params: &serde_json::Value, key: &str) -> Option<String> {
    match params.get(key)? {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(escape(s.trim())),
        serde_json::Value::Number(n) => Some(n.to_string()),
        value @ serde_json::Value::Object(_) => serde_json::from_value::<Money>(value.clone())
            .ok()
            .map(|money| escape(&format::money(money))),
        _ => None,
    }
}

fn canonical<'a>(template: &'a str, params: &serde_json::Value) -> &'a str {
    match template {
        templates::ORDER_FULFILLED => {
            match params.get("order_kind").and_then(serde_json::Value::as_str) {
                Some("renewal") => templates::SUBSCRIPTION_RENEWED,
                _ => templates::SUBSCRIPTION_READY,
            }
        }
        templates::AUTORENEW_REMINDER | templates::AUTORENEW_CHARGE_FAILED => {
            templates::AUTORENEW_FAILED
        }
        templates::AUTORENEW_TURNED_OFF => templates::AUTORENEW_DISABLED,
        other => other,
    }
}

fn date_param(params: &serde_json::Value, key: &str, offset: FixedOffset) -> Option<String> {
    let raw = params.get(key)?.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|at| text::date(at.with_timezone(&Utc), offset))
}

fn keyboard(button: Option<(&str, &str)>) -> Keyboard {
    match button {
        Some((label, data)) => Keyboard::new().button(label, data),
        None => Keyboard::new(),
    }
}

pub fn render(template: &str, params: &serde_json::Value, offset: FixedOffset) -> Option<Rendered> {
    let template = canonical(template, params);
    let amount = param(params, "amount");
    let date =
        date_param(params, "date", offset).or_else(|| date_param(params, "expire_at", offset));
    let (text, button): (String, Option<(&str, &str)>) = match template {
        templates::PAYMENT_PAID => (
            match &amount {
                Some(amount) => {
                    format!("Оплата {amount} получена. Подписка обновится в течение минуты.")
                }
                None => "Оплата получена. Подписка обновится в течение минуты.".into(),
            },
            Some(("Подписка", "subs")),
        ),
        templates::TOPUP_DONE => (
            match &amount {
                Some(amount) => format!("Баланс пополнен на {amount}."),
                None => "Баланс пополнен.".into(),
            },
            Some(("Баланс", "bal")),
        ),
        templates::SUBSCRIPTION_READY => (
            match &date {
                Some(date) => {
                    format!("Подписка готова и действует до {date}. Ссылка — в разделе «Подписка».")
                }
                None => "Подписка готова. Ссылка — в разделе «Подписка».".into(),
            },
            Some(("Подписка", "subs")),
        ),
        templates::SUBSCRIPTION_RENEWED => (
            match &date {
                Some(date) => format!("Подписка продлена до {date}."),
                None => "Подписка продлена.".into(),
            },
            Some(("Подписка", "subs")),
        ),
        templates::SUBSCRIPTION_EXPIRING => (
            match &date {
                Some(date) => {
                    format!("Подписка закончится {date}. Продлите её, чтобы VPN не отключился.")
                }
                None => "Подписка скоро закончится. Продлите её, чтобы VPN не отключился.".into(),
            },
            Some(("Продлить", "buy")),
        ),
        templates::AUTORENEW_UPCOMING => (
            match (&date, &amount) {
                (Some(date), Some(amount)) => format!(
                    "{date} спишем {amount} за продление подписки. Если на балансе не хватит, пополните его заранее."
                ),
                _ => "Скоро продлим подписку. Если на балансе не хватит, пополните его заранее."
                    .into(),
            },
            Some(("Баланс", "bal")),
        ),
        templates::AUTORENEW_FAILED => (
            match param(params, "missing") {
                Some(missing) => format!(
                    "Не получилось продлить подписку: не хватает {missing}. Пополните баланс — продлим автоматически."
                ),
                None => {
                    "Не получилось продлить подписку. Пополните баланс — продлим автоматически."
                        .into()
                }
            },
            Some(("Баланс", "bal")),
        ),
        templates::AUTORENEW_DISABLED => (
            "Автопродление выключено: оплатить не удалось. Продлите подписку вручную.".into(),
            Some(("Продлить", "buy")),
        ),
        templates::SUPPORT_REPLY => (
            format!("<b>Ответ поддержки</b>\n\n{}", param(params, "body")?),
            Some(("Ответить", "sup")),
        ),
        templates::REPEAT_CREDITED => (param(params, "text")?, Some(("Баланс", "bal"))),
        templates::REPEAT_REVIEW => (param(params, "text")?, Some(("Поддержка", "sup"))),
        referral::TEMPLATE_CREDIT => (
            format!(
                "Начислено {} за оплату приглашённого друга.",
                amount.clone()?
            ),
            Some(("Пригласить друга", "x:referral")),
        ),
        referral::TEMPLATE_WITHDRAWAL_PAID => (
            format!("Заявка на вывод {} выплачена.", amount.clone()?),
            None,
        ),
        referral::TEMPLATE_WITHDRAWAL_REJECTED => (
            match param(params, "comment") {
                Some(comment) => format!(
                    "Заявка на вывод {} отклонена: {comment}. Деньги вернулись на реферальный баланс.",
                    amount.clone()?
                ),
                None => format!(
                    "Заявка на вывод {} отклонена. Деньги вернулись на реферальный баланс.",
                    amount.clone()?
                ),
            },
            Some(("Пригласить друга", "x:referral")),
        ),
        _ => (param(params, "text")?, None),
    };
    Some(Rendered {
        text,
        keyboard: keyboard(button),
    })
}

pub struct NotifyHandler {
    api_base: String,
    cache: Mutex<Option<(String, Arc<TelegramApi>)>>,
}

impl Default for NotifyHandler {
    fn default() -> Self {
        Self::new(&BotConfig::default())
    }
}

impl NotifyHandler {
    pub fn new(config: &BotConfig) -> Self {
        NotifyHandler {
            api_base: config.api_base.clone(),
            cache: Mutex::new(None),
        }
    }

    fn api(&self, token: &str) -> Arc<TelegramApi> {
        let mut cache = match self.cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((cached, api)) = cache.as_ref()
            && cached == token
        {
            return api.clone();
        }
        let api = Arc::new(TelegramApi::new(&self.api_base, token));
        *cache = Some((token.to_string(), api.clone()));
        api
    }

    pub async fn deliver(
        &self,
        services: &Services,
        client_id: ClientId,
        template: &str,
        params: &serde_json::Value,
        attempt: u32,
    ) -> JobResult {
        let retry = |error: String| JobResult::Retry {
            after: retry_delay(attempt),
            error,
        };
        let Some(token) = services.bot_token() else {
            return retry("токен бота не задан".into());
        };
        let mut conn = match services.store().conn().await {
            Ok(conn) => conn,
            Err(err) => return retry(err.to_string()),
        };
        let client = match conn.clients().get(client_id).await {
            Ok(client) => client,
            Err(StoreError::NotFound(_)) => return JobResult::Fail("клиент не найден".into()),
            Err(err) => return retry(err.to_string()),
        };
        let identities = match conn.clients().identities(client_id).await {
            Ok(identities) => identities,
            Err(err) => return retry(err.to_string()),
        };
        let Some(chat_id) = identities
            .iter()
            .find(|i| i.kind == IdentityKind::Telegram)
            .and_then(|i| i.value.parse::<i64>().ok())
        else {
            tracing::debug!(client = %client_id, template, "у клиента нет Telegram, уведомление пропущено");
            return JobResult::Done;
        };
        if client.bot_unreachable {
            return JobResult::Done;
        }
        let Some(rendered) = render(template, params, services.settings().remna_offset()) else {
            return JobResult::Fail(format!("уведомление «{template}» без нужных параметров"));
        };
        drop(conn);
        match self
            .api(&token)
            .send_message(chat_id, &rendered.text, Some(&rendered.keyboard))
            .await
        {
            Ok(_) => JobResult::Done,
            Err(err) if err.is_forbidden() || chat_missing(&err) => {
                match services.store().conn().await {
                    Ok(mut conn) => {
                        if let Err(store) =
                            conn.clients().set_bot_unreachable(client_id, true).await
                        {
                            return retry(store.to_string());
                        }
                    }
                    Err(store) => return retry(store.to_string()),
                }
                tracing::info!(client = %client_id, "клиент закрыл бота, уведомления остановлены");
                JobResult::Done
            }
            Err(err) => match err.retry_after() {
                Some(seconds) => JobResult::Retry {
                    after: chrono::Duration::seconds(i64::try_from(seconds).unwrap_or(60)),
                    error: err.to_string(),
                },
                None if err.is_retryable() => retry(err.to_string()),
                None => JobResult::Fail(err.to_string()),
            },
        }
    }
}

fn chat_missing(err: &TelegramError) -> bool {
    matches!(err, TelegramError::Api { code: 400, description, .. } if description.contains("chat not found"))
}

#[async_trait]
impl JobHandler for NotifyHandler {
    async fn handle(&self, services: &Services, payload: &JobPayload, attempt: u32) -> JobResult {
        match payload {
            JobPayload::Notify {
                client_id,
                template,
                params,
            } => {
                self.deliver(services, *client_id, template, params, attempt)
                    .await
            }
            other => CoreHandlers.handle(services, other, attempt).await,
        }
    }
}
