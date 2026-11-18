mod routes;
pub mod signals;
pub mod store;
#[cfg(test)]
mod tests;

use crate::catalog::config_flag;
use crate::catalog::web::{address_network, is_internal};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{Module, ModuleConfig};
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use signals::Signal;
use sn_domain::events::DomainEvent;
use sn_domain::ids::{AdminId, ClientId, PaymentId};
use sn_domain::module::ModuleId;
use sn_domain::settings::SettingField;
use sn_store::{Db, StoreError};
use std::collections::HashSet;
use std::net::IpAddr;
use store::{Action, ChallengeCheck, NewEvent};
use uuid::Uuid;

pub use store::{JournalFilter, JournalItem, Mark, MarkItem, Summary};

pub const CONFIRM_CODE: &str = "antibot.confirm";
pub const DENIED_CODE: &str = "antibot.denied";
pub const CONFIRM_MIN_SECONDS: i64 = 1;
pub const CONFIRM_TTL_MINUTES: i64 = 15;
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
        })
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
    let cutoff = now - Duration::days(i64::from(settings.young_days));
    let (count, max) = store::telegram_history(db, cutoff).await?;
    Ok(max.filter(|_| count >= signals::MIN_HISTORY))
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
        if let Some(boundary) = young_boundary(db, settings, now).await?
            && telegram.id > boundary
        {
            found.add(
                Signal::YoungAccount,
                Some(("newer_than", serde_json::json!(boundary))),
            );
        }
        if settings.check_username && telegram.username.is_none() {
            found.add(Signal::NoUsername, None);
        }
        if settings.burst_count > 1 {
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
            let earlier = store::web_signups(
                db,
                client_id,
                registration.created_at - Duration::hours(signals::SIGNUP_WINDOW_HOURS),
                registration.created_at,
                grace,
            )
            .await?
            .iter()
            .filter(|ip| network(ip).as_deref() == Some(own.as_str()))
            .count();
            if earlier >= settings.ip_limit as usize {
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
    let mark = store::mark_of(db, client_id).await?;
    if mark.as_ref().is_some_and(|m| m.cleared_at.is_some()) {
        return Ok(Some(Verdict {
            settings,
            mark,
            signals: Vec::new(),
        }));
    }
    let now = db_now(db).await?;
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
    Ok(Some(Verdict {
        settings,
        mark,
        signals,
    }))
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

pub async fn trial_gate(ctx: &ModuleContext, client_id: ClientId) -> Result<Gate, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let Some(verdict) = verdict(&mut conn, client_id).await? else {
        return Ok(Gate::Allow);
    };
    if !verdict.flagged() {
        return Ok(Gate::Allow);
    }
    let now = db_now(&mut conn).await?;
    let (gate, action) = match verdict.settings.mode {
        Mode::Mark => return Ok(Gate::Allow),
        Mode::Confirm if verdict.confirmed() => return Ok(Gate::Allow),
        Mode::Confirm => (Gate::Confirm, Action::Asked),
        Mode::Block => (Gate::Deny, Action::TrialDenied),
    };
    store::event(
        &mut conn,
        &NewEvent {
            client_id,
            action,
            reasons: &verdict.signals,
            details: serde_json::json!({ "target": "trial" }),
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

pub async fn challenge(ctx: &ModuleContext, client_id: ClientId) -> Result<String, ModuleError> {
    let token = Uuid::new_v4().simple().to_string();
    let mut conn = ctx.store.conn().await?;
    store::set_challenge(&mut conn, client_id, &token).await?;
    Ok(token)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Confirmed,
    TooFast,
    Expired,
}

pub async fn confirm(
    ctx: &ModuleContext,
    client_id: ClientId,
    token: &str,
) -> Result<Confirmation, ModuleError> {
    let token = token.trim();
    if token.is_empty() || token.len() > 64 {
        return Ok(Confirmation::Expired);
    }
    let mut conn = ctx.store.conn().await?;
    let check = store::take_challenge(
        &mut conn,
        client_id,
        token,
        CONFIRM_MIN_SECONDS,
        CONFIRM_TTL_MINUTES,
    )
    .await?;
    Ok(match check {
        ChallengeCheck::Passed => {
            let reasons = store::mark_of(&mut conn, client_id)
                .await?
                .map(|m| m.reasons)
                .unwrap_or_default();
            store::event(
                &mut conn,
                &NewEvent {
                    client_id,
                    action: Action::Confirmed,
                    reasons: &reasons,
                    details: serde_json::json!({}),
                    dedupe: Some(format!("confirmed:{client_id}")),
                    admin: None,
                },
            )
            .await?;
            Confirmation::Confirmed
        }
        ChallengeCheck::TooFast => Confirmation::TooFast,
        ChallengeCheck::Wrong => Confirmation::Expired,
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

async fn self_invite(
    ctx: &ModuleContext,
    db: &mut impl Db,
    payer: ClientId,
    referrer: ClientId,
    now: DateTime<Utc>,
) -> Result<Option<serde_json::Value>, ModuleError> {
    let since = now - Duration::days(signals::SHARED_SESSIONS_DAYS);
    let mine = store::session_ips(db, payer, since).await?;
    let theirs = store::session_ips(db, referrer, since).await?;
    if let Some(network) = shared(&mine, &theirs) {
        return Ok(Some(serde_json::json!({ "network": network })));
    }
    if let (Some(a), Some(b)) = (
        store::email(db, payer).await?,
        store::email(db, referrer).await?,
    ) && signals::email_key(&a) == signals::email_key(&b)
    {
        return Ok(Some(serde_json::json!({ "email": true })));
    }
    let devices = match (
        ctx.core.client_devices(payer).await,
        ctx.core.client_devices(referrer).await,
    ) {
        (Ok(a), Ok(b)) => a.into_iter().find(|hwid| b.contains(hwid)),
        (Err(err), _) | (_, Err(err)) => {
            tracing::warn!(error = %err, "антибот: устройства клиентов не получены");
            None
        }
    };
    Ok(devices.map(|_| serde_json::json!({ "device": true })))
}

async fn mass_invites(
    db: &mut impl Db,
    settings: &AntibotSettings,
    referrer: ClientId,
    linked_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<usize>, StoreError> {
    if settings.mass_invites < 2 {
        return Ok(None);
    }
    let window = Duration::hours(signals::SIGNUP_WINDOW_HOURS);
    let around =
        store::invitees_around(db, referrer, linked_at - window, linked_at + window).await?;
    if around.len() < settings.mass_invites as usize {
        return Ok(None);
    }
    let boundary = young_boundary(db, settings, now).await?;
    let fresh = around
        .iter()
        .filter(|i| i.marked || boundary.zip(i.telegram_id).is_some_and(|(b, id)| id > b))
        .count();
    Ok((fresh >= settings.mass_invites as usize).then_some(fresh))
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
        if let Some(details) = self_invite(ctx, &mut conn, payer, referrer, now).await? {
            let details = serde_json::json!({ "self_invite": details, "referrer_id": referrer.0 });
            remember(&mut conn, payer, &[Signal::SelfInvite], &details).await?;
            return Ok(ReferralGate::NoDirect(Signal::SelfInvite));
        }
        if let Some(count) = mass_invites(&mut conn, &settings, referrer, linked_at, now).await? {
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

pub async fn on_registered(ctx: &ModuleContext, client_id: ClientId) -> Result<(), ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let Some(settings) = active(&mut conn).await? else {
        return Ok(());
    };
    let now = db_now(&mut conn).await?;
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
                "mark — только отмечать; confirm — просить нажать кнопку «Я не робот» перед пробным периодом; block — не давать пробный период, дни по промокодам, подписки по промо-ссылкам и бонусы",
                MODE_OPTIONS,
            )
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
                "Регистраций в кабинете с одного адреса за сутки",
                "Следующие регистрации с этого адреса отмечаются; 0 — без лимита",
            )
            .with_default("3"),
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
        ]
    }

    fn validate_config(&self, config: &ModuleConfig) -> Result<(), ModuleError> {
        AntibotSettings::parse(config)
            .map(|_| ())
            .map_err(ModuleError::Invalid)
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
