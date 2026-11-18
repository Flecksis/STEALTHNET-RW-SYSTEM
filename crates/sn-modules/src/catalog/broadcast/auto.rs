use super::auto_store::{self, Bounds, NewCase, Probe, RuleRow, Tally};
use super::store::{self, ImageMeta, Problem, Progress};
use super::{
    BroadcastError, ButtonSpec, Channel, DAYS_MAX, Lane, clean_buttons, clean_subject, clean_text,
    invalid, mail_site, rejected,
};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::format::plural_ru;
use sn_domain::ids::{AdminId, ClientId};
use sn_store::{ActorKind, AuditEntry, Db, StoreError};
use std::collections::HashSet;

pub const AUTO_TASK: &str = "auto";
pub const AUTO_EVERY_SECS: u64 = 300;
pub const CATCH_UP_HOURS: i64 = 24;
pub const RULE_ENTITY: &str = "broadcast_rule";
pub const RULES_MAX: usize = 50;
const BATCH: i64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Expiring,
    Expired,
    TrialEnded,
    NoPurchase,
    NotConnected,
}

impl Trigger {
    pub const ALL: [Trigger; 5] = [
        Trigger::Expiring,
        Trigger::Expired,
        Trigger::TrialEnded,
        Trigger::NoPurchase,
        Trigger::NotConnected,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Trigger::Expiring => "expiring",
            Trigger::Expired => "expired",
            Trigger::TrialEnded => "trial_ended",
            Trigger::NoPurchase => "no_purchase",
            Trigger::NotConnected => "not_connected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Trigger::ALL.into_iter().find(|t| t.as_str() == value)
    }

