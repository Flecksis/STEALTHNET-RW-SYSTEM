mod routes;
mod store;
#[cfg(test)]
mod tests;

use crate::catalog::{config_flag, installation_currency, module_config, start_param};
use crate::context::{BalanceChange, ModuleContext, Notification};
use crate::error::ModuleError;
use crate::module::{BotEntry, Module, ModuleConfig, ScheduledTask};
use async_trait::async_trait;
use axum::Router;
use serde::{Deserialize, Serialize};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::{AdminId, ClientId, PaymentId};
use sn_domain::ledger::{LedgerKind, Wallet};
use sn_domain::module::ModuleId;
use sn_domain::money::{Amount, Currency, Money, PayCurrency, parse_units};
use sn_domain::payment::{PaymentStatus, ProviderId, providers};
use sn_domain::settings::{SettingField, keys};
use sn_store::{Db, StoreError};
use std::time::Duration;
use uuid::Uuid;

pub use store::{Withdrawal, WithdrawalFilter, WithdrawalStatus};

pub const BOT_ENTRY: &str = "referral";
pub const LEVELS: usize = 3;
pub const SETTLE_TASK: &str = "settle";
pub const TEMPLATE_CREDIT: &str = "referral_credit";
pub const TEMPLATE_WITHDRAWAL_PAID: &str = "referral_withdrawal_paid";
pub const TEMPLATE_WITHDRAWAL_REJECTED: &str = "referral_withdrawal_rejected";
const DEFAULT_LEVELS: [u8; LEVELS] = [30, 10, 10];
const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReferralSettings {
    pub levels: [u8; LEVELS],
    pub withdraw_min: Option<Money>,
    pub withdrawals: bool,
}

fn percent_field(config: &ModuleConfig, key: &str, default: u8) -> Result<u8, String> {
    match config.get(key) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(default),
        Some(_) => crate::catalog::config_number(config, key)
            .and_then(|n| u8::try_from(n).ok())
            .filter(|n| *n <= 100)
            .ok_or_else(|| format!("{key}: процент от 0 до 100")),
    }
}

fn money_field(
    config: &ModuleConfig,
    key: &str,
    currency: Currency,
) -> Result<Option<Money>, String> {
    let raw = match config.get(key) {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(_) => return Err(format!("{key}: ожидается сумма")),
    };
    if raw.is_empty() {
        return Ok(None);
    }
    parse_units(&raw, PayCurrency::from(currency))
        .map(|minor| Some(Money::new(minor, currency)))
        .ok_or_else(|| format!("{key}: сумма указана неверно"))
}

impl ReferralSettings {
    pub fn parse(config: &ModuleConfig, currency: Currency) -> Result<Self, String> {
        let levels = [
            percent_field(config, "level1_percent", DEFAULT_LEVELS[0])?,
            percent_field(config, "level2_percent", DEFAULT_LEVELS[1])?,
            percent_field(config, "level3_percent", DEFAULT_LEVELS[2])?,
        ];
        if levels.iter().map(|l| u32::from(*l)).sum::<u32>() > 100 {
            return Err("сумма процентов всех уровней больше 100".into());
        }
        Ok(ReferralSettings {
            levels,
            withdraw_min: money_field(config, "withdraw_min", currency)?,
            withdrawals: config_flag(config, "withdrawals").unwrap_or(true),
        })
    }

    pub fn lenient(config: &ModuleConfig, currency: Currency) -> Self {
        Self::parse(config, currency).unwrap_or_else(|reason| {
            tracing::warn!(%reason, "настройки рефералов не разобраны, взяты значения по умолчанию");
            ReferralSettings {
                levels: DEFAULT_LEVELS,
                withdraw_min: None,
                withdrawals: true,
            }
        })
    }
}

async fn settings(db: &mut impl Db, currency: Currency) -> Result<ReferralSettings, StoreError> {
    let config = module_config(db, ModuleId::Referral).await?;
    Ok(ReferralSettings::lenient(&config, currency))
}

pub fn generate_code() -> String {
    Uuid::new_v4()
        .as_bytes()
        .iter()
        .take(8)
        .map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char)
        .collect()
}

