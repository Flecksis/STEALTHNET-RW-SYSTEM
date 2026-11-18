use crate::catalog::promo::{self, PromoKind};
use crate::error::ModuleError;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::{ClientId, SubscriptionId};
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Currency, Money, PayCurrency, parse_units};
use sn_store::{Db, StoreError};

pub const DAYS_MAX: u32 = 365;
pub const VALID_DAYS_MAX: u32 = 365;
pub const AMOUNT_MAX_UNITS: i64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Prize {
    Days { days: u32 },
    Balance { amount: Money },
    Promo { percent: u8, valid_days: u32 },
    Empty,
}

impl Prize {
    pub fn label(self) -> String {
        let locale = sn_domain::lang::current();
        if locale == sn_domain::client::Locale::En {
            return match self {
                Prize::Days { days } => {
                    format!("+{} of subscription", format::days_in(days, locale))
                }
                Prize::Balance { amount } => {
                    format!("{} to your balance", format::money_in(amount, locale))
                }
                Prize::Promo { percent, .. } => format!("A {percent}% discount code"),
                Prize::Empty => "Nothing".into(),
            };
        }
        match self {
            Prize::Days { days } => format!("+{} к подписке", format::days(days)),
            Prize::Balance { amount } => format!("{} на баланс", format::money(amount)),
            Prize::Promo { percent, .. } => format!("Промокод на скидку {percent}%"),
            Prize::Empty => "Пусто".into(),
        }
    }

    pub const fn is_empty(self) -> bool {
        matches!(self, Prize::Empty)
    }

