use crate::BotConfig;
use crate::gram_wallet;
use crate::telegram::{Button, Keyboard, TelegramApi, TelegramError};
use crate::text::{self, escape};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use sn_domain::bot_content::Content;
use sn_domain::bot_content::Value::{self, Html, Text};
use sn_domain::client::IdentityKind;
use sn_domain::format;
use sn_domain::ids::ClientId;
use sn_domain::jobs::{JobPayload, retry_delay};
use sn_domain::money::Money;
use sn_modules::MailSite;
use sn_modules::catalog::broadcast::{ButtonSpec, letter_for};
use sn_modules::catalog::{contests, gifts, limits, referral};
use sn_services::Services;
use sn_services::mail::{RichLetter, Undelivered};
use sn_store::{Db, StoreError};
use sn_worker::{CoreHandlers, JobHandler, JobResult};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

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
    pub const REFUNDED_BLOCKED: &str = sn_services::inbox::BLOCKED_REFUND_TEMPLATE;
    pub const AUTORENEW_AFTER_OFF: &str = sn_services::autorenew::CHARGED_AFTER_OFF_TEMPLATE;
    pub const PAYMENT_CREDITED: &str = sn_services::inbox::PAYMENT_CREDITED_TEMPLATE;
    pub const PAYMENT_REVIEW: &str = sn_services::inbox::PAYMENT_REVIEW_TEMPLATE;
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

fn keyboard(content: &Content, button: Option<(&str, &str)>) -> Keyboard {
    let Some((key, data)) = button else {
        return Keyboard::new();
    };
    match content.button(key, &[]) {
        Some(label) => Keyboard::new().row(vec![
            Button::callback(label.text, data)
                .look(label.emoji_id, label.style.map(|s| s.as_str())),
        ]),
        None => Keyboard::new(),
    }
}