pub fn invite_link(bot_username: &str, code: &str) -> String {
    format!(
        "https://t.me/{}?start=ref_{code}",
        bot_username.trim_start_matches('@')
    )
}

pub fn earns_referral(provider: &ProviderId, amount: Amount) -> bool {
    !provider.is_internal() && provider.as_str() != providers::STARS && amount.as_money().is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayoutMethod {
    Sbp,
    Card,
    UsdtTrc20,
}

impl PayoutMethod {
    pub const ALL: [PayoutMethod; 3] = [
        PayoutMethod::Sbp,
        PayoutMethod::Card,
        PayoutMethod::UsdtTrc20,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            PayoutMethod::Sbp => "sbp",
            PayoutMethod::Card => "card",
            PayoutMethod::UsdtTrc20 => "usdt_trc20",
        }
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            PayoutMethod::Sbp => "СБП",
            PayoutMethod::Card => "Банковская карта",
            PayoutMethod::UsdtTrc20 => "USDT (TRC20)",
        }
    }

    pub const fn details_hint(self) -> &'static str {
        match self {
            PayoutMethod::Sbp => "Номер телефона и банк, например: +7 900 123-45-67, Т-Банк",
            PayoutMethod::Card => "Номер карты, 16 цифр",
            PayoutMethod::UsdtTrc20 => "Адрес кошелька TRC20, начинается с T",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == value)
    }

    pub fn normalize_details(self, raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 200 {
            return None;
        }
        match self {
            PayoutMethod::Sbp => {
                let digits = trimmed.chars().filter(char::is_ascii_digit).count();
                (10..=15)
                    .contains(&digits)
                    .then(|| trimmed.split_whitespace().collect::<Vec<_>>().join(" "))
            }
            PayoutMethod::Card => {
                let digits: String = trimmed
                    .chars()
                    .filter(|c| !matches!(c, ' ' | '-'))
                    .collect();
                ((13..=19).contains(&digits.len()) && luhn(&digits)).then_some(digits)
            }
            PayoutMethod::UsdtTrc20 => {
                let valid = trimmed.len() == 34
                    && trimmed.starts_with('T')
                    && trimmed.bytes().all(|b| {
                        b.is_ascii_alphanumeric() && !matches!(b, b'0' | b'O' | b'I' | b'l')
                    });
                valid.then(|| trimmed.to_string())
            }
        }
    }
}