    pub const fn title(self) -> &'static str {
        match self {
            Trigger::Expiring => "Подписка скоро закончится",
            Trigger::Expired => "Подписка закончилась и не продлена",
            Trigger::TrialEnded => "Пробный период закончился, покупки нет",
            Trigger::NoPurchase => "Нет покупок после регистрации",
            Trigger::NotConnected => "Нет подключений после покупки",
        }
    }

    pub const fn min_days(self) -> u16 {
        match self {
            Trigger::Expired | Trigger::TrialEnded => 0,
            Trigger::Expiring | Trigger::NoPurchase | Trigger::NotConnected => 1,
        }
    }

    pub fn timing(self, days: u16) -> String {
        let n = u64::from(days);
        let span = format!("{n} {}", plural_ru(n, "день", "дня", "дней"));
        match self {
            Trigger::Expiring => format!("за {span} до окончания"),
            Trigger::Expired | Trigger::TrialEnded if days == 0 => "сразу после окончания".into(),
            Trigger::Expired | Trigger::TrialEnded => format!("через {span} после окончания"),
            Trigger::NoPurchase => format!("через {span} после регистрации"),
            Trigger::NotConnected => format!("через {span} после покупки"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TriggerOption {
    pub key: Trigger,
    pub title: &'static str,
    pub min_days: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutoOptions {
    pub every_minutes: u64,
    pub catch_up_hours: i64,
    pub rules_max: usize,
    pub triggers: Vec<TriggerOption>,
}

pub fn options() -> AutoOptions {
    AutoOptions {
        every_minutes: AUTO_EVERY_SECS / 60,
        catch_up_hours: CATCH_UP_HOURS,
        rules_max: RULES_MAX,
        triggers: Trigger::ALL
            .into_iter()
            .map(|key| TriggerOption {
                key,
                title: key.title(),
                min_days: key.min_days(),
            })
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuleDraft {
    pub trigger: Trigger,
    pub days: u16,
    #[serde(default)]
    pub with_autorenew: bool,
    #[serde(default)]
    pub channel: Channel,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub image_id: Option<i64>,
    #[serde(default)]
    pub buttons: Vec<ButtonSpec>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleContent {
    pub trigger: Trigger,
    pub days: u16,
    pub with_autorenew: bool,
    pub channel: Channel,
    pub subject: Option<String>,
    pub text: String,
    pub image_id: Option<i64>,
    pub buttons: Vec<ButtonSpec>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RuleStats {
    pub cases: i64,
    pub unreachable: i64,
    pub progress: Progress,
    pub last_sent_at: Option<DateTime<Utc>>,
}

impl From<Tally> for RuleStats {
    fn from(tally: Tally) -> Self {
        RuleStats {
            cases: tally.cases,
            unreachable: tally.unreachable,
            progress: tally.progress,
            last_sent_at: tally.last_sent_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rule {
    pub id: i64,
    pub trigger: Trigger,
    pub days: u16,
    pub with_autorenew: bool,
    pub title: &'static str,
    pub timing: String,
    pub channel: Channel,
    pub subject: Option<String>,
    pub text: String,
    pub image: Option<ImageMeta>,
    pub buttons: Vec<ButtonSpec>,
    pub enabled: bool,
    pub notice: Option<String>,
    pub sending_notices: Vec<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub stats: RuleStats,
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleItem {
    pub id: i64,
    pub trigger: Trigger,
    pub days: u16,
    pub with_autorenew: bool,
    pub title: &'static str,
    pub timing: String,
    pub channel: Channel,
    pub preview: String,
    pub has_image: bool,
    pub buttons: usize,
    pub enabled: bool,
    pub notice: Option<String>,
    pub stats: RuleStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReachQuery {
    pub trigger: Trigger,
    pub days: u16,
    #[serde(default)]
    pub with_autorenew: bool,
    #[serde(default)]
    pub id: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AutoReach {
    pub cases: i64,
    pub recipients: i64,
    pub stopped: i64,
    pub emails: i64,
    pub unsubscribed: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CheckReport {
    pub rules: usize,
    pub cases: usize,
    pub queued: usize,
}

fn check_days(trigger: Trigger, days: u16) -> Result<u16, BroadcastError> {
    let min = trigger.min_days();
    if days < min || days > DAYS_MAX {
        return Err(invalid(format!("Когда: число дней от {min} до {DAYS_MAX}")));
    }
    Ok(days)
}

async fn cleaned(db: &mut impl Db, draft: &RuleDraft) -> Result<RuleContent, BroadcastError> {
    let days = check_days(draft.trigger, draft.days)?;
    if let Some(id) = draft.image_id
        && !store::image_exists(db, id).await?
    {
        return Err(rejected(
            "broadcast.image",
            "Картинка не найдена: загрузите её заново",
        ));
    }
    Ok(RuleContent {
        trigger: draft.trigger,
        days,
        with_autorenew: draft.with_autorenew && draft.trigger == Trigger::Expiring,
        channel: draft.channel,
        subject: clean_subject(draft.subject.as_deref(), draft.channel)?,
        text: clean_text(&draft.text, draft.image_id.is_some())?,
        image_id: draft.image_id,
        buttons: clean_buttons(&draft.buttons)?,
    })
}

fn ready(ctx: &ModuleContext, channel: Channel, enabled: bool) -> Result<(), BroadcastError> {
    if enabled && channel.has(Lane::Email) {
        mail_site(ctx)?;
    }
    Ok(())
}

fn audit(action: &str, admin: Option<AdminId>, id: i64, details: serde_json::Value) -> AuditEntry {
    AuditEntry {
        actor_kind: if admin.is_some() {
            ActorKind::Admin
        } else {
            ActorKind::System
        },
        actor_id: admin.map(|a| a.0),
        action: action.into(),
        entity_kind: Some(RULE_ENTITY.into()),
        entity_id: Some(id),
        details,
        ip: None,
    }
}

async fn view(db: &mut impl Db, id: i64) -> Result<Rule, BroadcastError> {
    let row = auto_store::row(db, id).await?;
    let parsed = auto_store::parse(&row)?;
    let image = match row.image_id {
        Some(image) => store::image_meta(db, image).await?,
        None => None,
    };
    let stats = auto_store::tallies(db, &[id])
        .await?
        .remove(&id)
        .unwrap_or_default()
        .into();
    let problems = auto_store::problems(db, id).await?;
    let sending_notices = auto_store::sending_notices(db, id).await?;
    Ok(Rule {
        id: row.id,
        trigger: parsed.trigger,
        days: parsed.days,
        with_autorenew: row.with_autorenew,
        title: parsed.trigger.title(),
        timing: parsed.trigger.timing(parsed.days),
        channel: parsed.channel,
        subject: row.subject,
        text: row.text,
        image,
        buttons: parsed.buttons,
        enabled: row.enabled,
        notice: row.notice,
        sending_notices,
        checked_at: row.checked_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        stats,
        problems,
    })
}

fn item(row: &RuleRow, tally: Tally) -> Result<RuleItem, BroadcastError> {
    let parsed = auto_store::parse(row)?;
    Ok(RuleItem {
        id: row.id,
        trigger: parsed.trigger,
        days: parsed.days,
        with_autorenew: row.with_autorenew,
        title: parsed.trigger.title(),
        timing: parsed.trigger.timing(parsed.days),
        channel: parsed.channel,
        preview: store::preview(&row.text),
        has_image: row.image_id.is_some(),
        buttons: parsed.buttons.len(),
        enabled: row.enabled,
        notice: row.notice.clone(),
        stats: tally.into(),
    })
}

pub async fn rules(ctx: &ModuleContext) -> Result<Vec<RuleItem>, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    let rows = auto_store::rows(&mut conn).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut tallies = auto_store::tallies(&mut conn, &ids).await?;
    rows.iter()
        .map(|row| item(row, tallies.remove(&row.id).unwrap_or_default()))
        .collect()
}

pub async fn rule(ctx: &ModuleContext, id: i64) -> Result<Rule, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    view(&mut conn, id).await
}

pub async fn create_rule(
    ctx: &ModuleContext,
    draft: &RuleDraft,
    admin: Option<AdminId>,
) -> Result<Rule, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let content = cleaned(&mut tx, draft).await?;
    if auto_store::rows(&mut tx).await?.len() >= RULES_MAX {
        return Err(invalid(format!(
            "Правил не больше {RULES_MAX}: удалите ненужные"
        )));
    }
    ready(ctx, content.channel, draft.enabled)?;
    let id = auto_store::insert(&mut tx, &content, draft.enabled, admin).await?;
    tx.audit()
        .record(&audit(
            "broadcast.auto_created",
            admin,
            id,
            serde_json::json!({
                "trigger": content.trigger,
                "days": content.days,
                "channel": content.channel,
                "enabled": draft.enabled,
            }),
        ))
        .await?;
    let view = view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn update_rule(
    ctx: &ModuleContext,
    id: i64,
    draft: &RuleDraft,
    admin: Option<AdminId>,
) -> Result<Rule, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let before = auto_store::lock(&mut tx, id).await?;
    let content = cleaned(&mut tx, draft).await?;
    ready(ctx, content.channel, draft.enabled)?;
    auto_store::update(&mut tx, id, &content, draft.enabled).await?;
    if before.enabled && !draft.enabled {
        auto_store::stop_queue(&mut tx, id).await?;
    }
    tx.audit()
        .record(&audit(
            "broadcast.auto_updated",
            admin,
            id,
            serde_json::json!({
                "trigger": content.trigger,
                "days": content.days,
                "channel": content.channel,
                "enabled": draft.enabled,
            }),
        ))
        .await?;
    let view = view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn switch_rule(
    ctx: &ModuleContext,
    id: i64,
    enabled: bool,
    admin: Option<AdminId>,
) -> Result<Rule, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let row = auto_store::lock(&mut tx, id).await?;
    if row.enabled != enabled {
        let parsed = auto_store::parse(&row)?;
        ready(ctx, parsed.channel, enabled)?;
        auto_store::set_enabled(&mut tx, id, enabled).await?;
        if !enabled {
            auto_store::stop_queue(&mut tx, id).await?;
        }
        tx.audit()
            .record(&audit(
                if enabled {
                    "broadcast.auto_enabled"
                } else {
                    "broadcast.auto_disabled"
                },
                admin,
                id,
                serde_json::json!({}),
            ))
            .await?;
    }
    let view = view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn delete_rule(
    ctx: &ModuleContext,
    id: i64,
    admin: Option<AdminId>,
) -> Result<(), BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    auto_store::lock(&mut tx, id).await?;
    auto_store::remove(&mut tx, id).await?;
    let stopped = auto_store::stop_queue(&mut tx, id).await?;
    tx.audit()
        .record(&audit(
            "broadcast.auto_deleted",
            admin,
            id,
            serde_json::json!({ "stopped": stopped }),
        ))
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn rule_reach(
    ctx: &ModuleContext,
    query: &ReachQuery,
) -> Result<AutoReach, BroadcastError> {
    let days = check_days(query.trigger, query.days)?;
    let mut conn = ctx.store.conn().await?;
    let now = crate::catalog::db_now(&mut conn).await?;
    let probe = Probe {
        trigger: query.trigger,
        days,
        with_autorenew: query.with_autorenew && query.trigger == Trigger::Expiring,
        rule: query.id,
    };
    let bounds = Bounds::of(query.trigger, days, now, Duration::hours(CATCH_UP_HOURS));
    let counted = auto_store::count(&mut conn, &probe, &bounds).await?;
    Ok(AutoReach {
        cases: counted.cases,
        recipients: counted.recipients,
        stopped: counted.stopped,
        emails: counted.emails,
        unsubscribed: counted.unsubscribed,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct Round {
    cases: usize,
    queued: usize,
    more: bool,
}

pub async fn check(ctx: &ModuleContext, now: DateTime<Utc>) -> Result<CheckReport, ModuleError> {
    let ids = {
        let mut conn = ctx.store.conn().await?;
        auto_store::enabled_ids(&mut conn).await?
    };
    let mail = mail_site(ctx).map(|_| ()).map_err(|err| err.to_string());
    let mut report = CheckReport::default();
    for id in ids {
        report.rules += 1;
        loop {
            let round = check_rule(ctx, id, now, &mail).await?;
            report.cases += round.cases;
            report.queued += round.queued;
            if !round.more {
                break;
            }
        }
    }
    Ok(report)
}

fn mail_notice(channel: Channel, mail: &Result<(), String>) -> Option<String> {
    match (mail, channel) {
        (Err(reason), Channel::Email) => Some(format!(
            "{reason}. Пока почта не заработает, правило не срабатывает"
        )),
        (Err(reason), Channel::Both) => Some(format!(
            "{reason}. Письма по правилу не уходят, в бот сообщения уходят"
        )),
        _ => None,
    }
}

async fn check_rule(
    ctx: &ModuleContext,
    id: i64,
    now: DateTime<Utc>,
    mail: &Result<(), String>,
) -> Result<Round, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let row = match auto_store::lock(&mut tx, id).await {
        Ok(row) if row.enabled => row,
        Ok(_) | Err(StoreError::NotFound(_)) => {
            tx.rollback().await?;
            return Ok(Round::default());
        }
        Err(err) => return Err(err.into()),
    };
    let parsed = auto_store::parse(&row)?;
    let notice = mail_notice(parsed.channel, mail);
    let bot_lane = parsed.channel.has(Lane::Bot);
    let mail_lane = parsed.channel.has(Lane::Email) && mail.is_ok();
    if !bot_lane && !mail_lane {
        auto_store::set_checked(&mut tx, id, now, notice.as_deref()).await?;
        tx.commit().await?;
        return Ok(Round::default());
    }
    let probe = Probe {
        trigger: parsed.trigger,
        days: parsed.days,
        with_autorenew: row.with_autorenew,
        rule: Some(id),
    };
    let bounds = Bounds::of(
        parsed.trigger,
        parsed.days,
        now,
        Duration::hours(CATCH_UP_HOURS),
    );
    let found = auto_store::found(&mut tx, &probe, &bounds, BATCH).await?;
    let full = i64::try_from(found.len()).unwrap_or(i64::MAX) >= BATCH;
    let targets: Vec<(Option<i64>, Option<String>)> = found
        .iter()
        .map(|f| {
            (
                f.chat().filter(|_| bot_lane),
                f.mailbox().filter(|_| mail_lane).map(str::to_string),
            )
        })
        .collect();
    let any = targets
        .iter()
        .any(|(chat, email)| chat.is_some() || email.is_some());
    let batch = if any {
        Some(auto_store::batch(&mut tx, id, now.date_naive(), row.version).await?)
    } else {
        None
    };
    let cases: Vec<NewCase<'_>> = found
        .iter()
        .zip(&targets)
        .map(|(f, (chat, email))| NewCase {
            key: &f.case_key,
            client_id: ClientId(f.client_id),
            due_at: f.due_at,
            queued: chat.is_some() || email.is_some(),
        })
        .collect();
    let recorded: HashSet<String> = auto_store::record_cases(&mut tx, id, batch, &cases)
        .await?
        .into_iter()
        .collect();
    let mut chats: Vec<(ClientId, i64)> = Vec::new();
    let mut mails: Vec<(ClientId, String)> = Vec::new();
    let mut seen_chat: HashSet<i64> = HashSet::new();
    let mut seen_mail: HashSet<i64> = HashSet::new();
    let mut queued = 0usize;
    for (f, (chat, email)) in found.iter().zip(&targets) {
        if !recorded.contains(&f.case_key) {
            continue;
        }
        if chat.is_some() || email.is_some() {
            queued += 1;
        }
        if let Some(chat) = chat
            && seen_chat.insert(f.client_id)
        {
            chats.push((ClientId(f.client_id), *chat));
        }
        if let Some(email) = email
            && seen_mail.insert(f.client_id)
        {
            mails.push((ClientId(f.client_id), email.clone()));
        }
    }
    if let Some(batch) = batch {
        let added = auto_store::queue_bot(&mut tx, batch, &chats).await?
            + auto_store::queue_mail(&mut tx, batch, &mails).await?;
        if added > 0 {
            auto_store::open_batch(&mut tx, batch, now).await?;
        }
    }
    auto_store::set_checked(&mut tx, id, now, notice.as_deref()).await?;
    tx.commit().await?;
    if !recorded.is_empty() {
        tracing::info!(
            rule = id,
            cases = recorded.len(),
            queued,
            "авторассылка: новые случаи"
        );
    }
    Ok(Round {
        cases: recorded.len(),
        queued,
        more: full && !recorded.is_empty(),
    })
}