    pub const fn kind(self) -> &'static str {
        match self {
            Prize::Days { .. } => "days",
            Prize::Balance { .. } => "balance",
            Prize::Promo { .. } => "promo",
            Prize::Empty => "empty",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrizeDraft {
    Days {
        days: u32,
    },
    Balance {
        amount: String,
    },
    Promo {
        percent: u8,
        #[serde(default = "default_valid_days")]
        valid_days: u32,
    },
    Empty,
}

const fn default_valid_days() -> u32 {
    14
}

fn invalid(message: impl Into<String>) -> ModuleError {
    ModuleError::Invalid(message.into())
}

pub fn parse_amount(raw: &str, currency: Currency) -> Option<Money> {
    let minor = parse_units(
        raw.trim().replace(',', ".").as_str(),
        PayCurrency::from(currency),
    )?;
    let max = AMOUNT_MAX_UNITS * currency.minor_per_unit();
    (minor > 0 && minor <= max && minor % currency.rounding_step() == 0)
        .then_some(Money::new(minor, currency))
}

pub fn validate(
    draft: &PrizeDraft,
    currency: Currency,
    allow_empty: bool,
) -> Result<Prize, ModuleError> {
    match draft {
        PrizeDraft::Days { days } if (1..=DAYS_MAX).contains(days) => {
            Ok(Prize::Days { days: *days })
        }
        PrizeDraft::Days { .. } => Err(invalid(format!("дней — от 1 до {DAYS_MAX}"))),
        PrizeDraft::Balance { amount } => match parse_amount(amount, currency) {
            Some(amount) => Ok(Prize::Balance { amount }),
            None if currency == Currency::Rub => {
                Err(invalid("сумма на баланс — целое число рублей больше нуля"))
            }
            None => Err(invalid("сумма на баланс указана неверно")),
        },
        PrizeDraft::Promo {
            percent,
            valid_days,
        } => {
            if !(1..=100).contains(percent) {
                return Err(invalid("скидка — от 1 до 100 %"));
            }
            if !(1..=VALID_DAYS_MAX).contains(valid_days) {
                return Err(invalid(format!(
                    "промокод действует от 1 до {VALID_DAYS_MAX} дней"
                )));
            }
            Ok(Prize::Promo {
                percent: *percent,
                valid_days: *valid_days,
            })
        }
        PrizeDraft::Empty if allow_empty => Ok(Prize::Empty),
        PrizeDraft::Empty => Err(invalid("приз не может быть пустым")),
    }
}

pub fn check_currency(prize: Prize, currency: Currency) -> Result<(), ModuleError> {
    match prize {
        Prize::Balance { amount } if amount.currency != currency => Err(invalid(
            "сумма приза задана в другой валюте: сохраните приз заново",
        )),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveSubscription {
    pub id: SubscriptionId,
    pub expire_at: Option<DateTime<Utc>>,
}

pub async fn live_subscription(
    db: &mut impl Db,
    client_id: ClientId,
    prefer: Option<SubscriptionId>,
) -> Result<Option<LiveSubscription>, StoreError> {
    let row: Option<(i64, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT s.id, s.expire_at FROM subscriptions s \
         WHERE s.client_id = $1 AND s.ended_at IS NULL AND s.remna_user_id IS NOT NULL \
         AND s.remna_missing_at IS NULL AND (s.status IS NULL OR s.status <> 'disabled') \
         ORDER BY (s.id = $2) DESC, (s.kind = 'trial'), s.expire_at DESC NULLS LAST, s.id DESC LIMIT 1",
    )
    .bind(client_id.0)
    .bind(prefer.map_or(0, |s| s.0))
    .fetch_optional(db.conn())
    .await?;
    Ok(row.map(|(id, expire_at)| LiveSubscription {
        id: SubscriptionId(id),
        expire_at,
    }))
}

pub async fn credit(
    db: &mut impl Db,
    client_id: ClientId,
    amount: Money,
    kind: LedgerKind,
    key: String,
    comment: String,
) -> Result<Money, ModuleError> {
    let appended = db
        .ledger()
        .append(&NewLedgerEntry {
            client_id,
            wallet: Wallet::Main,
            amount,
            kind,
            order_id: None,
            payment_id: None,
            admin_id: None,
            comment: Some(comment),
            idempotency_key: Some(key),
        })
        .await?;
    if !appended.duplicate {
        db.events()
            .push(&DomainEvent::BalanceChanged {
                client_id,
                wallet: Wallet::Main,
                kind,
                delta: amount,
            })
            .await?;
    }
    Ok(appended.balance_after)
}

pub async fn personal_code(
    db: &mut impl Db,
    kind: PromoKind,
    valid_until: Option<DateTime<Utc>>,
    note: &str,
    owner: ClientId,
) -> Result<String, ModuleError> {
    Ok(promo::issue_personal(db, kind, valid_until, note, owner)
        .await?
        .code)
}

pub fn promo_until(now: DateTime<Utc>, valid_days: u32) -> DateTime<Utc> {
    now + Duration::days(i64::from(valid_days))
}

pub async fn promo_enabled(db: &mut impl Db) -> Result<bool, StoreError> {
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM module_states WHERE module = 'promo'")
            .fetch_optional(db.conn())
            .await?;
    Ok(enabled.unwrap_or(sn_domain::module::ModuleId::Promo.info().enabled_by_default))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn приз_проверяется_по_валюте_и_пределам() {
        assert_eq!(
            validate(&PrizeDraft::Days { days: 7 }, Currency::Rub, false).unwrap(),
            Prize::Days { days: 7 }
        );
        assert!(validate(&PrizeDraft::Days { days: 0 }, Currency::Rub, false).is_err());
        assert_eq!(
            validate(
                &PrizeDraft::Balance {
                    amount: "150".into()
                },
                Currency::Rub,
                false
            )
            .unwrap(),
            Prize::Balance {
                amount: Money::new(15_000, Currency::Rub)
            }
        );
        assert!(
            validate(
                &PrizeDraft::Balance {
                    amount: "1.50".into()
                },
                Currency::Rub,
                false
            )
            .is_err()
        );
        assert_eq!(
            validate(
                &PrizeDraft::Balance {
                    amount: "1,50".into()
                },
                Currency::Usd,
                false
            )
            .unwrap(),
            Prize::Balance {
                amount: Money::new(150, Currency::Usd)
            }
        );
        assert!(validate(&PrizeDraft::Empty, Currency::Usd, false).is_err());
        assert_eq!(
            validate(&PrizeDraft::Empty, Currency::Usd, true).unwrap(),
            Prize::Empty
        );
        assert!(
            validate(
                &PrizeDraft::Promo {
                    percent: 0,
                    valid_days: 7
                },
                Currency::Usd,
                false
            )
            .is_err()
        );
        assert_eq!(
            Prize::Balance {
                amount: Money::new(50_000, Currency::Rub)
            }
            .label(),
            "500\u{a0}₽ на баланс"
        );
        assert_eq!(Prize::Days { days: 7 }.label(), "+7\u{a0}дней к подписке");
    }
}
