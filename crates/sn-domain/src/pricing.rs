use crate::DomainError;
use crate::format;
use crate::ids::{PeriodId, TariffId};
use crate::module::ModuleId;
use crate::money::{Amount, Currency, Money, PayCurrency, Stars};
use crate::tariff::Tariff;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdjustmentKind {
    Percent { percent: u8 },
    Fixed { amount: Money },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdjustmentSource {
    PersonalDiscount,
    Promo { code: String, promo_id: Option<i64> },
    Module { module: ModuleId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceAdjustment {
    pub source: AdjustmentSource,
    pub kind: AdjustmentKind,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuoteLineKind {
    Period {
        tariff_id: TariffId,
        period_id: PeriodId,
        days: u32,
        months: u32,
    },
    ExtraDevices {
        count: u32,
        months: u32,
    },
    Adjustment {
        source: AdjustmentSource,
    },
    TopUp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteLine {
    pub kind: QuoteLineKind,
    pub label: String,
    pub amount: Money,
    pub stars: Option<Stars>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarsTotal {
    pub subtotal: Stars,
    pub discount: Stars,
    pub total: Stars,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub currency: Currency,
    pub lines: Vec<QuoteLine>,
    pub subtotal: Money,
    pub discount: Money,
    pub total: Money,
    pub stars: Option<StarsTotal>,
    pub days: u32,
    pub extra_devices: u32,
}

impl Quote {
    pub const fn is_free(&self) -> bool {
        self.total.minor == 0
    }

    pub fn amount_in(&self, currency: PayCurrency) -> Option<Amount> {
        match currency {
            PayCurrency::Xtr => self.stars.map(|s| s.total.into()),
            other if other == PayCurrency::from(self.currency) => Some(self.total.into()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct QuoteRequest<'a> {
    pub currency: Currency,
    pub tariff: &'a Tariff,
    pub period_id: PeriodId,
    pub extra_devices: u32,
    pub sell_extra_devices: bool,
    pub adjustments: &'a [PriceAdjustment],
}

fn add_stars(left: Option<Stars>, right: Option<Stars>) -> Result<Option<Stars>, DomainError> {
    match (left, right) {
        (Some(l), Some(r)) => l.checked_add(r).map(Some),
        _ => Ok(None),
    }
}

pub fn quote(req: &QuoteRequest<'_>) -> Result<Quote, DomainError> {
    let tariff = req.tariff;
    let period = tariff.sellable_period(req.period_id, req.currency)?;
    tariff
        .devices
        .check_extra(req.extra_devices, req.sell_extra_devices)?;
    let months = period.months();
    let base = period
        .price(req.currency)
        .ok_or(DomainError::NoPriceInCurrency(req.currency.code()))?;
    let mut lines = vec![QuoteLine {
        kind: QuoteLineKind::Period {
            tariff_id: tariff.id,
            period_id: period.id,
            days: period.days,
            months,
        },
        label: format!("{} · {}", tariff.name, format::days(period.days)),
        amount: base,
        stars: period.stars,
    }];
    let mut subtotal = base;
    let mut stars_subtotal = period.stars;

    if req.extra_devices > 0 {
        let extra = tariff
            .devices
            .extra
            .as_ref()
            .ok_or(DomainError::ExtraDevicesNotSold)?;
        let monthly = extra
            .monthly
            .get(req.currency)
            .ok_or(DomainError::NoPriceInCurrency(req.currency.code()))?;
        let units = req.extra_devices as i64 * months as i64;
        let amount = monthly.checked_mul(units)?;
        let stars = match extra.monthly_stars {
            Some(s) => Some(s.checked_mul(units)?),
            None => None,
        };
        subtotal = subtotal.checked_add(amount)?;
        stars_subtotal = add_stars(stars_subtotal, stars)?;
        lines.push(QuoteLine {
            kind: QuoteLineKind::ExtraDevices {
                count: req.extra_devices,
                months,
            },
            label: format!(
                "Доп. устройства: {} × {months}{}{}",
                req.extra_devices,
                format::NBSP,
                format::plural_ru(months as u64, "месяц", "месяца", "месяцев")
            ),
            amount,
            stars,
        });
    }

    let mut total = subtotal;
    let mut stars_total = stars_subtotal;
    for adjustment in req.adjustments {
        let (cut, stars_cut) = match &adjustment.kind {
            AdjustmentKind::Percent { percent } => {
                let cut = total.percent(*percent)?.min(total)?;
                let stars_cut = match stars_total {
                    Some(s) => Some(s.percent(*percent)?),
                    None => None,
                };
                (cut, stars_cut)
            }
            AdjustmentKind::Fixed { amount } => {
                if amount.is_negative() {
                    return Err(DomainError::NegativeAmount);
                }
                (amount.min(total)?, stars_total.map(|_| Stars::ZERO))
            }
        };
        total = total.checked_sub(cut)?;
        if let (Some(current), Some(cut)) = (stars_total, stars_cut) {
            stars_total = Some(current.checked_sub(cut)?);
        }
        lines.push(QuoteLine {
            kind: QuoteLineKind::Adjustment {
                source: adjustment.source.clone(),
            },
            label: adjustment.label.clone(),
            amount: cut.checked_neg()?,
            stars: stars_cut.map(|s| Stars(-s.0)),
        });
    }

    Ok(Quote {
        currency: req.currency,
        lines,
        subtotal,
        discount: subtotal.checked_sub(total)?,
        total,
        stars: match (stars_subtotal, stars_total) {
            (Some(subtotal), Some(total)) => Some(StarsTotal {
                subtotal,
                discount: subtotal.checked_sub(total)?,
                total,
            }),
            _ => None,
        },
        days: period.days,
        extra_devices: req.extra_devices,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpLimits {
    pub min: Money,
    pub max: Money,
}

pub fn topup_quote(amount: Money, limits: TopUpLimits) -> Result<Quote, DomainError> {
    if amount.currency != limits.min.currency {
        return Err(DomainError::CurrencyMismatch {
            left: amount.currency.code(),
            right: limits.min.currency.code(),
        });
    }
    if amount.minor < limits.min.minor || amount.minor > limits.max.minor {
        return Err(DomainError::TopUpOutOfRange {
            min: format::money(limits.min),
            max: format::money(limits.max),
        });
    }
    Ok(Quote {
        currency: amount.currency,
        lines: vec![QuoteLine {
            kind: QuoteLineKind::TopUp,
            label: "Пополнение баланса".into(),
            amount,
            stars: None,
        }],
        subtotal: amount,
        discount: Money::zero(amount.currency),
        total: amount,
        stars: None,
        days: 0,
        extra_devices: 0,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BalanceCheck {
    Enough,
    Short { balance: Money, missing: Money },
}

pub fn balance_check(balance: Money, total: Money) -> Result<BalanceCheck, DomainError> {
    if balance.minor >= total.minor {
        balance.checked_sub(total)?;
        Ok(BalanceCheck::Enough)
    } else {
        Ok(BalanceCheck::Short {
            balance,
            missing: total.checked_sub(balance)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tariff::fixtures::{standard, unblock};

    fn promo(percent: u8) -> PriceAdjustment {
        PriceAdjustment {
            source: AdjustmentSource::Promo {
                code: "ZIMA".into(),
                promo_id: Some(1),
            },
            kind: AdjustmentKind::Percent { percent },
            label: "Промокод ZIMA".into(),
        }
    }

    #[test]
    fn стандарт_на_три_месяца_с_двумя_устройствами() {
        let tariff = standard();
        let q = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff: &tariff,
            period_id: PeriodId(12),
            extra_devices: 2,
            sell_extra_devices: true,
            adjustments: &[],
        })
        .unwrap();
        assert_eq!(q.subtotal, Money::new(699 + 2 * 99 * 3, Currency::Usd));
        assert_eq!(q.total, q.subtotal);
        assert_eq!(q.stars.unwrap().total, Stars(350 + 2 * 50 * 3));
        assert_eq!(q.days, 90);
    }

    #[test]
    fn промокод_минус_десять_процентов_и_в_звёздах() {
        let tariff = standard();
        let adjustments = [promo(10)];
        let q = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff: &tariff,
            period_id: PeriodId(12),
            extra_devices: 0,
            sell_extra_devices: true,
            adjustments: &adjustments,
        })
        .unwrap();
        assert_eq!(q.total, Money::new(629, Currency::Usd));
        assert_eq!(q.discount, Money::new(70, Currency::Usd));
        assert_eq!(q.stars.unwrap().total, Stars(315));
        assert_eq!(q.stars.unwrap().discount, Stars(35));
    }

    #[test]
    fn рубли_со_скидкой_остаются_целыми() {
        let tariff = unblock();
        let adjustments = [promo(10)];
        let q = quote(&QuoteRequest {
            currency: Currency::Rub,
            tariff: &tariff,
            period_id: PeriodId(32),
            extra_devices: 0,
            sell_extra_devices: true,
            adjustments: &adjustments,
        })
        .unwrap();
        assert_eq!(q.total, Money::new(58_400, Currency::Rub));
        assert_eq!(q.total.minor % 100, 0);
    }

    #[test]
    fn фиксированная_скидка_не_уводит_в_минус_и_не_трогает_звёзды() {
        let tariff = standard();
        let adjustments = [PriceAdjustment {
            source: AdjustmentSource::PersonalDiscount,
            kind: AdjustmentKind::Fixed {
                amount: Money::new(10_000, Currency::Usd),
            },
            label: "Скидка".into(),
        }];
        let q = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff: &tariff,
            period_id: PeriodId(11),
            extra_devices: 0,
            sell_extra_devices: true,
            adjustments: &adjustments,
        })
        .unwrap();
        assert!(q.is_free());
        assert_eq!(q.stars.unwrap().total, Stars(150));
    }

    #[test]
    fn устройства_без_модуля_не_продаются() {
        let tariff = standard();
        let err = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff: &tariff,
            period_id: PeriodId(11),
            extra_devices: 1,
            sell_extra_devices: false,
            adjustments: &[],
        })
        .unwrap_err();
        assert_eq!(err, DomainError::ExtraDevicesNotSold);
    }

    #[test]
    fn баланс_только_на_полную_сумму() {
        let balance = Money::new(420, Currency::Usd);
        assert_eq!(
            balance_check(balance, Money::new(699, Currency::Usd)).unwrap(),
            BalanceCheck::Short {
                balance,
                missing: Money::new(279, Currency::Usd)
            }
        );
        assert_eq!(
            balance_check(balance, Money::new(420, Currency::Usd)).unwrap(),
            BalanceCheck::Enough
        );
    }

    #[test]
    fn пополнение_в_пределах() {
        let limits = TopUpLimits {
            min: Money::new(100, Currency::Usd),
            max: Money::new(100_000, Currency::Usd),
        };
        assert!(topup_quote(Money::new(1000, Currency::Usd), limits).is_ok());
        assert!(topup_quote(Money::new(50, Currency::Usd), limits).is_err());
        assert!(topup_quote(Money::new(1000, Currency::Rub), limits).is_err());
    }
}