pub fn luhn(digits: &str) -> bool {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let sum: u32 = digits
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| {
            let d = u32::from(b - b'0');
            if i % 2 == 1 {
                let doubled = d * 2;
                if doubled > 9 { doubled - 9 } else { doubled }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ReferralRejection {
    WithdrawalsOff,
    BadAmount,
    BelowMinimum { min: Money },
    NotEnough { balance: Money },
    BadDetails,
    AlreadyDecided,
    NeedComment,
}

impl ReferralRejection {
    pub const fn code(self) -> &'static str {
        match self {
            ReferralRejection::WithdrawalsOff => "referral.withdrawals_off",
            ReferralRejection::BadAmount => "referral.bad_amount",
            ReferralRejection::BelowMinimum { .. } => "referral.below_minimum",
            ReferralRejection::NotEnough { .. } => "referral.not_enough",
            ReferralRejection::BadDetails => "referral.bad_details",
            ReferralRejection::AlreadyDecided => "referral.already_decided",
            ReferralRejection::NeedComment => "referral.need_comment",
        }
    }

    pub fn message(self) -> String {
        match self {
            ReferralRejection::WithdrawalsOff => "Вывод сейчас не принимается".into(),
            ReferralRejection::BadAmount => "Сумма указана неверно".into(),
            ReferralRejection::BelowMinimum { min } => {
                format!("Минимальная сумма вывода — {}", format::money(min))
            }
            ReferralRejection::NotEnough { balance } => format!(
                "На реферальном балансе {}, этого не хватает",
                format::money(balance)
            ),
            ReferralRejection::BadDetails => "Реквизиты указаны неверно".into(),
            ReferralRejection::AlreadyDecided => "Заявка уже обработана".into(),
            ReferralRejection::NeedComment => "Укажите причину отказа".into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReferralError {
    #[error("{}", .0.message())]
    Rejected(ReferralRejection),
    #[error(transparent)]
    Module(#[from] ModuleError),
}

impl From<StoreError> for ReferralError {
    fn from(err: StoreError) -> Self {
        ReferralError::Module(ModuleError::Store(err))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferralOverview {
    pub code: String,
    pub bot_link: Option<String>,
    pub invited: u32,
    pub earned: Money,
    pub balance: Money,
    pub levels: [u8; LEVELS],
    pub withdraw_min: Option<Money>,
    pub withdrawals_enabled: bool,
    pub withdrawals: Vec<Withdrawal>,
}

pub async fn ensure_code(db: &mut impl Db, client_id: ClientId) -> Result<String, StoreError> {
    if let Some(code) = store::code_of(db, client_id).await? {
        return Ok(code);
    }
    for _ in 0..8 {
        if let Some(code) = store::insert_code(db, client_id, &generate_code()).await? {
            return Ok(code);
        }
        if let Some(code) = store::code_of(db, client_id).await? {
            return Ok(code);
        }
    }
    Err(StoreError::Conflict {
        constraint: Some("referral_codes_code_key".into()),
        message: "не удалось подобрать свободный реферальный код".into(),
    })
}

pub async fn overview(
    ctx: &ModuleContext,
    client_id: ClientId,
) -> Result<ReferralOverview, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let settings = settings(&mut conn, currency).await?;
    let code = ensure_code(&mut conn, client_id).await?;
    let personal = store::personal_percent(&mut conn, client_id).await?;
    let bot_link = conn
        .settings()
        .get(keys::BOT_USERNAME)
        .await?
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|u| !u.trim().is_empty())
        .map(|username| invite_link(&username, &code));
    let invited = store::invited_count(&mut conn, client_id).await?;
    let earned = store::earned(&mut conn, client_id, currency).await?;
    let balance = conn
        .ledger()
        .balance(client_id, Wallet::Referral, currency)
        .await?;
    let withdrawals = store::client_withdrawals(&mut conn, client_id, 10).await?;
    let mut levels = settings.levels;
    if let Some(personal) = personal {
        levels[0] = personal;
    }
    Ok(ReferralOverview {
        code,
        bot_link,
        invited,
        earned,
        balance,
        levels,
        withdraw_min: settings.withdraw_min,
        withdrawals_enabled: settings.withdrawals,
        withdrawals,
    })
}

pub async fn link_by_start_param(
    ctx: &ModuleContext,
    client_id: ClientId,
    start_param: &str,
) -> Result<bool, ModuleError> {
    let Some(code) = start_param::referral_code(start_param) else {
        return Ok(false);
    };
    let mut tx = ctx.store.begin().await?;
    let Some(referrer) = store::owner_of_code(&mut tx, &code).await? else {
        tx.commit().await?;
        return Ok(false);
    };
    if referrer == client_id {
        tx.commit().await?;
        return Ok(false);
    }
    let ancestors = store::ancestors(&mut tx, referrer, 16).await?;
    if ancestors.iter().any(|a| a.referrer_id == client_id) {
        tx.commit().await?;
        return Ok(false);
    }
    let linked = store::link(&mut tx, client_id, referrer).await?;
    tx.commit().await?;
    Ok(linked)
}

pub async fn accrue(
    ctx: &ModuleContext,
    payment_id: PaymentId,
    payer: ClientId,
    amount: Amount,
    provider: &ProviderId,
) -> Result<usize, ModuleError> {
    if !earns_referral(provider, amount) {
        return Ok(0);
    }
    let Some(base) = amount.as_money().filter(|m| m.minor > 0) else {
        return Ok(0);
    };
    let mut tx = ctx.store.begin().await?;
    if !store::mark_processed(&mut tx, payment_id).await? {
        tx.commit().await?;
        settle_payment(ctx, payment_id).await?;
        return Ok(0);
    }
    let settings = settings(&mut tx, base.currency).await?;
    let ancestors = store::ancestors(&mut tx, payer, LEVELS as i32).await?;
    let mut seen = vec![payer];
    let mut created = 0;
    for ancestor in ancestors {
        if seen.contains(&ancestor.referrer_id) {
            break;
        }
        seen.push(ancestor.referrer_id);
        let index = (ancestor.level as usize).saturating_sub(1);
        if index >= LEVELS {
            break;
        }
        let percent = match (index, ancestor.personal_percent) {
            (0, Some(personal)) => personal,
            _ => settings.levels[index],
        };
        if percent == 0 || ancestor.blocked {
            continue;
        }
        let credit = base
            .percent(percent)
            .map_err(|e| ModuleError::Invalid(e.to_string()))?;
        if credit.minor <= 0 {
            continue;
        }
        if store::insert_credit(
            &mut tx,
            &store::NewCredit {
                payment_id,
                level: ancestor.level,
                referrer_id: ancestor.referrer_id,
                referral_id: payer,
                base,
                percent,
                amount: credit,
            },
        )
        .await?
        {
            created += 1;
        }
    }
    tx.commit().await?;
    settle_payment(ctx, payment_id).await?;
    Ok(created)
}

fn reversal_for(
    amount_minor: i64,
    base_minor: i64,
    refunded_minor: i64,
    currency: Currency,
) -> i64 {
    if base_minor <= 0 || refunded_minor <= 0 {
        return 0;
    }
    if refunded_minor >= base_minor {
        return amount_minor;
    }
    let step = i128::from(currency.rounding_step());
    let raw = i128::from(amount_minor) * i128::from(refunded_minor);
    let denominator = i128::from(base_minor) * step;
    let units = (2 * raw + denominator) / (2 * denominator);
    i64::try_from(units * step)
        .unwrap_or(amount_minor)
        .min(amount_minor)
}

pub async fn schedule_reversal(
    ctx: &ModuleContext,
    payment_id: PaymentId,
) -> Result<usize, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let payment = tx.payments().get(payment_id).await?;
    let refunded = match payment.status {
        PaymentStatus::Refunded => payment.amount.minor,
        _ => store::refunded_minor(&mut tx, payment_id).await?,
    };
    let credits = store::credits_for_payment(&mut tx, payment_id).await?;
    let mut changed = 0;
    for credit in &credits {
        let target = reversal_for(
            credit.amount.minor,
            credit.base_minor,
            refunded,
            credit.amount.currency,
        );
        if target > credit.reversal_target_minor {
            store::set_reversal_target(&mut tx, credit.id, target).await?;
            changed += 1;
        }
    }
    tx.commit().await?;
    settle_payment(ctx, payment_id).await?;
    Ok(changed)
}

async fn settle_payment(ctx: &ModuleContext, payment_id: PaymentId) -> Result<(), ModuleError> {
    let ids = {
        let mut conn = ctx.store.conn().await?;
        store::unsettled_for_payment(&mut conn, payment_id).await?
    };
    for id in ids {
        settle_credit(ctx, id).await?;
    }
    Ok(())
}

async fn reverse_step(ctx: &ModuleContext, credit: &store::Credit) -> Result<i64, ModuleError> {
    let base = credit.reversed_minor;
    let wanted = credit.reversal_target_minor - base;
    if wanted <= 0 {
        return Ok(base);
    }
    let currency = credit.amount.currency;
    let key = format!(
        "referral_reversal:{}:{}:{}",
        credit.payment_id, credit.level, base
    );
    let (known, available) = {
        let mut conn = ctx.store.conn().await?;
        (
            store::reversal_applied(&mut conn, &key).await?,
            conn.ledger()
                .balance(credit.referrer_id, Wallet::Referral, currency)
                .await?,
        )
    };
    let charged = match known {
        Some(_) => None,
        None => {
            let take = wanted.min(available.minor);
            if take <= 0 {
                return Ok(base);
            }
            Some(
                ctx.core
                    .change_balance(BalanceChange {
                        client_id: credit.referrer_id,
                        wallet: Wallet::Referral,
                        amount: Money::new(-take, currency),
                        kind: LedgerKind::ReferralReversal,
                        comment: Some(format!(
                            "Возврат оплаты: сторно реферального начисления, уровень {}",
                            credit.level
                        )),
                        idempotency_key: key.clone(),
                        order_id: None,
                        payment_id: Some(credit.payment_id),
                    })
                    .await,
            )
        }
    };
    let mut conn = ctx.store.conn().await?;
    let applied = match (known, charged) {
        (Some(applied), _) => applied,
        (None, charged) => match (store::reversal_applied(&mut conn, &key).await?, charged) {
            (Some(applied), _) => applied,
            (None, Some(Err(err))) => return Err(err),
            (None, _) => {
                return Err(ModuleError::Core(
                    "сторно реферального начисления не записано в журнал баланса".into(),
                ));
            }
        },
    };
    let reversed = base + applied;
    store::advance_reversed(&mut conn, credit.id, base, reversed).await?;
    Ok(reversed)
}

pub async fn settle_credit(ctx: &ModuleContext, id: i64) -> Result<bool, ModuleError> {
    let found = {
        let mut conn = ctx.store.conn().await?;
        store::credit(&mut conn, id).await?
    };
    let Some(credit) = found else {
        return Ok(false);
    };
    if credit.credited_at.is_none() {
        ctx.core
            .change_balance(BalanceChange {
                client_id: credit.referrer_id,
                wallet: Wallet::Referral,
                amount: credit.amount,
                kind: LedgerKind::ReferralCredit,
                comment: Some(format!("Реферальное начисление, уровень {}", credit.level)),
                idempotency_key: format!("referral:{}:{}", credit.payment_id, credit.level),
                order_id: None,
                payment_id: Some(credit.payment_id),
            })
            .await?;
        let first = {
            let mut conn = ctx.store.conn().await?;
            store::mark_credited(&mut conn, credit.id).await?
        };
        if first
            && let Err(err) = ctx
                .core
                .notify(
                    credit.referrer_id,
                    Notification {
                        template: TEMPLATE_CREDIT.into(),
                        params: serde_json::json!({
                            "amount": format::money(credit.amount),
                            "level": credit.level,
                        }),
                    },
                )
                .await
        {
            tracing::warn!(error = %err, "уведомление о реферальном начислении не поставлено");
        }
    }
    let reversed = reverse_step(ctx, &credit).await?;
    Ok(reversed >= credit.reversal_target_minor)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WithdrawalRequest {
    pub amount: String,
    pub method: PayoutMethod,
    pub details: String,
}

pub async fn request_withdrawal(
    ctx: &ModuleContext,
    client_id: ClientId,
    request: &WithdrawalRequest,
) -> Result<Withdrawal, ReferralError> {
    let rejected = ReferralError::Rejected;
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let settings = settings(&mut conn, currency).await?;
    if !settings.withdrawals {
        return Err(rejected(ReferralRejection::WithdrawalsOff));
    }
    let minor = parse_units(request.amount.trim(), PayCurrency::from(currency))
        .filter(|m| *m > 0)
        .ok_or(rejected(ReferralRejection::BadAmount))?;
    let amount = Money::new(minor, currency);
    if let Some(min) = settings.withdraw_min
        && amount.minor < min.minor
    {
        return Err(rejected(ReferralRejection::BelowMinimum { min }));
    }
    let details = request
        .method
        .normalize_details(&request.details)
        .ok_or(rejected(ReferralRejection::BadDetails))?;
    let balance = conn
        .ledger()
        .balance(client_id, Wallet::Referral, currency)
        .await?;
    if balance.minor < amount.minor {
        return Err(rejected(ReferralRejection::NotEnough { balance }));
    }
    let created =
        store::insert_withdrawal(&mut conn, client_id, amount, request.method, &details).await?;
    drop(conn);
    match hold(ctx, &created).await {
        Ok(withdrawal) => Ok(withdrawal),
        Err(HoldError::NotEnough) => Err(rejected(ReferralRejection::NotEnough { balance })),
        Err(HoldError::Module(err)) => Err(err.into()),
    }
}

enum HoldError {
    NotEnough,
    Module(ModuleError),
}

async fn hold(ctx: &ModuleContext, withdrawal: &Withdrawal) -> Result<Withdrawal, HoldError> {
    let held = ctx
        .core
        .change_balance(BalanceChange {
            client_id: withdrawal.client_id,
            wallet: Wallet::Referral,
            amount: Money::new(-withdrawal.amount.minor, withdrawal.amount.currency),
            kind: LedgerKind::WithdrawalHold,
            comment: Some(format!("Заявка на вывод №{}", withdrawal.id)),
            idempotency_key: format!("withdrawal:{}:hold", withdrawal.id),
            order_id: None,
            payment_id: None,
        })
        .await;
    let mut conn = ctx
        .store
        .conn()
        .await
        .map_err(|e| HoldError::Module(e.into()))?;
    match held {
        Ok(_) => store::mark_held(&mut conn, withdrawal.id)
            .await
            .map_err(|e| HoldError::Module(e.into())),
        Err(ModuleError::Core(reason)) => {
            store::cancel_unheld(&mut conn, withdrawal.id, &reason)
                .await
                .map_err(|e| HoldError::Module(e.into()))?;
            Err(HoldError::NotEnough)
        }
        Err(err) => Err(HoldError::Module(err)),
    }
}

async fn release(ctx: &ModuleContext, withdrawal: &Withdrawal) -> Result<(), ModuleError> {
    ctx.core
        .change_balance(BalanceChange {
            client_id: withdrawal.client_id,
            wallet: Wallet::Referral,
            amount: withdrawal.amount,
            kind: LedgerKind::WithdrawalRelease,
            comment: Some(format!("Возврат заявки на вывод №{}", withdrawal.id)),
            idempotency_key: format!("withdrawal:{}:release", withdrawal.id),
            order_id: None,
            payment_id: None,
        })
        .await?;
    let mut conn = ctx.store.conn().await?;
    store::mark_released(&mut conn, withdrawal.id).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Paid { comment: Option<String> },
    Rejected { comment: String },
}

pub async fn decide(
    ctx: &ModuleContext,
    id: i64,
    decision: &Decision,
    admin: Option<AdminId>,
) -> Result<Withdrawal, ReferralError> {
    let (status, comment) = match decision {
        Decision::Paid { comment } => (
            WithdrawalStatus::Paid,
            comment.as_deref().map(str::trim).filter(|c| !c.is_empty()),
        ),
        Decision::Rejected { comment } => {
            let comment = comment.trim();
            if comment.is_empty() {
                return Err(ReferralError::Rejected(ReferralRejection::NeedComment));
            }
            (WithdrawalStatus::Rejected, Some(comment))
        }
    };
    let decided = {
        let mut conn = ctx.store.conn().await?;
        store::decide(&mut conn, id, None, status, comment, admin).await?
    };
    let Some(withdrawal) = decided else {
        let mut conn = ctx.store.conn().await?;
        return match store::withdrawal(&mut conn, id).await? {
            Some(_) => Err(ReferralError::Rejected(ReferralRejection::AlreadyDecided)),
            None => Err(ModuleError::NotFound("заявка на вывод").into()),
        };
    };
    if status == WithdrawalStatus::Rejected {
        release(ctx, &withdrawal).await?;
    }
    let template = match status {
        WithdrawalStatus::Paid => TEMPLATE_WITHDRAWAL_PAID,
        _ => TEMPLATE_WITHDRAWAL_REJECTED,
    };
    if let Err(err) = ctx
        .core
        .notify(
            withdrawal.client_id,
            Notification {
                template: template.into(),
                params: serde_json::json!({
                    "amount": format::money(withdrawal.amount),
                    "comment": withdrawal.comment,
                }),
            },
        )
        .await
    {
        tracing::warn!(error = %err, "уведомление о заявке на вывод не поставлено");
    }
    Ok(withdrawal)
}

pub async fn cancel_withdrawal(
    ctx: &ModuleContext,
    client_id: ClientId,
    id: i64,
) -> Result<Withdrawal, ReferralError> {
    let canceled = {
        let mut conn = ctx.store.conn().await?;
        store::decide(
            &mut conn,
            id,
            Some(client_id),
            WithdrawalStatus::Canceled,
            Some("Отменена клиентом"),
            None,
        )
        .await?
    };
    let Some(withdrawal) = canceled else {
        let mut conn = ctx.store.conn().await?;
        return match store::withdrawal(&mut conn, id).await? {
            Some(w) if w.client_id == client_id => {
                Err(ReferralError::Rejected(ReferralRejection::AlreadyDecided))
            }
            _ => Err(ModuleError::NotFound("заявка на вывод").into()),
        };
    };
    release(ctx, &withdrawal).await?;
    Ok(withdrawal)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SettleReport {
    pub payments: usize,
    pub credits: usize,
    pub holds: usize,
    pub releases: usize,
}

pub async fn settle(ctx: &ModuleContext, limit: i64) -> Result<SettleReport, ModuleError> {
    let mut report = SettleReport::default();
    let missed = {
        let mut conn = ctx.store.conn().await?;
        store::unprocessed_payments(&mut conn, limit).await?
    };
    for payment in missed {
        accrue(
            ctx,
            payment.payment_id,
            payment.client_id,
            payment.amount,
            &payment.provider,
        )
        .await?;
        if payment.status != PaymentStatus::Paid {
            schedule_reversal(ctx, payment.payment_id).await?;
        }
        report.payments += 1;
    }
    let (credits, holding, unreleased) = {
        let mut conn = ctx.store.conn().await?;
        (
            store::unsettled_credits(&mut conn, limit).await?,
            store::stale_holding(&mut conn, limit).await?,
            store::unreleased(&mut conn, limit).await?,
        )
    };
    for id in credits {
        if settle_credit(ctx, id).await? {
            report.credits += 1;
        }
    }
    for withdrawal in holding {
        match hold(ctx, &withdrawal).await {
            Ok(_) | Err(HoldError::NotEnough) => report.holds += 1,
            Err(HoldError::Module(err)) => return Err(err),
        }
    }
    for withdrawal in unreleased {
        release(ctx, &withdrawal).await?;
        report.releases += 1;
    }
    Ok(report)
}

pub struct ReferralModule;

#[async_trait]
impl Module for ReferralModule {
    fn id(&self) -> ModuleId {
        ModuleId::Referral
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::number(
                "level1_percent",
                "Процент с оплат приглашённых",
                "Сколько процентов от каждой оплаты друга получает пригласивший",
            )
            .with_default("30"),
            SettingField::number(
                "level2_percent",
                "Второй уровень",
                "Процент с оплат тех, кого пригласили друзья; 0 — без второго уровня",
            )
            .with_default("10"),
            SettingField::number(
                "level3_percent",
                "Третий уровень",
                "0 — без третьего уровня",
            )
            .with_default("10"),
            SettingField::number(
                "withdraw_min",
                "Минимальная сумма вывода",
                "В валюте сервиса; пусто — без минимума",
            ),
            SettingField::flag("withdrawals", "Принимать заявки на вывод", "").with_default("true"),
        ]
    }

    fn validate_config(&self, config: &ModuleConfig) -> Result<(), ModuleError> {
        ReferralSettings::parse(config, Currency::Usd)
            .map(|_| ())
            .map_err(ModuleError::Invalid)
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    fn client_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::client(ctx))
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: SETTLE_TASK,
            every: Duration::from_secs(600),
        }]
    }

    fn bot_entries(&self) -> Vec<BotEntry> {
        vec![BotEntry {
            key: BOT_ENTRY,
            label: "Пригласить друга",
            order: 50,
        }]
    }

    async fn on_event(&self, ctx: &ModuleContext, event: &DomainEvent) -> Result<(), ModuleError> {
        match event {
            DomainEvent::ClientRegistered {
                client_id,
                start_param: Some(param),
                ..
            } => link_by_start_param(ctx, *client_id, param)
                .await
                .map(|_| ()),
            DomainEvent::PaymentPaid {
                payment_id,
                client_id,
                amount,
                provider,
                ..
            } => accrue(ctx, *payment_id, *client_id, *amount, provider)
                .await
                .map(|_| ()),
            DomainEvent::PaymentRefunded { payment_id, .. } => {
                schedule_reversal(ctx, *payment_id).await.map(|_| ())
            }
            _ => Ok(()),
        }
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        if task != SETTLE_TASK {
            return Err(ModuleError::Invalid(format!("у модуля нет задачи {task}")));
        }
        let report = settle(ctx, 200).await?;
        if report != SettleReport::default() {
            tracing::info!(?report, "рефералы: начисления и заявки досведены");
        }
        Ok(())
    }
}
