mod routes;
pub mod store;
#[cfg(test)]
mod tests;

use crate::context::{Lifted, ModuleContext, Notification, Restricted, TrafficRestriction};
use crate::error::ModuleError;
use crate::module::{Module, ScheduledTask};
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Offset, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::format;
use sn_domain::ids::{AdminId, ClientId, SubscriptionId};
use sn_domain::module::ModuleId;
use sn_domain::subscription::SubscriptionStatus;
use sn_domain::traffic::GIB;
use std::collections::{HashMap, HashSet};
use store::{Exceed, NewExceed};
use uuid::Uuid;

pub use store::{Counts, ExceedFilter, ExceedItem};

pub const SCAN_TASK: &str = "scan";
pub const LIFT_TASK: &str = "lift";
pub const TEMPLATE_WARNING: &str = "traffic_warning";
pub const TEMPLATE_RESTRICTED: &str = "traffic_restricted";
pub const TEMPLATE_LIFTED: &str = "traffic_lifted";
pub const BOUNDARY_GRACE_MINUTES: i64 = 20;
pub const RESTRICT_TRIES: i32 = 5;
pub const MAX_LIMIT_GB: f64 = 1_000_000.0;
pub const LAST_SCAN_KEY: &str = "limits.last_scan";
const NOTE_MAX: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Day,
    Week,
}

impl Period {
    pub const fn as_str(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "day" => Some(Period::Day),
            "week" => Some(Period::Week),
            _ => None,
        }
    }

    pub const fn per_ru(self) -> &'static str {
        match self {
            Period::Day => "в сутки",
            Period::Week => "в неделю",
        }
    }

    pub const fn during_ru(self) -> &'static str {
        match self {
            Period::Day => "Сегодня",
            Period::Week => "На этой неделе",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    All,
    Trial,
    Paid,
}

