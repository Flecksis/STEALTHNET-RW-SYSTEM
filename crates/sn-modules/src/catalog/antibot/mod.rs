pub mod captcha;
pub mod checks;
mod routes;
pub mod signals;
pub mod signups;
pub mod store;
#[cfg(test)]
mod tests;

use crate::catalog::config_flag;
use crate::catalog::web::{address_network, is_internal};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{Module, ModuleConfig, ScheduledTask};
use async_trait::async_trait;
use axum::Router;
use captcha::{Captcha, Provider, Scope};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use signals::Signal;
use sn_domain::events::DomainEvent;
use sn_domain::ids::{AdminId, ClientId, PaymentId};
use sn_domain::module::ModuleId;
use sn_domain::settings::SettingField;
use sn_store::{Db, StoreError};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use store::NewEvent;

pub use checks::{Answer, Challenge, Check, Confirmation, Surface};
pub use store::{Action, JournalFilter, JournalItem, Mark, MarkItem, Summary};

pub const CONFIRM_CODE: &str = "antibot.confirm";
pub const DENIED_CODE: &str = "antibot.denied";
pub const TOO_FAST: &str = "antibot.too_fast";
pub const EXPIRED: &str = "antibot.expired";
pub const FAILED: &str = "antibot.failed";
pub const UNAVAILABLE: &str = "antibot.unavailable";
pub const CAPTCHA_REQUIRED: &str = "antibot.captcha";
pub const PROMO_LINK: &str = "promo_link";
pub const TRIAL: &str = "trial";
pub const SIGNUP: &str = "signup";
pub const LOGIN: &str = "login";
pub const CLEARED_AUDIT: &str = "client.antibot_cleared";
const SWEEP_TASK: &str = "antibot.sweep";
const MODE_OPTIONS: &[&str] = &["mark", "confirm", "block"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Mark,
    #[default]
    Confirm,
    Block,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "mark" => Some(Mode::Mark),
            "confirm" => Some(Mode::Confirm),
            "block" => Some(Mode::Block),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Mark => "mark",
            Mode::Confirm => "confirm",
            Mode::Block => "block",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AntibotSettings {
    pub mode: Mode,
    pub young_days: u32,
    pub check_username: bool,
    pub burst_count: u32,
    pub ip_limit: u32,
    pub extra_domains: Vec<String>,
    pub referral_guard: bool,
    pub mass_invites: u32,
    pub roulette_guard: bool,
    pub contest_guard: bool,
    pub captcha: Provider,
    pub captcha_site_key: String,
    pub captcha_trial: Scope,
    pub captcha_signup: bool,
    pub site_address_day: u32,
    pub site_network_day: u32,
    pub site_minute: u32,
}

impl Default for AntibotSettings {
    fn default() -> Self {
        AntibotSettings {
            mode: Mode::Confirm,
            young_days: 30,
            check_username: true,
            burst_count: 5,
            ip_limit: 3,
            extra_domains: Vec::new(),
            referral_guard: true,
            mass_invites: 5,
            roulette_guard: true,
            contest_guard: true,
            captcha: Provider::None,
            captcha_site_key: String::new(),
            captcha_trial: Scope::All,
            captcha_signup: true,
            site_address_day: 0,
            site_network_day: 0,
            site_minute: 0,
        }
    }
}

fn whole(config: &ModuleConfig, key: &str, default: u32, max: u32) -> Result<u32, String> {
    match config.get(key) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(default),
        Some(_) => crate::catalog::config_number(config, key)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n <= max)
            .ok_or_else(|| format!("{key}: целое число от 0 до {max}")),
    }
}

impl AntibotSettings {
    pub fn parse(config: &ModuleConfig) -> Result<Self, String> {
        let defaults = AntibotSettings::default();
        let mode = match config.text("mode") {
            None => defaults.mode,
            Some(raw) => Mode::parse(raw).ok_or("mode: mark, confirm или block")?,
        };
        Ok(AntibotSettings {
            mode,
            young_days: whole(config, "young_days", defaults.young_days, 3650)?,
            check_username: config_flag(config, "check_username")
                .unwrap_or(defaults.check_username),
            burst_count: whole(config, "burst_count", defaults.burst_count, 1000)?,
            ip_limit: whole(config, "ip_limit", defaults.ip_limit, 1000)?,
            extra_domains: config
                .text("extra_domains")
                .map(signals::parse_domains)
                .unwrap_or_default(),
            referral_guard: config_flag(config, "referral_guard")
                .unwrap_or(defaults.referral_guard),
            mass_invites: whole(config, "mass_invites", defaults.mass_invites, 1000)?,
            roulette_guard: config_flag(config, "roulette_guard")
                .unwrap_or(defaults.roulette_guard),
            contest_guard: config_flag(config, "contest_guard").unwrap_or(defaults.contest_guard),
            captcha: match config.text("captcha") {
                None => defaults.captcha,
                Some(raw) => Provider::parse(raw).ok_or("captcha: none, turnstile или yandex")?,
            },
            captcha_site_key: match config.text("captcha_site_key") {
                None => String::new(),
                Some(raw) => captcha::clean_key(raw).ok_or(
                    "captcha_site_key: латинские буквы, цифры, точка, дефис и подчёркивание",
                )?,
            },
            captcha_trial: match config.text("captcha_trial") {
                None => defaults.captcha_trial,
                Some(raw) => Scope::parse(raw).ok_or("captcha_trial: all или suspicious")?,
            },
            captcha_signup: config_flag(config, "captcha_signup")
                .unwrap_or(defaults.captcha_signup),
            site_address_day: whole(
                config,
                "site_address_day",
                defaults.site_address_day,
                signups::MAX_LIMIT,
            )?,
            site_network_day: whole(
                config,
                "site_network_day",
                defaults.site_network_day,
                signups::MAX_LIMIT,
            )?,
            site_minute: whole(
                config,
                "site_minute",
                defaults.site_minute,
                signups::MAX_LIMIT,
            )?,
        })
    }

