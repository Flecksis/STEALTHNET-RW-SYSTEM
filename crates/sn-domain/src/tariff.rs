use crate::DomainError;
use crate::ids::{CategoryId, PeriodId, SquadId, TariffId};
use crate::money::{Currency, Money, Stars, div_round_half_up};
use crate::traffic::TrafficPolicy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CurrencyPrices {
    pub usd: Option<i64>,
    pub rub: Option<i64>,
}

impl CurrencyPrices {
    pub const fn get(&self, currency: Currency) -> Option<Money> {
        let minor = match currency {
            Currency::Usd => self.usd,
            Currency::Rub => self.rub,
        };
        match minor {
            Some(minor) => Some(Money::new(minor, currency)),
            None => None,
        }
    }

    pub fn set(&mut self, currency: Currency, minor: Option<i64>) {
        match currency {
            Currency::Usd => self.usd = minor,
            Currency::Rub => self.rub = minor,
        }
    }

    pub fn validate(&self, field: &'static str) -> Result<(), DomainError> {
        for value in [self.usd, self.rub].into_iter().flatten() {
            if value < 0 {
                return Err(DomainError::invalid(
                    field,
                    "цена не может быть отрицательной",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: CategoryId,
    pub name: String,
    pub sort: i32,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TariffPeriod {
    pub id: PeriodId,
    pub days: u32,
    pub prices: CurrencyPrices,
    pub stars: Option<Stars>,
    pub sort: i32,
    pub active: bool,
}

impl TariffPeriod {
    pub fn months(&self) -> u32 {
        months_for_days(self.days)
    }

    pub const fn price(&self, currency: Currency) -> Option<Money> {
        self.prices.get(currency)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtraDevices {
    pub max: u32,
    pub monthly: CurrencyPrices,
    pub monthly_stars: Option<Stars>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePolicy {
    pub included: u32,
    pub extra: Option<ExtraDevices>,
}

impl DevicePolicy {
    pub fn max_extra(&self) -> u32 {
        self.extra.as_ref().map_or(0, |e| e.max)
    }

    pub fn max_total(&self) -> u32 {
        self.included.saturating_add(self.max_extra())
    }

    pub fn check_extra(&self, wanted: u32, selling_enabled: bool) -> Result<(), DomainError> {
        if wanted == 0 {
            return Ok(());
        }
        if !selling_enabled || self.extra.is_none() {
            return Err(DomainError::ExtraDevicesNotSold);
        }
        let max = self.max_extra();
        if wanted > max {
            return Err(DomainError::TooManyExtraDevices { max });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tariff {
    pub id: TariffId,
    pub category_id: CategoryId,
    pub name: String,
    pub description: Option<String>,
    pub on_sale: bool,
    pub sort: i32,
    pub periods: Vec<TariffPeriod>,
    pub devices: DevicePolicy,
    pub traffic: TrafficPolicy,
    pub squads: Vec<SquadId>,
    pub external_squad: Option<SquadId>,
}

impl Tariff {
    pub fn period(&self, id: PeriodId) -> Option<&TariffPeriod> {
        self.periods.iter().find(|p| p.id == id)
    }

    pub fn sellable_period(
        &self,
        id: PeriodId,
        currency: Currency,
    ) -> Result<&TariffPeriod, DomainError> {
        if !self.on_sale {
            return Err(DomainError::TariffNotOnSale(self.id));
        }
        let period = self.period(id).ok_or(DomainError::PeriodNotFound(id))?;
        if !period.active {
            return Err(DomainError::PeriodInactive(id));
        }
        if period.price(currency).is_none() {
            return Err(DomainError::NoPriceInCurrency(currency.code()));
        }
        Ok(period)
    }

    pub fn offered_periods(&self, currency: Currency) -> Vec<&TariffPeriod> {
        let mut periods: Vec<&TariffPeriod> = self
            .periods
            .iter()
            .filter(|p| p.active && p.price(currency).is_some())
            .collect();
        periods.sort_by_key(|p| (p.sort, p.days));
        periods
    }

    pub fn is_offered(&self, currency: Currency) -> bool {
        self.on_sale && !self.offered_periods(currency).is_empty()
    }

    pub fn snapshot(&self, period: &TariffPeriod) -> TariffSnapshot {
        TariffSnapshot {
            tariff_id: self.id,
            category_id: self.category_id,
            name: self.name.clone(),
            period_id: period.id,
            days: period.days,
            devices_included: self.devices.included,
            max_extra_devices: self.devices.max_extra(),
            traffic: self.traffic,
            squads: self.squads.clone(),
            external_squad: self.external_squad,
        }
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.name.trim().is_empty() {
            return Err(DomainError::invalid("name", "название тарифа не задано"));
        }
        self.traffic.validate()?;
        for period in &self.periods {
            if period.days == 0 {
                return Err(DomainError::invalid("days", "срок должен быть больше нуля"));
            }
            period.prices.validate("price")?;
            if matches!(period.stars, Some(Stars(n)) if n <= 0) {
                return Err(DomainError::invalid(
                    "stars",
                    "цена в звёздах должна быть больше нуля",
                ));
            }
        }
        if let Some(extra) = &self.devices.extra {
            extra.monthly.validate("extra_device_price")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TariffSnapshot {
    pub tariff_id: TariffId,
    pub category_id: CategoryId,
    pub name: String,
    pub period_id: PeriodId,
    pub days: u32,
    pub devices_included: u32,
    pub max_extra_devices: u32,
    pub traffic: TrafficPolicy,
    pub squads: Vec<SquadId>,
    pub external_squad: Option<SquadId>,
}

pub fn months_for_days(days: u32) -> u32 {
    ((days as u64 * 12 + 182) / 365).max(1) as u32
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodOffer {
    pub period_id: PeriodId,
    pub days: u32,
    pub months: u32,
    pub price: Money,
    pub per_month: Money,
    pub savings_percent: Option<u8>,
    pub stars: Option<Stars>,
    pub stars_per_month: Option<Stars>,
    pub stars_savings_percent: Option<u8>,
}

fn savings(price: i64, monthly_base: i64, months: u32) -> Option<u8> {
    let full = monthly_base.checked_mul(months as i64)?;
    if months <= 1 || full <= 0 || price >= full {
        return None;
    }
    let percent = div_round_half_up(100 * (full - price) as i128, full as i128);
    u8::try_from(percent).ok().filter(|p| *p > 0)
}

pub fn period_offers(tariff: &Tariff, currency: Currency) -> Vec<PeriodOffer> {
    let periods = tariff.offered_periods(currency);
    let base = periods.iter().find(|p| p.months() == 1).copied();
    periods
        .into_iter()
        .filter_map(|p| {
            let price = p.price(currency)?;
            let months = p.months();
            let per_month = price.divide_round(months).ok()?;
            let savings_percent = base
                .and_then(|b| b.price(currency))
                .and_then(|b| savings(price.minor, b.minor, months));
            let stars_per_month = p.stars.and_then(|s| s.divide_round(months).ok());
            let stars_savings_percent = match (p.stars, base.and_then(|b| b.stars)) {
                (Some(s), Some(b)) => savings(s.0, b.0, months),
                _ => None,
            };
            Some(PeriodOffer {
                period_id: p.id,
                days: p.days,
                months,
                price,
                per_month,
                savings_percent,
                stars: p.stars,
                stars_per_month,
                stars_savings_percent,
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::traffic::{GIB, TrafficReset};
    use uuid::Uuid;

    pub fn period(id: i64, days: u32, usd: i64, rub: i64, stars: i64) -> TariffPeriod {
        TariffPeriod {
            id: PeriodId(id),
            days,
            prices: CurrencyPrices {
                usd: Some(usd),
                rub: Some(rub * 100),
            },
            stars: Some(Stars(stars)),
            sort: 0,
            active: true,
        }
    }

    pub fn extra() -> Option<ExtraDevices> {
        Some(ExtraDevices {
            max: 2,
            monthly: CurrencyPrices {
                usd: Some(99),
                rub: Some(4900),
            },
            monthly_stars: Some(Stars(50)),
        })
    }

    pub fn standard() -> Tariff {
        Tariff {
            id: TariffId(1),
            category_id: CategoryId(1),
            name: "Стандарт".into(),
            description: None,
            on_sale: true,
            sort: 0,
            periods: vec![
                period(11, 30, 299, 199, 150),
                period(12, 90, 699, 499, 350),
                period(13, 365, 2399, 1690, 1200),
            ],
            devices: DevicePolicy {
                included: 3,
                extra: extra(),
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![SquadId(Uuid::from_u128(1)), SquadId(Uuid::from_u128(2))],
            external_squad: None,
        }
    }

    pub fn unblock() -> Tariff {
        Tariff {
            id: TariffId(3),
            category_id: CategoryId(2),
            name: "Unblock".into(),
            description: None,
            on_sale: true,
            sort: 0,
            periods: vec![
                period(31, 30, 349, 249, 175),
                period(32, 90, 899, 649, 450),
                period(33, 365, 2999, 2190, 1500),
            ],
            devices: DevicePolicy {
                included: 3,
                extra: extra(),
            },
            traffic: TrafficPolicy {
                limit_bytes: Some(300 * GIB),
                reset: TrafficReset::Month,
            },
            squads: vec![SquadId(Uuid::from_u128(3)), SquadId(Uuid::from_u128(1))],
            external_squad: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn месяцы_по_сроку_в_днях() {
        assert_eq!(months_for_days(30), 1);
        assert_eq!(months_for_days(90), 3);
        assert_eq!(months_for_days(180), 6);
        assert_eq!(months_for_days(365), 12);
        assert_eq!(months_for_days(7), 1);
    }

    #[test]
    fn выгода_и_цена_за_месяц_как_в_currency_md() {
        let offers = period_offers(&standard(), Currency::Usd);
        assert_eq!(offers.len(), 3);
        assert_eq!(offers[1].per_month, Money::new(233, Currency::Usd));
        assert_eq!(offers[1].savings_percent, Some(22));
        assert_eq!(offers[2].savings_percent, Some(33));
        assert_eq!(offers[0].savings_percent, None);
        let rub = period_offers(&standard(), Currency::Rub);
        assert_eq!(rub[1].savings_percent, Some(16));
        assert_eq!(rub[2].savings_percent, Some(29));
        assert_eq!(rub[2].per_month, Money::new(14_100, Currency::Rub));
        let unblock = period_offers(&unblock(), Currency::Usd);
        assert_eq!(unblock[1].savings_percent, Some(14));
        assert_eq!(unblock[2].savings_percent, Some(28));
        assert_eq!(unblock[1].stars_savings_percent, Some(14));
        assert_eq!(unblock[2].stars_savings_percent, Some(29));
        assert_eq!(offers[1].stars_per_month, Some(Stars(117)));
    }

    #[test]
    fn срок_без_цены_в_валюте_не_продаётся() {
        let mut tariff = standard();
        tariff.periods[0].prices.rub = None;
        assert_eq!(
            tariff.sellable_period(PeriodId(11), Currency::Rub),
            Err(DomainError::NoPriceInCurrency("RUB"))
        );
        tariff.on_sale = false;
        assert_eq!(
            tariff.sellable_period(PeriodId(12), Currency::Usd),
            Err(DomainError::TariffNotOnSale(TariffId(1)))
        );
    }

    #[test]
    fn доп_устройства_в_пределах_лимита() {
        let devices = standard().devices;
        assert!(devices.check_extra(2, true).is_ok());
        assert_eq!(
            devices.check_extra(3, true),
            Err(DomainError::TooManyExtraDevices { max: 2 })
        );
        assert_eq!(
            devices.check_extra(1, false),
            Err(DomainError::ExtraDevicesNotSold)
        );
        assert_eq!(devices.max_total(), 5);
    }
}
