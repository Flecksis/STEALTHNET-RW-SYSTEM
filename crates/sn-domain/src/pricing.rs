use crate::DomainError;
use crate::format;
use crate::ids::{PeriodId, TariffId};
use crate::module::ModuleId;
use crate::money::{Amount, Currency, Money, PayCurrency, Stars, div_round_half_up};
use crate::tariff::Tariff;
use chrono::{DateTime, Utc};
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
    DeviceAddon {
        count: u32,
        days: u32,
    },
    TrafficAddon {
        gb: u32,
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

pub const MONTH_DAYS: u32 = 30;

pub fn days_left(expire_at: DateTime<Utc>, now: DateTime<Utc>) -> u32 {
    let seconds = (expire_at - now).num_seconds();
    if seconds <= 0 {
        return 0;
    }
    u32::try_from((seconds + 86_399) / 86_400).unwrap_or(u32::MAX)
}

pub fn prorated(monthly: Money, units: u32, days: u32) -> Result<Money, DomainError> {
    if monthly.is_negative() {
        return Err(DomainError::NegativeAmount);
    }
    if monthly.is_zero() || units == 0 || days == 0 {
        return Ok(Money::zero(monthly.currency));
    }
    let step = i128::from(monthly.currency.rounding_step());
    let numerator = i128::from(monthly.minor) * i128::from(units) * i128::from(days);
    let steps = div_round_half_up(numerator, i128::from(MONTH_DAYS) * step).max(1);
    let minor = i64::try_from(steps * step).map_err(|_| DomainError::AmountOverflow)?;
    Ok(Money::new(minor, monthly.currency))
}

pub fn prorated_stars(monthly: Stars, units: u32, days: u32) -> Result<Stars, DomainError> {
    if monthly.0 <= 0 || units == 0 || days == 0 {
        return Ok(Stars::ZERO);
    }
    let numerator = i128::from(monthly.0) * i128::from(units) * i128::from(days);
    let month = i128::from(MONTH_DAYS);
    let stars = ((numerator + month - 1) / month).max(1);
    Ok(Stars(
        i64::try_from(stars).map_err(|_| DomainError::AmountOverflow)?,
    ))
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

pub fn topup_presets(limits: TopUpLimits) -> Vec<Money> {
    let currency = limits.min.currency;
    let steps: [i64; 4] = match currency {
        Currency::Usd => [500, 1_000, 2_000, 5_000],
        Currency::Rub => [30_000, 50_000, 100_000, 200_000],
    };
    steps
        .into_iter()
        .filter(|minor| (limits.min.minor..=limits.max.minor).contains(minor))
        .map(|minor| Money::new(minor, currency))
        .collect()
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

    #[test]
    fn готовые_суммы_пополнения_только_в_пределах() {
        let usd = |min, max| TopUpLimits {
            min: Money::new(min, Currency::Usd),
            max: Money::new(max, Currency::Usd),
        };
        let minors = |limits| -> Vec<i64> {
            topup_presets(limits)
                .into_iter()
                .map(|money| money.minor)
                .collect()
        };
        assert_eq!(minors(usd(100, 100_000)), [500, 1_000, 2_000, 5_000]);
        assert_eq!(minors(usd(1_000, 2_000)), [1_000, 2_000]);
        assert!(minors(usd(6_000, 9_000)).is_empty());
        let rub = topup_presets(TopUpLimits {
            min: Money::new(10_000, Currency::Rub),
            max: Money::new(10_000_000, Currency::Rub),
        });
        assert_eq!(rub.len(), 4);
        assert!(rub.iter().all(|money| money.currency == Currency::Rub));
        assert_eq!(rub[0].minor, 30_000);
    }
}

#[cfg(test)]
mod proration_tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn устройство_за_полмесяца_стоит_половину() {
        let rub = |r: i64| Money::new(r * 100, Currency::Rub);
        assert_eq!(prorated(rub(50), 1, 15).unwrap(), rub(25));
        assert_eq!(prorated(rub(50), 2, 15).unwrap(), rub(50));
        assert_eq!(prorated(rub(50), 1, 30).unwrap(), rub(50));
        assert_eq!(prorated(rub(50), 1, 45).unwrap(), rub(75));
        assert_eq!(prorated(rub(50), 1, 14).unwrap(), rub(23));
        assert_eq!(prorated(rub(1), 1, 1).unwrap(), rub(1));
        let usd = |c: i64| Money::new(c, Currency::Usd);
        assert_eq!(prorated(usd(99), 1, 15).unwrap(), usd(50));
        assert_eq!(prorated(usd(99), 1, 0).unwrap(), usd(0));
        assert_eq!(prorated_stars(Stars(50), 1, 15).unwrap(), Stars(25));
        assert_eq!(prorated_stars(Stars(40), 1, 1).unwrap(), Stars(2));
        assert_eq!(prorated_stars(Stars(1), 1, 1).unwrap(), Stars(1));
    }

    #[test]
    fn оставшиеся_дни_округляются_вверх() {
        let now = Utc.with_ymd_and_hms(2026, 11, 17, 12, 0, 0).unwrap();
        assert_eq!(days_left(now + Duration::days(15), now), 15);
        assert_eq!(days_left(now + Duration::hours(5), now), 1);
        assert_eq!(
            days_left(now + Duration::days(14) + Duration::hours(1), now),
            15
        );
        assert_eq!(days_left(now - Duration::hours(1), now), 0);
    }
}