    pub fn check_captcha(config: &ModuleConfig) -> Result<(), String> {
        let settings = Self::parse(config)?;
        if settings.captcha == Provider::None {
            return Ok(());
        }
        if settings.captcha_site_key.is_empty() {
            return Err(format!(
                "Для капчи {} укажите ключ сайта",
                settings.captcha.title()
            ));
        }
        match config.text("captcha_secret") {
            None => Err(format!(
                "Для капчи {} укажите секретный ключ",
                settings.captcha.title()
            )),
            Some(raw) => {
                let secret = captcha::clean_key(raw).ok_or(
                    "captcha_secret: латинские буквы, цифры, точка, дефис и подчёркивание",
                )?;
                captcha::check_keys(settings.captcha, &settings.captcha_site_key, &secret)
            }
        }
    }

    pub fn lenient(config: &ModuleConfig) -> Self {
        Self::parse(config).unwrap_or_else(|reason| {
            tracing::warn!(%reason, "настройки антибота не разобраны, взяты значения по умолчанию");
            AntibotSettings::default()
        })
    }
}

pub async fn active(db: &mut impl Db) -> Result<Option<AntibotSettings>, StoreError> {
    Ok(match store::module_state(db).await? {
        Some((true, config)) => Some(AntibotSettings::lenient(&ModuleConfig::from_value(&config))),
        _ => None,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Assessment {
    pub signals: Vec<Signal>,
    pub premium: bool,
    pub details: serde_json::Map<String, serde_json::Value>,
}

impl Assessment {
    pub fn suspicious(&self) -> bool {
        signals::suspicious(&self.signals, self.premium)
    }

    fn add(&mut self, signal: Signal, detail: Option<(&str, serde_json::Value)>) {
        if !self.signals.contains(&signal) {
            self.signals.push(signal);
            self.signals.sort();
        }
        if let Some((key, value)) = detail {
            self.details.insert(key.to_string(), value);
        }
    }

    pub fn details_value(&self) -> serde_json::Value {
        serde_json::Value::Object(self.details.clone())
    }
}

fn network(raw: &str) -> Option<String> {
    let ip: IpAddr = raw.trim().parse().ok()?;
    (!is_internal(ip)).then(|| address_network(ip))
}

pub async fn young_boundary(
    db: &mut impl Db,
    settings: &AntibotSettings,
    now: DateTime<Utc>,
) -> Result<Option<i64>, StoreError> {
    if settings.young_days == 0 {
        return Ok(None);
    }
    match store::saved_boundary(db, settings.young_days).await? {
        Some(saved) => Ok(Some(saved)),
        None => count_boundary(db, settings, now).await,
    }
}

async fn count_boundary(
    db: &mut impl Db,
    settings: &AntibotSettings,
    now: DateTime<Utc>,
) -> Result<Option<i64>, StoreError> {
    let cutoff = now - Duration::days(i64::from(settings.young_days));
    let (count, max) = store::telegram_history(db, cutoff).await?;
    Ok(max.filter(|_| count >= signals::MIN_HISTORY))
}

async fn keep_boundary(
    db: &mut impl Db,
    settings: &AntibotSettings,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let days = settings.young_days;
    if days == 0 || store::saved_boundary(db, days).await?.is_some() {
        return Ok(());
    }
    if let Some(boundary) = count_boundary(db, settings, now).await? {
        store::save_boundary(db, days, boundary).await?;
    }
    Ok(())
}

pub async fn assess(
    db: &mut impl Db,
    client_id: ClientId,
    settings: &AntibotSettings,
    now: DateTime<Utc>,
) -> Result<Assessment, StoreError> {
    let mut found = Assessment::default();
    let Some(registration) = store::registration(db, client_id).await? else {
        return Ok(found);
    };
    if let Some(telegram) = store::telegram(db, client_id).await? {
        found.premium = telegram.premium;
        if !registration.brought
            && let Some(boundary) = young_boundary(db, settings, now).await?
            && telegram.id > boundary
        {
            found.add(
                Signal::YoungAccount,
                Some(("newer_than", serde_json::json!(boundary))),
            );
        }
        if !registration.brought && settings.check_username && telegram.username.is_none() {
            found.add(Signal::NoUsername, None);
        }
        if !registration.brought && settings.burst_count > 1 {
            let window = Duration::minutes(signals::BURST_MINUTES);
            let neighbors = store::telegram_neighbors(
                db,
                client_id,
                telegram.id,
                telegram.linked_at - window,
                telegram.linked_at + window,
                signals::BURST_SPREAD,
            )
            .await?;
            if neighbors + 1 >= i64::from(settings.burst_count) {
                found.add(
                    Signal::Burst,
                    Some(("burst", serde_json::json!(neighbors + 1))),
                );
            }
        }
        found
            .details
            .insert("telegram_id".into(), serde_json::json!(telegram.id));
    }
    if let Some(email) = store::email(db, client_id).await? {
        if signals::disposable(&email, &settings.extra_domains) {
            found.add(
                Signal::DisposableEmail,
                signals::email_domain(&email).map(|d| ("domain", serde_json::json!(d))),
            );
        }
        if let Some(twin) = store::email_twin(db, client_id, &email).await? {
            found.add(
                Signal::EmailAlias,
                Some(("alias_of", serde_json::json!(twin.0))),
            );
        }
    }
    if registration.web && settings.ip_limit > 0 {
        let grace = Duration::minutes(signals::SESSION_GRACE_MINUTES);
        if let Some(own) = store::first_ip(db, client_id, registration.created_at + grace)
            .await?
            .as_deref()
            .and_then(network)
        {
            let earlier = store::signups_from(
                db,
                client_id,
                &own,
                registration.created_at - Duration::hours(signals::SIGNUP_WINDOW_HOURS),
                registration.created_at,
                grace,
            )
            .await?;
            if earlier >= i64::from(settings.ip_limit) {
                found.add(Signal::IpLimit, Some(("network", serde_json::json!(own))));
            }
        }
    }
    Ok(found)
}

async fn remember(
    db: &mut impl Db,
    client_id: ClientId,
    found: &[Signal],
    details: &serde_json::Value,
) -> Result<(), StoreError> {
    let mut reasons = store::mark_of(db, client_id)
        .await?
        .filter(|m| m.cleared_at.is_none())
        .map(|m| m.reasons)
        .unwrap_or_default();
    signals::merge(&mut reasons, found);
    let created = store::save_mark(db, client_id, &reasons, details).await?;
    if created {
        store::event(
            db,
            &NewEvent {
                client_id,
                action: Action::Marked,
                reasons: &reasons,
                details: details.clone(),
                dedupe: Some(format!("marked:{client_id}")),
                admin: None,
            },
        )
        .await?;
    }
    Ok(())
}

async fn db_now(db: &mut impl Db) -> Result<DateTime<Utc>, StoreError> {
    crate::catalog::db_now(db).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub settings: AntibotSettings,
    pub mark: Option<Mark>,
    pub signals: Vec<Signal>,
}

impl Verdict {
    pub fn flagged(&self) -> bool {
        self.mark.as_ref().is_some_and(|m| m.cleared_at.is_none())
    }

    pub fn confirmed(&self) -> bool {
        self.mark.as_ref().is_some_and(|m| m.confirmed_at.is_some())
    }
}

pub async fn verdict(db: &mut impl Db, client_id: ClientId) -> Result<Option<Verdict>, StoreError> {
    let Some(settings) = active(db).await? else {
        return Ok(None);
    };
    judge(db, client_id, settings).await.map(Some)
}

async fn judge(
    db: &mut impl Db,
    client_id: ClientId,
    settings: AntibotSettings,
) -> Result<Verdict, StoreError> {
    let mark = store::mark_of(db, client_id).await?;
    if mark.as_ref().is_some_and(|m| m.cleared_at.is_some()) {
        return Ok(Verdict {
            settings,
            mark,
            signals: Vec::new(),
        });
    }
    let now = db_now(db).await?;
    keep_boundary(db, &settings, now).await?;
    let assessment = assess(db, client_id, &settings, now).await?;
    if assessment.suspicious() {
        remember(
            db,
            client_id,
            &assessment.signals,
            &assessment.details_value(),
        )
        .await?;
    }
    let mark = store::mark_of(db, client_id).await?;
    let signals = mark.as_ref().map(|m| m.reasons.clone()).unwrap_or_default();
    Ok(Verdict {
        settings,
        mark,
        signals,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Roulette,
    Contest,
}

impl Target {
    pub const fn as_str(self) -> &'static str {
        match self {
            Target::Roulette => "roulette",
            Target::Contest => "contest",
        }
    }

    pub const fn guarded(self, settings: &AntibotSettings) -> bool {
        match self {
            Target::Roulette => settings.roulette_guard,
            Target::Contest => settings.contest_guard,
        }
    }
}

pub async fn guarding(
    db: &mut impl Db,
    target: Target,
) -> Result<Option<AntibotSettings>, StoreError> {
    Ok(active(db).await?.filter(|s| target.guarded(s)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard {
    pub gate: Gate,
    pub reasons: Vec<Signal>,
}

impl Guard {
    pub fn denied(guard: Option<&Guard>) -> bool {
        guard.is_some_and(|g| g.gate == Gate::Deny)
    }

    pub fn asks(guard: Option<&Guard>) -> bool {
        guard.is_some_and(|g| g.gate == Gate::Confirm)
    }

    pub fn noted(guard: Option<&Guard>) -> bool {
        guard.is_some_and(|g| g.gate == Gate::Allow)
    }
}

pub async fn guard(
    ctx: &ModuleContext,
    client_id: ClientId,
    target: Target,
) -> Result<Option<Guard>, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let Some(settings) = guarding(&mut conn, target).await? else {
        return Ok(None);
    };
    let verdict = judge(&mut conn, client_id, settings).await?;
    if !verdict.flagged() {
        return Ok(None);
    }
    let gate = match verdict.settings.mode {
        Mode::Mark => Gate::Allow,
        Mode::Confirm if verdict.confirmed() => return Ok(None),
        Mode::Confirm => Gate::Confirm,
        Mode::Block => Gate::Deny,
    };
    Ok(Some(Guard {
        gate,
        reasons: verdict.signals,
    }))
}

pub async fn note(
    db: &mut impl Db,
    client_id: ClientId,
    action: Action,
    reasons: &[Signal],
    details: serde_json::Value,
    once: Option<i64>,
) -> Result<(), StoreError> {
    let scope = match once {
        Some(id) => id.to_string(),
        None => day_key(db_now(db).await?),
    };
    store::event(
        db,
        &NewEvent {
            client_id,
            action,
            reasons,
            details,
            dedupe: Some(format!("{}:{client_id}:{scope}", action.as_str())),
            admin: None,
        },
    )
    .await?;
    Ok(())
}

pub async fn record(
    ctx: &ModuleContext,
    client_id: ClientId,
    guard: Option<&Guard>,
    action: Action,
    details: serde_json::Value,
    once: Option<i64>,
) -> Result<(), ModuleError> {
    let Some(guard) = guard else {
        return Ok(());
    };
    let mut conn = ctx.store.conn().await?;
    note(&mut conn, client_id, action, &guard.reasons, details, once).await?;
    Ok(())
}

pub async fn blocked_among(
    db: &mut impl Db,
    target: Target,
    ids: &[i64],
) -> Result<HashMap<ClientId, Vec<Signal>>, StoreError> {
    match guarding(db, target).await? {
        Some(settings) if settings.mode == Mode::Block && !ids.is_empty() => {
            store::open_marks_among(db, ids).await
        }
        _ => Ok(HashMap::new()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invite {
    pub referrer: ClientId,
    pub invitee: ClientId,
    pub linked_at: DateTime<Utc>,
}

pub async fn screen_invites(
    db: &mut impl Db,
    settings: &AntibotSettings,
    invites: &[Invite],
) -> Result<HashSet<ClientId>, StoreError> {
    let mut out = HashSet::new();
    if invites.is_empty() {
        return Ok(out);
    }
    let ids: Vec<i64> = invites.iter().map(|i| i.invitee.0).collect();
    let marks = store::marks_among(db, &ids).await?;
    let now = db_now(db).await?;
    let since = now - Duration::days(signals::SHARED_SESSIONS_DAYS);
    let mut boundary = None;
    let mut referrers: HashMap<ClientId, Traces> = HashMap::new();
    for invite in invites {
        let mark = marks.get(&invite.invitee);
        if mark.is_some_and(|m| m.cleared_at.is_some()) {
            continue;
        }
        if let Some(mark) = mark {
            let invited_badly = mark
                .reasons
                .iter()
                .any(|r| matches!(r, Signal::SelfInvite | Signal::MassInvites));
            let unproven = match settings.mode {
                Mode::Mark => false,
                Mode::Confirm => mark.confirmed_at.is_none(),
                Mode::Block => true,
            };
            if invited_badly || unproven {
                out.insert(invite.invitee);
                continue;
            }
        } else if settings.mode != Mode::Mark {
            let assessment = assess(db, invite.invitee, settings, now).await?;
            if assessment.suspicious() {
                remember(
                    db,
                    invite.invitee,
                    &assessment.signals,
                    &assessment.details_value(),
                )
                .await?;
                out.insert(invite.invitee);
                continue;
            }
        }
        if !settings.referral_guard {
            continue;
        }
        let theirs = match referrers.get(&invite.referrer) {
            Some(found) => found.clone(),
            None => {
                let found = Traces::of(db, invite.referrer, since).await?;
                referrers.insert(invite.referrer, found.clone());
                found
            }
        };
        let mine = Traces::of(db, invite.invitee, since).await?;
        if let Some(details) = mine.shared_with(&theirs) {
            let details =
                serde_json::json!({ "self_invite": details, "referrer_id": invite.referrer.0 });
            remember(db, invite.invitee, &[Signal::SelfInvite], &details).await?;
            out.insert(invite.invitee);
            continue;
        }
        if let Some(count) = mass_invites(db, settings, *invite, now, &mut boundary).await? {
            let details = serde_json::json!({ "invites": count, "referrer_id": invite.referrer.0 });
            remember(db, invite.invitee, &[Signal::MassInvites], &details).await?;
            out.insert(invite.invitee);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Allow,
    Confirm,
    Deny,
}

fn day_key(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%d").to_string()
}

fn wants_captcha(captcha: &Captcha, verdict: &Verdict) -> bool {
    match captcha.trial {
        Scope::All => true,
        Scope::Suspicious => verdict.flagged() && verdict.settings.mode == Mode::Confirm,
    }
}

pub async fn trial_gate(ctx: &ModuleContext, client_id: ClientId) -> Result<Gate, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let Some(verdict) = verdict(&mut conn, client_id).await? else {
        return Ok(Gate::Allow);
    };
    let details = serde_json::json!({ "target": TRIAL });
    let actions = (Action::Asked, Action::TrialDenied);
    let blocked = verdict.flagged() && verdict.settings.mode == Mode::Block;
    let captcha = captcha::active(ctx).filter(|c| wants_captcha(c, &verdict));
    if captcha.is_none() || blocked {
        return Ok(gate_of(&mut conn, client_id, &verdict, actions, details).await?);
    }
    if checks::passed_recently(&mut conn, client_id, TRIAL).await? {
        return Ok(Gate::Allow);
    }
    if verdict.flagged() {
        note(
            &mut conn,
            client_id,
            Action::Asked,
            &verdict.signals,
            details,
            None,
        )
        .await?;
    }
    Ok(Gate::Confirm)
}

pub async fn trial_challenge(
    ctx: &ModuleContext,
    client_id: ClientId,
    surface: Surface,
    variant: Option<u32>,
) -> Result<Challenge, ModuleError> {
    if let Some(captcha) = captcha::active(ctx) {
        let mut conn = ctx.store.conn().await?;
        if let Some(verdict) = verdict(&mut conn, client_id).await?
            && wants_captcha(&captcha, &verdict)
        {
            drop(conn);
            let details = match variant {
                Some(id) => serde_json::json!({ "variant": id }),
                None => serde_json::json!({}),
            };
            return checks::captcha_challenge(ctx, client_id, TRIAL, &captcha, details).await;
        }
    }
    checks::challenge(ctx, client_id, TRIAL, surface).await
}

pub async fn promo_link_gate(
    ctx: &ModuleContext,
    client_id: ClientId,
    link_id: i64,
    name: &str,
) -> Result<Gate, ModuleError> {
    free_gate(
        ctx,
        client_id,
        (Action::PromoLinkAsked, Action::PromoLinkDenied),
        serde_json::json!({ "target": PROMO_LINK, "link_id": link_id, "name": name }),
    )
    .await
}

async fn free_gate(
    ctx: &ModuleContext,
    client_id: ClientId,
    actions: (Action, Action),
    details: serde_json::Value,
) -> Result<Gate, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let Some(verdict) = verdict(&mut conn, client_id).await? else {
        return Ok(Gate::Allow);
    };
    Ok(gate_of(&mut conn, client_id, &verdict, actions, details).await?)
}

async fn gate_of(
    db: &mut impl Db,
    client_id: ClientId,
    verdict: &Verdict,
    (asked, denied): (Action, Action),
    details: serde_json::Value,
) -> Result<Gate, StoreError> {
    if !verdict.flagged() {
        return Ok(Gate::Allow);
    }
    let now = db_now(db).await?;
    let (gate, action) = match verdict.settings.mode {
        Mode::Mark => return Ok(Gate::Allow),
        Mode::Confirm if verdict.confirmed() => return Ok(Gate::Allow),
        Mode::Confirm => (Gate::Confirm, asked),
        Mode::Block => (Gate::Deny, denied),
    };
    store::event(
        db,
        &NewEvent {
            client_id,
            action,
            reasons: &verdict.signals,
            details,
            dedupe: Some(format!("{}:{client_id}:{}", action.as_str(), day_key(now))),
            admin: None,
        },
    )
    .await?;
    Ok(gate)
}

pub async fn promo_allowed(ctx: &ModuleContext, client_id: ClientId) -> Result<bool, ModuleError> {
    free_allowed(ctx, client_id, "promo").await
}

pub async fn free_allowed(
    ctx: &ModuleContext,
    client_id: ClientId,
    target: &'static str,
) -> Result<bool, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let Some(verdict) = verdict(&mut conn, client_id).await? else {
        return Ok(true);
    };
    if !verdict.flagged() || verdict.settings.mode != Mode::Block {
        return Ok(true);
    }
    let now = db_now(&mut conn).await?;
    store::event(
        &mut conn,
        &NewEvent {
            client_id,
            action: Action::PromoDenied,
            reasons: &verdict.signals,
            details: serde_json::json!({ "target": target }),
            dedupe: Some(format!(
                "promo_denied:{target}:{client_id}:{}",
                day_key(now)
            )),
            admin: None,
        },
    )
    .await?;
    Ok(false)
}

pub async fn bonus_allowed(ctx: &ModuleContext, client_id: ClientId) -> Result<bool, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let Some(verdict) = verdict(&mut conn, client_id).await? else {
        return Ok(true);
    };
    Ok(!(verdict.flagged() && verdict.settings.mode == Mode::Block))
}

pub async fn challenge(
    ctx: &ModuleContext,
    client_id: ClientId,
    target: &str,
    surface: Surface,
) -> Result<Challenge, ModuleError> {
    checks::challenge(ctx, client_id, target, surface).await
}

pub async fn confirm(
    ctx: &ModuleContext,
    client_id: ClientId,
    token: &str,
    target: &str,
    answer: Answer<'_>,
) -> Result<Confirmation, ModuleError> {
    checks::confirm(ctx, client_id, token, target, answer).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignupCheck {
    Off,
    Passed,
    Missing,
    Failed,
    Unavailable,
}

pub fn signup_captcha(ctx: &ModuleContext) -> Option<Captcha> {
    captcha::active(ctx).filter(|c| c.signup)
}

pub fn spare_captcha(ctx: &ModuleContext) -> Option<Captcha> {
    let config = ctx.core.module_config(ModuleId::Antibot);
    captcha::active(ctx).filter(|_| AntibotSettings::lenient(&config).site_minute > 0)
}

pub async fn check_signup(
    ctx: &ModuleContext,
    action: &str,
    token: Option<&str>,
    ip: Option<&str>,
) -> Result<SignupCheck, ModuleError> {
    match signup_captcha(ctx) {
        Some(captcha) => check_answer(ctx, &captcha, action, token, ip).await,
        None => Ok(SignupCheck::Off),
    }
}

pub async fn check_human(
    ctx: &ModuleContext,
    token: Option<&str>,
    ip: Option<&str>,
) -> Result<SignupCheck, ModuleError> {
    match captcha::active(ctx) {
        Some(captcha) => check_answer(ctx, &captcha, SIGNUP, token, ip).await,
        None => Ok(SignupCheck::Off),
    }
}

async fn check_answer(
    ctx: &ModuleContext,
    captcha: &Captcha,
    action: &str,
    token: Option<&str>,
    ip: Option<&str>,
) -> Result<SignupCheck, ModuleError> {
    let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(SignupCheck::Missing);
    };
    let outcome = captcha::verify(
        &ctx.store,
        &ctx.core.captcha_endpoints(),
        captcha,
        token,
        ip,
        captcha::Expect {
            action,
            cdata: None,
        },
    )
    .await?;
    Ok(match outcome {
        captcha::Outcome::Passed => SignupCheck::Passed,
        captcha::Outcome::Rejected(_) => SignupCheck::Failed,
        captcha::Outcome::Unavailable(_) => SignupCheck::Unavailable,
    })
}

pub async fn clear(
    ctx: &ModuleContext,
    client_id: ClientId,
    admin: Option<AdminId>,
) -> Result<bool, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let Some(mark) = store::mark_of(&mut tx, client_id).await? else {
        return Err(ModuleError::NotFound("отметка антибота"));
    };
    if !store::clear(&mut tx, client_id, admin).await? {
        tx.commit().await?;
        return Ok(false);
    }
    store::event(
        &mut tx,
        &NewEvent {
            client_id,
            action: Action::Cleared,
            reasons: &mark.reasons,
            details: serde_json::json!({}),
            dedupe: None,
            admin,
        },
    )
    .await?;
    tx.audit()
        .record(&sn_store::AuditEntry {
            actor_kind: if admin.is_some() {
                sn_store::ActorKind::Admin
            } else {
                sn_store::ActorKind::System
            },
            actor_id: admin.map(|a| a.0),
            action: CLEARED_AUDIT.into(),
            entity_kind: Some(sn_store::audit_entity::CLIENT.into()),
            entity_id: Some(client_id.0),
            details: serde_json::json!({ "reason": signals::titles(&mark.reasons) }),
            ip: None,
        })
        .await?;
    tx.commit().await?;
    Ok(true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferralGate {
    Allow,
    NoDirect(Signal),
    NoBonus,
}

fn shared(a: &[String], b: &[String]) -> Option<String> {
    let mine: HashSet<String> = a.iter().filter_map(|ip| network(ip)).collect();
    b.iter()
        .filter_map(|ip| network(ip))
        .find(|n| mine.contains(n))
}

#[derive(Debug, Clone, Default)]
struct Traces {
    ips: Vec<String>,
    email: Option<String>,
}

impl Traces {
    async fn of(
        db: &mut impl Db,
        client_id: ClientId,
        since: DateTime<Utc>,
    ) -> Result<Self, StoreError> {
        Ok(Traces {
            ips: store::session_ips(db, client_id, since).await?,
            email: store::email(db, client_id).await?,
        })
    }

    fn shared_with(&self, other: &Traces) -> Option<serde_json::Value> {
        if let Some(network) = shared(&self.ips, &other.ips) {
            return Some(serde_json::json!({ "network": network }));
        }
        match (&self.email, &other.email) {
            (Some(a), Some(b)) if signals::email_key(a) == signals::email_key(b) => {
                Some(serde_json::json!({ "email": true }))
            }
            _ => None,
        }
    }
}

async fn shared_traces(
    db: &mut impl Db,
    payer: ClientId,
    referrer: ClientId,
    now: DateTime<Utc>,
) -> Result<Option<serde_json::Value>, StoreError> {
    let since = now - Duration::days(signals::SHARED_SESSIONS_DAYS);
    let mine = Traces::of(db, payer, since).await?;
    let theirs = Traces::of(db, referrer, since).await?;
    Ok(mine.shared_with(&theirs))
}

async fn shared_device(
    ctx: &ModuleContext,
    payer: ClientId,
    referrer: ClientId,
) -> Option<serde_json::Value> {
    let found = match (
        ctx.core.client_devices(payer).await,
        ctx.core.client_devices(referrer).await,
    ) {
        (Ok(a), Ok(b)) => a.into_iter().any(|hwid| b.contains(&hwid)),
        (Err(err), _) | (_, Err(err)) => {
            tracing::warn!(error = %err, "антибот: устройства клиентов не получены");
            false
        }
    };
    found.then(|| serde_json::json!({ "device": true }))
}

async fn mass_invites(
    db: &mut impl Db,
    settings: &AntibotSettings,
    invite: Invite,
    now: DateTime<Utc>,
    known: &mut Option<Option<i64>>,
) -> Result<Option<usize>, StoreError> {
    if settings.mass_invites < 2 {
        return Ok(None);
    }
    let window = Duration::hours(signals::SIGNUP_WINDOW_HOURS);
    let around = store::invitees_around(
        db,
        invite.referrer,
        invite.invitee,
        invite.linked_at - window,
        invite.linked_at + window,
    )
    .await?;
    if around.len() < settings.mass_invites as usize {
        return Ok(None);
    }
    let boundary = match *known {
        Some(boundary) => boundary,
        None => {
            let boundary = young_boundary(db, settings, now).await?;
            *known = Some(boundary);
            boundary
        }
    };
    let fresh: Vec<ClientId> = around
        .iter()
        .filter(|i| i.marked || boundary.zip(i.telegram_id).is_some_and(|(b, id)| id > b))
        .map(|i| i.client_id)
        .collect();
    let many = fresh.len() >= settings.mass_invites as usize;
    Ok((many && fresh.contains(&invite.invitee)).then_some(fresh.len()))
}

pub async fn referral_gate(
    ctx: &ModuleContext,
    payer: ClientId,
) -> Result<ReferralGate, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let Some(settings) = active(&mut conn).await? else {
        return Ok(ReferralGate::Allow);
    };
    let Some((referrer, linked_at)) = store::referrer(&mut conn, payer).await? else {
        return Ok(ReferralGate::Allow);
    };
    let mark = store::mark_of(&mut conn, payer).await?;
    if mark.as_ref().is_some_and(|m| m.cleared_at.is_some()) {
        return Ok(ReferralGate::Allow);
    }
    let now = db_now(&mut conn).await?;
    if settings.referral_guard {
        let found = match shared_traces(&mut conn, payer, referrer, now).await? {
            Some(found) => Some(found),
            None => {
                drop(conn);
                let found = shared_device(ctx, payer, referrer).await;
                conn = ctx.store.conn().await?;
                found
            }
        };
        if let Some(details) = found {
            let details = serde_json::json!({ "self_invite": details, "referrer_id": referrer.0 });
            remember(&mut conn, payer, &[Signal::SelfInvite], &details).await?;
            return Ok(ReferralGate::NoDirect(Signal::SelfInvite));
        }
        let invite = Invite {
            referrer,
            invitee: payer,
            linked_at,
        };
        if let Some(count) = mass_invites(&mut conn, &settings, invite, now, &mut None).await? {
            let details = serde_json::json!({ "invites": count, "referrer_id": referrer.0 });
            remember(&mut conn, payer, &[Signal::MassInvites], &details).await?;
            return Ok(ReferralGate::NoDirect(Signal::MassInvites));
        }
    }
    if settings.mode == Mode::Block {
        let flagged = match &mark {
            Some(_) => true,
            None => assess(&mut conn, payer, &settings, now).await?.suspicious(),
        };
        if flagged {
            return Ok(ReferralGate::NoBonus);
        }
    }
    Ok(ReferralGate::Allow)
}

pub async fn record_no_bonus(
    ctx: &ModuleContext,
    payer: ClientId,
    payment_id: PaymentId,
    gate: &ReferralGate,
    skipped: &[i32],
) -> Result<(), ModuleError> {
    let reasons: Vec<Signal> = match gate {
        ReferralGate::Allow => return Ok(()),
        ReferralGate::NoDirect(signal) => vec![*signal],
        ReferralGate::NoBonus => {
            let mut conn = ctx.store.conn().await?;
            store::mark_of(&mut conn, payer)
                .await?
                .map(|m| m.reasons)
                .unwrap_or_default()
        }
    };
    let mut conn = ctx.store.conn().await?;
    let referrer = store::referrer(&mut conn, payer).await?.map(|(id, _)| id.0);
    store::event(
        &mut conn,
        &NewEvent {
            client_id: payer,
            action: Action::NoBonus,
            reasons: &reasons,
            details: serde_json::json!({
                "payment_id": payment_id.0,
                "referrer_id": referrer,
                "levels": skipped,
            }),
            dedupe: Some(format!("no_bonus:{}", payment_id.0)),
            admin: None,
        },
    )
    .await?;
    Ok(())
}

pub async fn note_surge(
    ctx: &ModuleContext,
    client_id: ClientId,
    per_minute: u32,
) -> Result<bool, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    if active(&mut conn).await?.is_none() {
        return Ok(false);
    }
    let premium = store::telegram(&mut conn, client_id)
        .await?
        .is_some_and(|telegram| telegram.premium);
    if premium {
        return Ok(false);
    }
    remember(
        &mut conn,
        client_id,
        &[Signal::Surge],
        &serde_json::json!({ "surge": per_minute }),
    )
    .await?;
    Ok(true)
}

pub async fn on_registered(ctx: &ModuleContext, client_id: ClientId) -> Result<(), ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let Some(settings) = active(&mut conn).await? else {
        return Ok(());
    };
    let now = db_now(&mut conn).await?;
    keep_boundary(&mut conn, &settings, now).await?;
    let assessment = assess(&mut conn, client_id, &settings, now).await?;
    if assessment.suspicious() {
        remember(
            &mut conn,
            client_id,
            &assessment.signals,
            &assessment.details_value(),
        )
        .await?;
    }
    Ok(())
}

pub struct AntibotModule;

#[async_trait]
impl Module for AntibotModule {
    fn id(&self) -> ModuleId {
        ModuleId::Antibot
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::select(
                "mode",
                "Что делать с подозрительными",
                "«Только отмечать» — подозрительные попадают в журнал; «Просить подтвердить» — кнопка «Я не робот» перед пробным периодом, подпиской по промо-ссылке, бесплатным вращением рулетки и участием в конкурсе; «Не давать пробник и бонусы» — без пробного периода, дней по промокодам, подписок по промо-ссылкам, бесплатных вращений рулетки, конкурсов и бонусов",
                MODE_OPTIONS,
            )
            .labels(&["Только отмечать", "Просить подтвердить", "Не давать пробник и бонусы"])
            .with_default("confirm"),
            SettingField::number(
                "young_days",
                "Новый аккаунт Telegram, дней",
                "Аккаунт новее всех, кто пришёл к вам раньше этого срока, считается новым; 0 — не учитывать",
            )
            .with_default("30"),
            SettingField::flag(
                "check_username",
                "Учитывать аккаунты без @username",
                "Сам по себе не делает аккаунт подозрительным, только вместе с другим признаком",
            )
            .with_default("true"),
            SettingField::number(
                "burst_count",
                "Серия регистраций",
                "Столько аккаунтов с соседними номерами Telegram за 10 минут; 0 — не учитывать",
            )
            .with_default("5"),
            SettingField::number(
                "ip_limit",
                "Регистраций на сайте с одного адреса за сутки без отметки",
                "Считаются регистрации по почте и через Google и Apple. Следующие аккаунты создаются, но отмечаются: что они получат, решает режим; 0 — не отмечать",
            )
            .with_default("3"),
            SettingField::number(
                "site_address_day",
                "Предел новых аккаунтов на сайте с одного адреса за сутки",
                "Сверх предела аккаунт не создаётся. За общим Wi-Fi и мобильным адресом людей много, разумное значение — 10 и больше; 0 — без предела",
            )
            .with_default("0"),
            SettingField::number(
                "site_network_day",
                "Предел новых аккаунтов на сайте с одной сети IPv6 за сутки",
                "Сеть /48. Сверх предела аккаунт не создаётся; 0 — без предела",
            )
            .with_default("0"),
            SettingField::number(
                "site_minute",
                "Предел новых аккаунтов на сайте в минуту на всю панель",
                "Сверх предела — капча, если она настроена, иначе отказ до конца минуты; 0 — без предела",
            )
            .with_default("0"),
            SettingField::text(
                "extra_domains",
                "Свои одноразовые почтовые домены",
                "Через запятую, в дополнение к встроенному списку",
            )
            .optional(),
            SettingField::flag(
                "referral_guard",
                "Не начислять за самоприглашение и массовые приглашения",
                "Общий адрес, устройство или почта с пригласившим, либо много новых аккаунтов от одного клиента за сутки",
            )
            .with_default("true"),
            SettingField::number(
                "mass_invites",
                "Массовые приглашения",
                "Столько новых аккаунтов от одного клиента за сутки; 0 — не учитывать",
            )
            .with_default("5"),
            SettingField::flag(
                "roulette_guard",
                "Защищать рулетку",
                "Бесплатные вращения подозрительным — по режиму антибота; вращения за покупки не трогаются",
            )
            .with_default("true"),
            SettingField::flag(
                "contest_guard",
                "Защищать конкурсы",
                "Участие подозрительных и зачёт приглашённых друзей — по режиму антибота; самоприглашения и массовые приглашения не засчитываются никогда",
            )
            .with_default("true"),
            SettingField::select(
                "captcha",
                "Капча",
                "Внешняя проверка «Я не робот» перед пробным периодом и при регистрации по почте. Без неё кнопка «Я не робот» проверяется в браузере, но скрипт, написанный под вашу панель, её пройдёт",
                captcha::provider_options(),
            )
            .labels(&["Без внешней капчи", "Cloudflare Turnstile", "Яндекс SmartCaptcha"])
            .with_default("none"),
            SettingField::text(
                "captcha_site_key",
                "Ключ сайта капчи",
                "Публичный ключ из Cloudflare (Turnstile) или Яндекс Облака (SmartCaptcha), выданный для домена панели",
            )
            .optional(),
            SettingField::secret(
                "captcha_secret",
                "Секретный ключ капчи",
                "Хранится зашифрованным и нужен только серверу для проверки ответа",
            )
            .optional(),
            SettingField::select(
                "captcha_trial",
                "Капча перед пробным периодом",
                "«Всем» — каждый проходит капчу перед пробником; «Только подозрительным» — капча вместо кнопки «Я не робот» в режиме «Просить подтвердить»",
                captcha::scope_options(),
            )
            .labels(&["Всем", "Только подозрительным"])
            .with_default("all"),
            SettingField::flag(
                "captcha_signup",
                "Капча при регистрации по почте",
                "В кабинете на сайте: в форме регистрации по паролю, а при входе кодом из письма — перед отправкой кода",
            )
            .with_default("true"),
        ]
    }

    fn validate_config(&self, config: &ModuleConfig) -> Result<(), ModuleError> {
        AntibotSettings::check_captcha(config).map_err(ModuleError::Invalid)
    }

    fn validate_change(
        &self,
        before: &ModuleConfig,
        after: &ModuleConfig,
        new_secrets: &[&str],
    ) -> Result<(), ModuleError> {
        let was = AntibotSettings::lenient(before).captcha;
        let now = AntibotSettings::lenient(after).captcha;
        if was != now
            && was != Provider::None
            && now != Provider::None
            && !new_secrets.contains(&"captcha_secret")
        {
            return Err(ModuleError::SecretsAgain(format!(
                "При смене капчи на {} вставьте заново секретный ключ: ключ одного сервиса другому не подходит",
                now.title()
            )));
        }
        let secret = after
            .text("captcha_secret")
            .map(str::trim)
            .unwrap_or_default();
        if let Some(made) = captcha::foreign(now, secret) {
            let text = captcha::foreign_text(now, made);
            return Err(if new_secrets.contains(&"captcha_secret") {
                ModuleError::Invalid(text)
            } else {
                ModuleError::SecretsAgain(format!("{text}: вставьте секретный ключ заново"))
            });
        }
        Ok(())
    }

    fn config_warning(&self, config: &ModuleConfig) -> Option<&'static str> {
        let settings = AntibotSettings::lenient(config);
        (settings.captcha != Provider::None && Captcha::of(config).is_none()).then_some(
            "Капча выбрана, но ключи не заданы: пробный период и регистрация идут без неё",
        )
    }

    fn cleanup_tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: SWEEP_TASK,
            every: std::time::Duration::from_secs(3600),
        }]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        if task != SWEEP_TASK {
            return Err(ModuleError::Invalid(format!("у модуля нет задачи {task}")));
        }
        let mut conn = ctx.store.conn().await?;
        captcha::sweep(&mut conn).await?;
        signups::sweep(&mut conn).await?;
        Ok(())
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    async fn on_event(&self, ctx: &ModuleContext, event: &DomainEvent) -> Result<(), ModuleError> {
        match event {
            DomainEvent::ClientRegistered { client_id, .. } => on_registered(ctx, *client_id).await,
            _ => Ok(()),
        }
    }
}