impl Audience {
    pub const fn as_str(self) -> &'static str {
        match self {
            Audience::All => "all",
            Audience::Trial => "trial",
            Audience::Paid => "paid",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Audience::All),
            "trial" => Some(Audience::Trial),
            "paid" => Some(Audience::Paid),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficScope {
    Any,
    Unlimited,
    Limited,
}

impl TrafficScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            TrafficScope::Any => "any",
            TrafficScope::Unlimited => "unlimited",
            TrafficScope::Limited => "limited",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "any" => Some(TrafficScope::Any),
            "unlimited" => Some(TrafficScope::Unlimited),
            "limited" => Some(TrafficScope::Limited),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Restriction {
    None,
    Disable,
    Squad,
}

impl Restriction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Restriction::None => "none",
            Restriction::Disable => "disable",
            Restriction::Squad => "squad",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Restriction::None),
            "disable" => Some(Restriction::Disable),
            "squad" => Some(Restriction::Squad),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rule {
    pub id: i64,
    pub enabled: bool,
    pub audience: Audience,
    pub traffic: TrafficScope,
    pub devices_min: Option<u32>,
    pub period: Period,
    pub limit_bytes: u64,
    pub per_device: bool,
    pub warn: bool,
    pub alert: bool,
    pub restriction: Restriction,
    pub squad: Option<Uuid>,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubFacts {
    pub subscription_id: SubscriptionId,
    pub client_id: ClientId,
    pub trial: bool,
    pub unlimited: bool,
    pub devices: Option<u32>,
}

impl Rule {
    pub fn matches(&self, sub: &SubFacts) -> bool {
        let audience = match self.audience {
            Audience::All => true,
            Audience::Trial => sub.trial,
            Audience::Paid => !sub.trial,
        };
        let traffic = match self.traffic {
            TrafficScope::Any => true,
            TrafficScope::Unlimited => sub.unlimited,
            TrafficScope::Limited => !sub.unlimited,
        };
        let devices = self
            .devices_min
            .is_none_or(|min| sub.devices.is_some_and(|d| d >= min));
        self.enabled && audience && traffic && devices
    }

    pub fn threshold(&self, sub: &SubFacts) -> u64 {
        if self.per_device {
            self.limit_bytes
                .saturating_mul(u64::from(sub.devices.unwrap_or(1).max(1)))
        } else {
            self.limit_bytes
        }
    }

    pub fn group(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(
            match (self.audience, self.traffic) {
                (Audience::All, TrafficScope::Any) => "Все подписки",
                (Audience::All, TrafficScope::Unlimited) => "Подписки без лимита трафика",
                (Audience::All, TrafficScope::Limited) => "Подписки с лимитом трафика",
                (Audience::Trial, TrafficScope::Any) => "Пробный период",
                (Audience::Trial, TrafficScope::Unlimited) => "Пробный период без лимита трафика",
                (Audience::Trial, TrafficScope::Limited) => "Пробный период с лимитом трафика",
                (Audience::Paid, TrafficScope::Any) => "Платные подписки",
                (Audience::Paid, TrafficScope::Unlimited) => "Платные без лимита трафика",
                (Audience::Paid, TrafficScope::Limited) => "Платные с лимитом трафика",
            }
            .to_string(),
        );
        if let Some(min) = self.devices_min {
            parts.push(format!(
                "от {min} {}",
                format::plural_ru(u64::from(min), "устройства", "устройств", "устройств")
            ));
        }
        parts.join(", ")
    }

    pub fn describe(&self) -> String {
        let per_device = if self.per_device {
            " на устройство"
        } else {
            ""
        };
        format!(
            "{} · {}{per_device} {}",
            self.group(),
            format::traffic(self.limit_bytes),
            self.period.per_ru()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RuleInput {
    #[serde(default = "yes")]
    pub enabled: bool,
    pub audience: Audience,
    pub traffic: TrafficScope,
    #[serde(default)]
    pub devices_min: Option<u32>,
    pub period: Period,
    pub limit_gb: f64,
    #[serde(default)]
    pub per_device: bool,
    #[serde(default)]
    pub warn: bool,
    #[serde(default)]
    pub alert: bool,
    pub restriction: Restriction,
    #[serde(default)]
    pub squad: Option<Uuid>,
    #[serde(default)]
    pub note: Option<String>,
}

const fn yes() -> bool {
    true
}

fn invalid(message: &str) -> ModuleError {
    ModuleError::Invalid(message.to_string())
}

pub fn validate(input: &RuleInput, now: DateTime<Utc>) -> Result<Rule, ModuleError> {
    if !input.limit_gb.is_finite() || input.limit_gb <= 0.0 || input.limit_gb > MAX_LIMIT_GB {
        return Err(invalid("Порог — число гигабайт больше нуля"));
    }
    let limit_bytes = (input.limit_gb * GIB as f64).round() as u64;
    if limit_bytes == 0 {
        return Err(invalid("Порог — число гигабайт больше нуля"));
    }
    if input.devices_min.is_some_and(|d| d == 0 || d > 1000) {
        return Err(invalid("Число устройств — от 1 до 1000"));
    }
    let squad = match input.restriction {
        Restriction::Squad => Some(
            input
                .squad
                .ok_or_else(|| invalid("Выберите сквад, куда переводить"))?,
        ),
        _ => None,
    };
    if !input.warn && !input.alert && input.restriction == Restriction::None {
        return Err(invalid(
            "Выберите хотя бы одно действие: предупредить, сообщить админам или ограничить",
        ));
    }
    let note = input
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    if note.as_ref().is_some_and(|n| n.chars().count() > NOTE_MAX) {
        return Err(invalid("Заметка — не длиннее 200 символов"));
    }
    Ok(Rule {
        id: 0,
        enabled: input.enabled,
        audience: input.audience,
        traffic: input.traffic,
        devices_min: input.devices_min,
        period: input.period,
        limit_bytes,
        per_device: input.per_device,
        warn: input.warn,
        alert: input.alert,
        restriction: input.restriction,
        squad,
        note,
        created_at: now,
        updated_at: now,
    })
}

pub fn offset_of(minutes: i32) -> FixedOffset {
    FixedOffset::east_opt(minutes.clamp(-1439, 1439) * 60).unwrap_or_else(|| Utc.fix())
}

fn local_midnight(offset: FixedOffset, day: NaiveDate) -> DateTime<Utc> {
    day.and_hms_opt(0, 0, 0)
        .and_then(|naive| offset.from_local_datetime(&naive).single())
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub period: Period,
    pub start: NaiveDate,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

pub fn window(period: Period, now: DateTime<Utc>, offset: FixedOffset) -> Window {
    let today = now.with_timezone(&offset).date_naive();
    let (start, days) = match period {
        Period::Day => (today, 1),
        Period::Week => (
            today - Duration::days(i64::from(today.weekday().num_days_from_monday())),
            7,
        ),
    };
    Window {
        period,
        start,
        starts_at: local_midnight(offset, start),
        ends_at: local_midnight(offset, start + Duration::days(days)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    pub day: NaiveDate,
    pub day_base: u64,
    pub week: NaiveDate,
    pub week_base: u64,
    pub last_bytes: u64,
    pub seen_at: DateTime<Utc>,
}

impl Counter {
    pub fn used(&self, period: Period) -> u64 {
        let base = match period {
            Period::Day => self.day_base,
            Period::Week => self.week_base,
        };
        self.last_bytes.saturating_sub(base)
    }
}

pub fn advance(
    previous: Option<&Counter>,
    lifetime: u64,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Counter {
    let day = window(Period::Day, now, offset);
    let week = window(Period::Week, now, offset);
    let Some(previous) = previous else {
        return Counter {
            day: day.start,
            day_base: lifetime,
            week: week.start,
            week_base: lifetime,
            last_bytes: lifetime,
            seen_at: now,
        };
    };
    let grace = Duration::minutes(BOUNDARY_GRACE_MINUTES);
    let base_for = |kept: NaiveDate, base: u64, current: &Window| {
        if kept == current.start {
            base
        } else if previous.seen_at >= current.starts_at - grace {
            previous.last_bytes
        } else {
            lifetime
        }
    };
    let day_base = base_for(previous.day, previous.day_base, &day).min(lifetime);
    let week_base = base_for(previous.week, previous.week_base, &week).min(lifetime);
    Counter {
        day: day.start,
        day_base,
        week: week.start,
        week_base,
        last_bytes: lifetime,
        seen_at: now,
    }
}

pub fn until_text(at: DateTime<Utc>, offset: FixedOffset) -> String {
    let local = at.with_timezone(&offset);
    format!("{} в {}", local.format("%d.%m"), local.format("%H:%M"))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ScanReport {
    pub subscriptions: usize,
    pub exceeded: usize,
    pub restricted: usize,
    pub warned: usize,
}

pub async fn scan(ctx: &ModuleContext) -> Result<ScanReport, ModuleError> {
    scan_at(ctx, ctx.core.now()).await
}

pub async fn scan_at(ctx: &ModuleContext, now: DateTime<Utc>) -> Result<ScanReport, ModuleError> {
    let (rules, offset) = {
        let mut conn = ctx.store.conn().await?;
        (
            store::rules(&mut conn, true).await?,
            offset_of(store::offset_minutes(&mut conn).await?),
        )
    };
    let mut report = ScanReport::default();
    if rules.is_empty() {
        return Ok(report);
    }
    let readings = ctx.core.traffic_readings().await?;
    let ids: Vec<SubscriptionId> = readings.iter().map(|r| r.subscription_id).collect();
    let (facts, counters) = {
        let mut conn = ctx.store.conn().await?;
        (
            store::facts(&mut conn, &ids).await?,
            store::counters(&mut conn, &ids).await?,
        )
    };
    let mut updated: Vec<(SubscriptionId, Counter)> = Vec::with_capacity(readings.len());
    let mut fresh: Vec<(Exceed, SubscriptionStatus)> = Vec::new();
    for reading in &readings {
        let Some(sub) = facts.get(&reading.subscription_id) else {
            continue;
        };
        let counter = advance(
            counters.get(&reading.subscription_id),
            reading.lifetime_bytes,
            now,
            offset,
        );
        updated.push((reading.subscription_id, counter));
        report.subscriptions += 1;
        for rule in rules.iter().filter(|r| r.matches(sub)) {
            let used = counter.used(rule.period);
            let threshold = rule.threshold(sub);
            if used < threshold {
                continue;
            }
            let current = window(rule.period, now, offset);
            let mut conn = ctx.store.conn().await?;
            let (id, inserted) = store::upsert_exceed(
                &mut conn,
                &NewExceed {
                    rule: rule.id,
                    subscription_id: sub.subscription_id,
                    client_id: sub.client_id,
                    period: rule.period,
                    window_start: current.start,
                    window_end: current.ends_at,
                    limit_bytes: threshold,
                    used_bytes: used,
                    warn: rule.warn,
                    alert: rule.alert,
                    restriction: rule.restriction,
                    squad: rule.squad,
                    at: now,
                },
            )
            .await?;
            if inserted && let Some(exceed) = store::exceed(&mut conn, id).await? {
                report.exceeded += 1;
                fresh.push((exceed, reading.status));
            }
        }
    }
    {
        let mut conn = ctx.store.conn().await?;
        store::save_counters(&mut conn, &updated).await?;
    }
    let handled: HashSet<i64> = fresh.iter().map(|(exceed, _)| exceed.id).collect();
    for (exceed, status) in fresh {
        let restricted = act(ctx, &exceed, status, now, offset).await?;
        if restricted {
            report.restricted += 1;
        }
        if exceed.warn {
            report.warned += 1;
        }
    }
    let statuses: HashMap<SubscriptionId, SubscriptionStatus> = readings
        .iter()
        .map(|r| (r.subscription_id, r.status))
        .collect();
    let retry = {
        let mut conn = ctx.store.conn().await?;
        store::pending_restrictions(&mut conn, now, RESTRICT_TRIES).await?
    };
    for exceed in retry.into_iter().filter(|e| !handled.contains(&e.id)) {
        if let Some(status) = statuses.get(&exceed.subscription_id)
            && restrict(ctx, &exceed, *status, now).await?
        {
            report.restricted += 1;
            if exceed.warn {
                notify(ctx, &exceed, TEMPLATE_RESTRICTED, offset).await;
            }
        }
    }
    let mut conn = ctx.store.conn().await?;
    store::remember_scan(
        &mut conn,
        &serde_json::json!({
            "at": now,
            "subscriptions": report.subscriptions,
            "exceeded": report.exceeded,
        }),
    )
    .await?;
    Ok(report)
}

async fn restrict(
    ctx: &ModuleContext,
    exceed: &Exceed,
    status: SubscriptionStatus,
    now: DateTime<Utc>,
) -> Result<bool, ModuleError> {
    let restriction = match (exceed.restriction, exceed.squad) {
        (Restriction::None, _) => return Ok(false),
        (Restriction::Disable, _) => TrafficRestriction::Disable,
        (Restriction::Squad, Some(squad)) => TrafficRestriction::Squad {
            squad,
            previous: Vec::new(),
        },
        (Restriction::Squad, None) => return Ok(false),
    };
    let mut conn = ctx.store.conn().await?;
    if status != SubscriptionStatus::Active {
        store::note_restriction(
            &mut conn,
            exceed.id,
            "подписка не активна, ограничивать нечего",
            false,
        )
        .await?;
        return Ok(false);
    }
    if exceed.restriction == Restriction::Disable
        && store::restricted_by_others(&mut conn, exceed.subscription_id, exceed.id).await?
    {
        store::mark_restricted(&mut conn, exceed.id, now, exceed.window_end, &[]).await?;
        return Ok(true);
    }
    drop(conn);
    match ctx
        .core
        .restrict_traffic(exceed.subscription_id, &restriction)
        .await
    {
        Ok(Restricted::Applied { previous_squads }) => {
            let mut conn = ctx.store.conn().await?;
            store::mark_restricted(
                &mut conn,
                exceed.id,
                now,
                exceed.window_end,
                &previous_squads,
            )
            .await?;
            Ok(true)
        }
        Ok(Restricted::Skipped(reason)) => {
            let mut conn = ctx.store.conn().await?;
            store::note_restriction(&mut conn, exceed.id, &reason, false).await?;
            Ok(false)
        }
        Err(err) => {
            tracing::warn!(exceed = exceed.id, error = %err, "лимиты трафика: ограничить подписку не удалось");
            let mut conn = ctx.store.conn().await?;
            store::note_restriction(&mut conn, exceed.id, &err.to_string(), true).await?;
            Ok(false)
        }
    }
}

async fn act(
    ctx: &ModuleContext,
    exceed: &Exceed,
    status: SubscriptionStatus,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Result<bool, ModuleError> {
    let restricted = restrict(ctx, exceed, status, now).await?;
    if exceed.warn {
        let template = if restricted {
            TEMPLATE_RESTRICTED
        } else {
            TEMPLATE_WARNING
        };
        if notify(ctx, exceed, template, offset).await {
            let mut conn = ctx.store.conn().await?;
            store::mark_warned(&mut conn, exceed.id, now).await?;
        }
    }
    Ok(restricted)
}

pub fn notice(exceed: &Exceed, template: &str, offset: FixedOffset) -> Notification {
    let mut params = serde_json::json!({
        "used": format::traffic(exceed.used_bytes),
        "limit": format::traffic(exceed.limit_bytes),
        "used_bytes": exceed.used_bytes,
        "limit_bytes": exceed.limit_bytes,
        "period": exceed.period.as_str(),
        "when": exceed.period.during_ru(),
        "per": exceed.period.per_ru(),
        "until": until_text(exceed.window_end, offset),
        "until_at": exceed.window_end,
        "also_mail": true,
    });
    if template == TEMPLATE_LIFTED {
        params["also_mail"] = serde_json::json!(false);
    }
    Notification {
        template: template.to_string(),
        params,
    }
}

async fn notify(ctx: &ModuleContext, exceed: &Exceed, template: &str, offset: FixedOffset) -> bool {
    match ctx
        .core
        .notify(exceed.client_id, notice(exceed, template, offset))
        .await
    {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(exceed = exceed.id, error = %err, "лимиты трафика: уведомление клиенту не поставлено");
            false
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct LiftReport {
    pub lifted: usize,
    pub failed: usize,
}

pub async fn lift_due(
    ctx: &ModuleContext,
    now: DateTime<Utc>,
    everything: bool,
) -> Result<LiftReport, ModuleError> {
    let (due, offset) = {
        let mut conn = ctx.store.conn().await?;
        (
            store::due_lifts(&mut conn, now, everything).await?,
            offset_of(store::offset_minutes(&mut conn).await?),
        )
    };
    let mut report = LiftReport::default();
    let reason = if everything { "off" } else { "window" };
    for exceed in due {
        match lift_one(ctx, &exceed, now, reason, None, offset).await {
            Ok(true) => report.lifted += 1,
            Ok(false) => {}
            Err(err) => {
                report.failed += 1;
                tracing::warn!(exceed = exceed.id, error = %err, "лимиты трафика: ограничение пока не снято");
            }
        }
    }
    Ok(report)
}

async fn lift_one(
    ctx: &ModuleContext,
    exceed: &Exceed,
    now: DateTime<Utc>,
    reason: &str,
    admin: Option<AdminId>,
    offset: FixedOffset,
) -> Result<bool, ModuleError> {
    let live = {
        let mut conn = ctx.store.conn().await?;
        store::subscription_live(&mut conn, exceed.subscription_id).await?
    };
    let others = live && exceed.restriction == Restriction::Disable && {
        let mut conn = ctx.store.conn().await?;
        store::restricted_by_others(&mut conn, exceed.subscription_id, exceed.id).await?
    };
    let outcome = if !live {
        Lifted::Ended
    } else if others {
        let mut conn = ctx.store.conn().await?;
        return Ok(store::mark_lifted(&mut conn, exceed.id, now, reason, admin).await?);
    } else {
        let restriction = match exceed.restriction {
            Restriction::Squad => TrafficRestriction::Squad {
                squad: exceed.squad.unwrap_or_default(),
                previous: exceed.previous_squads.clone(),
            },
            _ => TrafficRestriction::Disable,
        };
        ctx.core
            .lift_traffic(exceed.subscription_id, &restriction)
            .await?
    };
    let (reason, tell) = match outcome {
        Lifted::Restored => (reason, true),
        Lifted::Changed(_) => ("changed", false),
        Lifted::Blocked => ("blocked", false),
        Lifted::Ended => ("ended", false),
    };
    let mut conn = ctx.store.conn().await?;
    let marked = store::mark_lifted(&mut conn, exceed.id, now, reason, admin).await?;
    drop(conn);
    if marked && tell && exceed.warn {
        notify(ctx, exceed, TEMPLATE_LIFTED, offset).await;
    }
    Ok(marked)
}

pub async fn lift_by_admin(
    ctx: &ModuleContext,
    id: i64,
    admin: AdminId,
) -> Result<Exceed, ModuleError> {
    let (exceed, offset, others) = {
        let mut conn = ctx.store.conn().await?;
        let exceed = store::exceed(&mut conn, id)
            .await?
            .ok_or(ModuleError::NotFound("превышение"))?;
        let offset = offset_of(store::offset_minutes(&mut conn).await?);
        let others =
            store::active_restrictions(&mut conn, exceed.subscription_id, exceed.id).await?;
        (exceed, offset, others)
    };
    if exceed.restricted_at.is_none() || exceed.lifted_at.is_some() {
        return Err(ModuleError::Invalid(
            "Ограничения нет или оно уже снято".into(),
        ));
    }
    let now = ctx.core.now();
    for other in &others {
        lift_one(ctx, other, now, "admin", Some(admin), offset).await?;
    }
    lift_one(ctx, &exceed, now, "admin", Some(admin), offset).await?;
    let mut conn = ctx.store.conn().await?;
    store::exceed(&mut conn, id)
        .await?
        .ok_or(ModuleError::NotFound("превышение"))
}

async fn module_enabled(ctx: &ModuleContext) -> Result<bool, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM module_states WHERE module = 'limits'")
            .fetch_optional(sn_store::Db::conn(&mut conn))
            .await
            .map_err(sn_store::StoreError::from)?;
    Ok(enabled.unwrap_or(false))
}

pub struct LimitsModule;

#[async_trait]
impl Module for LimitsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Limits
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: SCAN_TASK,
            every: std::time::Duration::from_secs(600),
        }]
    }

    fn cleanup_tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: LIFT_TASK,
            every: std::time::Duration::from_secs(120),
        }]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        match task {
            SCAN_TASK => {
                let report = scan(ctx).await?;
                if report.exceeded > 0 {
                    tracing::info!(?report, "лимиты трафика: найдены превышения");
                }
                Ok(())
            }
            LIFT_TASK => {
                let everything = !module_enabled(ctx).await?;
                let report = lift_due(ctx, ctx.core.now(), everything).await?;
                if report.lifted > 0 {
                    tracing::info!(?report, "лимиты трафика: ограничения сняты");
                }
                Ok(())
            }
            other => Err(ModuleError::Invalid(format!("у модуля нет задачи {other}"))),
        }
    }
}