pub fn render(
    content: &Content,
    template: &str,
    params: &serde_json::Value,
    offset: FixedOffset,
) -> Option<Rendered> {
    let template = canonical(template, params);
    let amount = param(params, "amount");
    let date =
        date_param(params, "date", offset).or_else(|| date_param(params, "expire_at", offset));
    let t = |key: &str, vars: &[(&str, Value<'_>)]| content.text(key, vars);
    let reply = match params.get("ticket_id").and_then(serde_json::Value::as_i64) {
        Some(id) if id > 0 => format!("sup:{id}"),
        _ => "sup".to_string(),
    };
    let (text, button): (String, Option<(&str, &str)>) = match template {
        templates::PAYMENT_PAID => (
            match &amount {
                Some(amount) => t("notify.payment_paid", &[("amount", Html(amount))]),
                None => t("notify.payment_paid_plain", &[]),
            },
            Some(("notify.btn.subscription", "subs")),
        ),
        templates::TOPUP_DONE => (
            match &amount {
                Some(amount) => t("notify.topup_done", &[("amount", Html(amount))]),
                None => t("notify.topup_done_plain", &[]),
            },
            Some(("notify.btn.balance", "bal")),
        ),
        templates::SUBSCRIPTION_READY => (
            match &date {
                Some(date) => t("notify.subscription_ready", &[("date", Text(date))]),
                None => t("notify.subscription_ready_plain", &[]),
            },
            Some(("notify.btn.subscription", "subs")),
        ),
        templates::SUBSCRIPTION_RENEWED => (
            match &date {
                Some(date) => t("notify.subscription_renewed", &[("date", Text(date))]),
                None => t("notify.subscription_renewed_plain", &[]),
            },
            Some(("notify.btn.subscription", "subs")),
        ),
        templates::SUBSCRIPTION_EXPIRING => (
            match &date {
                Some(date) => t("notify.subscription_expiring", &[("date", Text(date))]),
                None => t("notify.subscription_expiring_plain", &[]),
            },
            Some(("notify.btn.extend", "buy")),
        ),
        templates::AUTORENEW_UPCOMING => (
            match (&date, &amount) {
                (Some(date), Some(amount)) => t(
                    "notify.autorenew_upcoming",
                    &[("date", Text(date)), ("amount", Html(amount))],
                ),
                _ => t("notify.autorenew_upcoming_plain", &[]),
            },
            Some(("notify.btn.balance", "bal")),
        ),
        templates::AUTORENEW_FAILED if params["balance_off"].as_bool() == Some(true) => (
            match &date {
                Some(date) => t("notify.autorenew_pay", &[("date", Text(date))]),
                None => t("notify.autorenew_pay_plain", &[]),
            },
            Some(("notify.btn.extend", "buy")),
        ),
        templates::AUTORENEW_FAILED => (
            match param(params, "missing") {
                Some(missing) => t("notify.autorenew_failed", &[("missing", Html(&missing))]),
                None => t("notify.autorenew_failed_plain", &[]),
            },
            Some(("notify.btn.balance", "bal")),
        ),
        templates::AUTORENEW_DISABLED => (
            t("notify.autorenew_disabled", &[]),
            Some(("notify.btn.extend", "buy")),
        ),
        templates::AUTORENEW_AFTER_OFF => (
            match (&amount, &date) {
                (Some(amount), Some(date)) => t(
                    "notify.autorenew_after_off",
                    &[("amount", Html(amount)), ("date", Text(date))],
                ),
                _ => t("notify.autorenew_after_off_plain", &[]),
            },
            Some(("notify.btn.support", "sup")),
        ),
        templates::REFUNDED_BLOCKED => (
            match &amount {
                Some(amount) => t(
                    "notify.payment_refunded_blocked",
                    &[("amount", Html(amount))],
                ),
                None => t("notify.payment_refunded_blocked_plain", &[]),
            },
            None,
        ),
        templates::SUPPORT_REPLY => (
            t(
                "notify.support_reply",
                &[("body", Html(&param(params, "body")?))],
            ),
            Some(("notify.btn.reply", reply.as_str())),
        ),
        templates::REPEAT_CREDITED | templates::PAYMENT_CREDITED => {
            (param(params, "text")?, Some(("notify.btn.balance", "bal")))
        }
        templates::REPEAT_REVIEW | templates::PAYMENT_REVIEW => {
            (param(params, "text")?, Some(("notify.btn.support", "sup")))
        }
        gifts::TEMPLATE_READY => {
            let tariff = param(params, "tariff")?;
            let term = param(params, "term")?;
            let code = param(params, "code")?;
            let mut lines = vec![t(
                "notify.gift_ready",
                &[
                    ("tariff", Html(&tariff)),
                    ("term", Html(&term)),
                    ("code", Html(&code)),
                ],
            )];
            if let Some(link) = param(params, "bot_link") {
                lines.push(t("notify.gift_ready_link", &[("link", Html(&link))]));
            }
            if let Some(url) = param(params, "cabinet_link") {
                lines.push(t("notify.gift_ready_cabinet", &[("url", Html(&url))]));
            }
            if let Some(until) = date_param(params, "expire_at", offset) {
                lines.push(t("notify.gift_ready_until", &[("date", Text(&until))]));
            }
            (
                lines
                    .into_iter()
                    .filter(|line| !line.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
                Some(("notify.btn.gifts", "gl")),
            )
        }
        gifts::TEMPLATE_REDEEMED => (
            t(
                "notify.gift_redeemed",
                &[
                    ("code", Html(&param(params, "code")?)),
                    ("tariff", Html(&param(params, "tariff")?)),
                    ("term", Html(&param(params, "term")?)),
                ],
            ),
            Some(("notify.btn.gifts", "gl")),
        ),
        referral::TEMPLATE_CREDIT => (
            t(
                "notify.referral_credit",
                &[("amount", Html(&amount.clone()?))],
            ),
            Some(("notify.btn.referral", "x:referral")),
        ),
        limits::TEMPLATE_WARNING => (
            t(
                "notify.traffic_warning",
                &[
                    ("when", Text(params.get("when")?.as_str()?)),
                    ("used", Html(&param(params, "used")?)),
                    ("limit", Html(&param(params, "limit")?)),
                    ("per", Text(params.get("per")?.as_str()?)),
                ],
            ),
            Some(("notify.btn.subscription", "subs")),
        ),
        limits::TEMPLATE_RESTRICTED => (
            t(
                "notify.traffic_restricted",
                &[
                    ("when", Text(params.get("when")?.as_str()?)),
                    ("used", Html(&param(params, "used")?)),
                    ("limit", Html(&param(params, "limit")?)),
                    ("per", Text(params.get("per")?.as_str()?)),
                    ("until", Html(&param(params, "until")?)),
                ],
            ),
            Some(("notify.btn.subscription", "subs")),
        ),
        limits::TEMPLATE_LIFTED => (
            t("notify.traffic_lifted", &[]),
            Some(("notify.btn.subscription", "subs")),
        ),
        contests::TEMPLATE_WON => {
            let mut text = t(
                "notify.contest_won",
                &[
                    ("title", Html(&param(params, "title")?)),
                    ("prize", Html(&param(params, "prize")?)),
                    ("result", Html(&param(params, "note").unwrap_or_default())),
                ],
            );
            if let Some(code) = param(params, "code") {
                text.push('\n');
                text.push_str(&t("notify.contest_won_code", &[("code", Html(&code))]));
            }
            (text, Some(("notify.btn.contests", "x:contests")))
        }
        contests::TEMPLATE_WAITING => (
            t(
                "notify.contest_waiting",
                &[
                    ("title", Html(&param(params, "title")?)),
                    ("prize", Html(&param(params, "prize")?)),
                ],
            ),
            Some(("notify.btn.contests", "x:contests")),
        ),
        referral::TEMPLATE_WITHDRAWAL_PAID => (
            t(
                "notify.withdrawal_paid",
                &[("amount", Html(&amount.clone()?))],
            ),
            None,
        ),
        referral::TEMPLATE_WITHDRAWAL_REJECTED => (
            match param(params, "comment") {
                Some(comment) => t(
                    "notify.withdrawal_rejected",
                    &[
                        ("amount", Html(&amount.clone()?)),
                        ("comment", Html(&comment)),
                    ],
                ),
                None => t(
                    "notify.withdrawal_rejected_plain",
                    &[("amount", Html(&amount.clone()?))],
                ),
            },
            Some(("notify.btn.referral", "x:referral")),
        ),
        _ => (param(params, "text")?, None),
    };
    Some(Rendered {
        text,
        keyboard: keyboard(content, button),
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
        let chat = identities
            .iter()
            .find(|i| i.kind == IdentityKind::Telegram)
            .and_then(|i| i.value.parse::<i64>().ok())
            .filter(|_| !client.bot_unreachable);
        let Some(chat_id) = chat else {
            drop(conn);
            return match identities.iter().find(|i| i.kind == IdentityKind::Email) {
                Some(email) => mail_notice(services, &email.value, template, params, attempt).await,
                None => {
                    tracing::debug!(client = %client_id, template, "клиенту не написать ни в Telegram, ни на почту");
                    JobResult::Done
                }
            };
        };
        drop(conn);
        let Some(token) = services.bot_token() else {
            return retry("токен бота не задан".into());
        };
        let sent = if template == gram_wallet::TEMPLATE {
            let Some(payment) = params
                .get("payment_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
            else {
                return JobResult::Fail("в задании нет счёта".into());
            };
            let widget = match gram_wallet::compose_for(services, payment).await {
                Ok(Some(widget)) => widget,
                Ok(None) => return JobResult::Done,
                Err(err) => return retry(err.to_string()),
            };
            self.api(&token)
                .send_widget(chat_id, &widget)
                .await
                .map(|_| ())
        } else {
            let content = services.bot_content();
            let Some(rendered) = render(
                &content.content,
                template,
                params,
                services.settings().remna_offset(),
            ) else {
                return JobResult::Fail(format!("уведомление «{template}» без нужных параметров"));
            };
            self.api(&token)
                .send_message(chat_id, &rendered.text, Some(&rendered.keyboard))
                .await
                .map(|_| ())
        };
        match sent {
            Ok(()) => {
                also_mail(services, client_id, template, params, attempt).await;
                JobResult::Done
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetterKind {
    Expiring,
    Payments,
    Support,
    Service,
}

pub struct LetterHead {
    pub kind: LetterKind,
    pub subject: String,
    pub button: &'static str,
    pub path: String,
}

pub fn letter_head(template: &str, params: &serde_json::Value) -> Option<LetterHead> {
    let ticket = params
        .get("ticket_id")
        .and_then(serde_json::Value::as_i64)
        .filter(|id| *id > 0);
    let head = |kind, subject: &str, button, path: &str| LetterHead {
        kind,
        subject: subject.to_string(),
        button,
        path: path.to_string(),
    };
    Some(match canonical(template, params) {
        templates::PAYMENT_PAID => head(
            LetterKind::Payments,
            "Оплата получена",
            "Открыть кабинет",
            "/cabinet",
        ),
        templates::TOPUP_DONE => head(
            LetterKind::Payments,
            "Баланс пополнен",
            "История оплат",
            "/cabinet/payments",
        ),
        templates::PAYMENT_CREDITED => head(
            LetterKind::Payments,
            "Деньги зачислены на баланс",
            "История оплат",
            "/cabinet/payments",
        ),
        templates::PAYMENT_REVIEW => head(
            LetterKind::Payments,
            "Оплату проверим вручную",
            "История оплат",
            "/cabinet/payments",
        ),
        templates::SUBSCRIPTION_READY => head(
            LetterKind::Payments,
            "Подписка готова",
            "Открыть кабинет",
            "/cabinet",
        ),
        templates::SUBSCRIPTION_RENEWED => head(
            LetterKind::Payments,
            "Подписка продлена",
            "Открыть кабинет",
            "/cabinet",
        ),
        templates::SUBSCRIPTION_EXPIRING => head(
            LetterKind::Expiring,
            "Подписка скоро закончится",
            "Продлить",
            "/cabinet/pay",
        ),
        templates::AUTORENEW_UPCOMING => head(
            LetterKind::Expiring,
            "Скоро автопродление",
            "Пополнить баланс",
            "/cabinet/topup",
        ),
        templates::AUTORENEW_FAILED => head(
            LetterKind::Expiring,
            "Подписку не удалось продлить",
            "Продлить",
            "/cabinet/pay",
        ),
        templates::AUTORENEW_DISABLED => head(
            LetterKind::Expiring,
            "Автопродление выключено",
            "Продлить",
            "/cabinet/pay",
        ),
        templates::AUTORENEW_AFTER_OFF => head(
            LetterKind::Payments,
            "Списание прошло до выключения автопродления",
            "Написать в поддержку",
            "/cabinet/help",
        ),
        templates::REFUNDED_BLOCKED => head(
            LetterKind::Payments,
            "Оплата возвращена",
            "Открыть сайт",
            "/",
        ),
        gifts::TEMPLATE_READY => head(
            LetterKind::Payments,
            "Подарок оплачен",
            "Мои подарки",
            "/cabinet/gifts",
        ),
        gifts::TEMPLATE_REDEEMED => head(
            LetterKind::Payments,
            "Ваш подарок активировали",
            "Мои подарки",
            "/cabinet/gifts",
        ),
        limits::TEMPLATE_WARNING => head(
            LetterKind::Service,
            "Превышен лимит трафика",
            "Открыть кабинет",
            "/cabinet",
        ),
        limits::TEMPLATE_RESTRICTED => head(
            LetterKind::Service,
            "Доступ временно ограничен",
            "Открыть кабинет",
            "/cabinet",
        ),
        limits::TEMPLATE_LIFTED => head(
            LetterKind::Service,
            "Ограничение по трафику снято",
            "Открыть кабинет",
            "/cabinet",
        ),
        templates::SUPPORT_REPLY => LetterHead {
            kind: LetterKind::Support,
            subject: match ticket {
                Some(id) => format!("Ответ поддержки по обращению №{id}"),
                None => "Ответ поддержки".to_string(),
            },
            button: "Открыть переписку",
            path: match ticket {
                Some(id) => format!("/cabinet/help/{id}"),
                None => "/cabinet/help".to_string(),
            },
        },
        _ => return None,
    })
}

async fn mail_notice(
    services: &Services,
    email: &str,
    template: &str,
    params: &serde_json::Value,
    attempt: u32,
) -> JobResult {
    let mail = services.mail();
    let Some(letters) = mail.letters() else {
        return JobResult::Done;
    };
    let Some(head) = letter_head(template, params) else {
        return JobResult::Done;
    };
    let wanted = match head.kind {
        LetterKind::Expiring => letters.expiring,
        LetterKind::Payments => letters.payments,
        LetterKind::Support => letters.support,
        LetterKind::Service => true,
    };
    if !wanted {
        return JobResult::Done;
    }
    let settings = services.settings();
    let content = services.bot_content();
    let Some(rendered) = render(&content.content, template, params, settings.remna_offset()) else {
        return JobResult::Fail(format!("уведомление «{template}» без нужных параметров"));
    };
    let base = settings
        .public_url
        .as_deref()
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"));
    let buttons: Vec<ButtonSpec> = base
        .iter()
        .map(|base| ButtonSpec {
            text: head.button.to_string(),
            url: Some(format!("{base}{}", head.path)),
            section: None,
        })
        .collect();
    let site = MailSite {
        base_url: base.clone().unwrap_or_default(),
        brand: settings.brand_name.clone(),
    };
    let letter = letter_for(&head.subject, &rendered.text, None, &buttons, &site);
    let rich = RichLetter {
        to: email.to_string(),
        subject: head.subject.clone(),
        text: letter.notice_text(),
        html: letter.notice_html(),
        unsubscribe: None,
    };
    match mail.deliver(&rich).await {
        Ok(()) => JobResult::Done,
        Err(Undelivered::Off(_)) => JobResult::Done,
        Err(Undelivered::Address(reason)) => {
            tracing::info!(template, reason = %reason, "письмо клиенту не отправлено: адрес не принят");
            JobResult::Done
        }
        Err(Undelivered::Limited(wait)) => JobResult::Retry {
            after: chrono::Duration::from_std(wait)
                .unwrap_or_else(|_| chrono::Duration::seconds(60)),
            error: "почтовый сервис просит подождать".into(),
        },
        Err(Undelivered::Failed(error)) => JobResult::Retry {
            after: retry_delay(attempt),
            error,
        },
    }
}

async fn also_mail(
    services: &Services,
    client_id: ClientId,
    template: &str,
    params: &serde_json::Value,
    attempt: u32,
) {
    if params.get("also_mail").and_then(serde_json::Value::as_bool) != Some(true) {
        return;
    }
    let email = match services.store().conn().await {
        Ok(mut conn) => match conn.clients().identities(client_id).await {
            Ok(identities) => identities
                .into_iter()
                .find(|i| i.kind == IdentityKind::Email)
                .map(|i| i.value),
            Err(err) => {
                tracing::warn!(client = %client_id, error = %err, "почта клиента для копии уведомления не прочитана");
                None
            }
        },
        Err(err) => {
            tracing::warn!(client = %client_id, error = %err, "почта клиента для копии уведомления не прочитана");
            None
        }
    };
    let Some(email) = email else {
        return;
    };
    if let JobResult::Retry { error, .. } | JobResult::Fail(error) =
        mail_notice(services, &email, template, params, attempt).await
    {
        tracing::warn!(client = %client_id, template, error = %error, "копия уведомления на почту не отправлена");
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
