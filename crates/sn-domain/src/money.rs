use crate::DomainError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub enum Currency {
    #[default]
    #[serde(rename = "USD")]
    Usd,
    #[serde(rename = "RUB")]
    Rub,
}

impl Currency {
    pub const ALL: [Currency; 2] = [Currency::Usd, Currency::Rub];

    pub const fn code(self) -> &'static str {
        match self {
            Currency::Usd => "USD",
            Currency::Rub => "RUB",
        }
    }

    pub const fn rounding_step(self) -> i64 {
        match self {
            Currency::Usd => 1,
            Currency::Rub => 100,
        }
    }

    pub const fn minor_per_unit(self) -> i64 {
        100
    }

    pub fn parse(code: &str) -> Result<Self, DomainError> {
        match code.trim().to_ascii_uppercase().as_str() {
            "USD" => Ok(Currency::Usd),
            "RUB" => Ok(Currency::Rub),
            _ => Err(DomainError::UnknownCurrency(code.to_string())),
        }
    }
}

impl FromStr for Currency {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Currency::parse(s)
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PayCurrency {
    #[serde(rename = "USD")]
    Usd,
    #[serde(rename = "RUB")]
    Rub,
    #[serde(rename = "XTR")]
    Xtr,
}

impl PayCurrency {
    pub const fn code(self) -> &'static str {
        match self {
            PayCurrency::Usd => "USD",
            PayCurrency::Rub => "RUB",
            PayCurrency::Xtr => "XTR",
        }
    }

    pub const fn decimals(self) -> u32 {
        match self {
            PayCurrency::Usd | PayCurrency::Rub => 2,
            PayCurrency::Xtr => 0,
        }
    }

    pub const fn as_currency(self) -> Option<Currency> {
        match self {
            PayCurrency::Usd => Some(Currency::Usd),
            PayCurrency::Rub => Some(Currency::Rub),
            PayCurrency::Xtr => None,
        }
    }

    pub fn parse(code: &str) -> Result<Self, DomainError> {
        match code.trim().to_ascii_uppercase().as_str() {
            "USD" => Ok(PayCurrency::Usd),
            "RUB" => Ok(PayCurrency::Rub),
            "XTR" => Ok(PayCurrency::Xtr),
            _ => Err(DomainError::UnknownCurrency(code.to_string())),
        }
    }
}

impl From<Currency> for PayCurrency {
    fn from(value: Currency) -> Self {
        match value {
            Currency::Usd => PayCurrency::Usd,
            Currency::Rub => PayCurrency::Rub,
        }
    }
}

impl FromStr for PayCurrency {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PayCurrency::parse(s)
    }
}

impl fmt::Display for PayCurrency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

pub(crate) fn div_round_half_up(numerator: i128, denominator: i128) -> i128 {
    if numerator >= 0 {
        (2 * numerator + denominator) / (2 * denominator)
    } else {
        -((2 * -numerator + denominator) / (2 * denominator))
    }
}

fn to_i64(value: i128) -> Result<i64, DomainError> {
    i64::try_from(value).map_err(|_| DomainError::AmountOverflow)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    pub minor: i64,
    pub currency: Currency,
}

impl Money {
    pub const fn new(minor: i64, currency: Currency) -> Self {
        Money { minor, currency }
    }

    pub const fn zero(currency: Currency) -> Self {
        Money { minor: 0, currency }
    }

    pub const fn is_zero(self) -> bool {
        self.minor == 0
    }

    pub const fn is_negative(self) -> bool {
        self.minor < 0
    }

    fn same_currency(self, other: Money) -> Result<(), DomainError> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(DomainError::CurrencyMismatch {
                left: self.currency.code(),
                right: other.currency.code(),
            })
        }
    }

    pub fn checked_add(self, other: Money) -> Result<Money, DomainError> {
        self.same_currency(other)?;
        let minor = self
            .minor
            .checked_add(other.minor)
            .ok_or(DomainError::AmountOverflow)?;
        Ok(Money::new(minor, self.currency))
    }

    pub fn checked_sub(self, other: Money) -> Result<Money, DomainError> {
        self.same_currency(other)?;
        let minor = self
            .minor
            .checked_sub(other.minor)
            .ok_or(DomainError::AmountOverflow)?;
        Ok(Money::new(minor, self.currency))
    }

    pub fn checked_mul(self, factor: i64) -> Result<Money, DomainError> {
        let minor = self
            .minor
            .checked_mul(factor)
            .ok_or(DomainError::AmountOverflow)?;
        Ok(Money::new(minor, self.currency))
    }

    pub fn checked_neg(self) -> Result<Money, DomainError> {
        let minor = self
            .minor
            .checked_neg()
            .ok_or(DomainError::AmountOverflow)?;
        Ok(Money::new(minor, self.currency))
    }

    pub fn round_to_step(self) -> Money {
        let step = self.currency.rounding_step() as i128;
        let rounded = div_round_half_up(self.minor as i128, step) * step;
        Money::new(rounded as i64, self.currency)
    }

    pub fn percent(self, percent: u8) -> Result<Money, DomainError> {
        if percent > 100 {
            return Err(DomainError::InvalidPercent(percent));
        }
        if self.minor < 0 {
            return Err(DomainError::NegativeAmount);
        }
        let step = self.currency.rounding_step() as i128;
        let units = div_round_half_up(self.minor as i128 * percent as i128, 100 * step);
        Ok(Money::new(to_i64(units * step)?, self.currency))
    }

    pub fn divide_round(self, divisor: u32) -> Result<Money, DomainError> {
        if divisor == 0 {
            return Err(DomainError::invalid("divisor", "деление на ноль"));
        }
        let step = self.currency.rounding_step() as i128;
        let units = div_round_half_up(self.minor as i128, divisor as i128 * step);
        Ok(Money::new(to_i64(units * step)?, self.currency))
    }

    pub fn min(self, other: Money) -> Result<Money, DomainError> {
        self.same_currency(other)?;
        Ok(if self.minor <= other.minor {
            self
        } else {
            other
        })
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::format::money(*self))
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Stars(pub i64);

impl Stars {
    pub const ZERO: Stars = Stars(0);

    pub fn checked_add(self, other: Stars) -> Result<Stars, DomainError> {
        self.0
            .checked_add(other.0)
            .map(Stars)
            .ok_or(DomainError::AmountOverflow)
    }

    pub fn checked_sub(self, other: Stars) -> Result<Stars, DomainError> {
        self.0
            .checked_sub(other.0)
            .map(Stars)
            .ok_or(DomainError::AmountOverflow)
    }

    pub fn checked_mul(self, factor: i64) -> Result<Stars, DomainError> {
        self.0
            .checked_mul(factor)
            .map(Stars)
            .ok_or(DomainError::AmountOverflow)
    }

    pub fn percent(self, percent: u8) -> Result<Stars, DomainError> {
        if percent > 100 {
            return Err(DomainError::InvalidPercent(percent));
        }
        if self.0 < 0 {
            return Err(DomainError::NegativeAmount);
        }
        to_i64(div_round_half_up(self.0 as i128 * percent as i128, 100)).map(Stars)
    }

    pub fn divide_round(self, divisor: u32) -> Result<Stars, DomainError> {
        if divisor == 0 {
            return Err(DomainError::invalid("divisor", "деление на ноль"));
        }
        to_i64(div_round_half_up(self.0 as i128, divisor as i128)).map(Stars)
    }
}

impl fmt::Display for Stars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::format::stars(*self))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Amount {
    pub minor: i64,
    pub currency: PayCurrency,
}

impl Amount {
    pub const fn new(minor: i64, currency: PayCurrency) -> Self {
        Amount { minor, currency }
    }

    pub fn as_money(self) -> Option<Money> {
        self.currency
            .as_currency()
            .map(|currency| Money::new(self.minor, currency))
    }

    pub fn as_stars(self) -> Option<Stars> {
        match self.currency {
            PayCurrency::Xtr => Some(Stars(self.minor)),
            _ => None,
        }
    }

    pub fn units(self) -> String {
        minor_to_units(self.minor, self.currency)
    }
}

impl From<Money> for Amount {
    fn from(value: Money) -> Self {
        Amount::new(value.minor, value.currency.into())
    }
}

impl From<Stars> for Amount {
    fn from(value: Stars) -> Self {
        Amount::new(value.0, PayCurrency::Xtr)
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.as_money(), self.as_stars()) {
            (Some(money), _) => money.fmt(f),
            (None, Some(stars)) => stars.fmt(f),
            (None, None) => write!(f, "{} {}", self.units(), self.currency),
        }
    }
}

pub fn minor_to_units(minor: i64, currency: PayCurrency) -> String {
    match currency.decimals() {
        0 => minor.to_string(),
        _ => {
            let sign = if minor < 0 { "-" } else { "" };
            let abs = minor.unsigned_abs();
            format!("{sign}{}.{:02}", abs / 100, abs % 100)
        }
    }
}

pub fn parse_units(raw: &str, currency: PayCurrency) -> Option<i64> {
    let decimals = currency.decimals() as usize;
    let (int_part, frac_part) = match raw.split_once('.') {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (raw, None),
    };
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut value: i64 = int_part.parse().ok()?;
    value = value.checked_mul(10_i64.checked_pow(decimals as u32)?)?;
    if let Some(frac) = frac_part {
        if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let (kept, rest) = frac.split_at(frac.len().min(decimals));
        if !rest.bytes().all(|b| b == b'0') {
            return None;
        }
        let mut frac_value: i64 = if kept.is_empty() {
            0
        } else {
            kept.parse().ok()?
        };
        for _ in kept.len()..decimals {
            frac_value *= 10;
        }
        value = value.checked_add(frac_value)?;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd(minor: i64) -> Money {
        Money::new(minor, Currency::Usd)
    }

    fn rub(rubles: i64) -> Money {
        Money::new(rubles * 100, Currency::Rub)
    }

    #[test]
    fn скидка_в_долларах_считается_до_цента() {
        assert_eq!(usd(699).percent(10).unwrap(), usd(70));
        assert_eq!(usd(2399).percent(10).unwrap(), usd(240));
        assert_eq!(usd(299).percent(0).unwrap(), usd(0));
        assert_eq!(usd(299).percent(100).unwrap(), usd(299));
    }

    #[test]
    fn скидка_в_рублях_целыми_рублями() {
        assert_eq!(rub(499).percent(10).unwrap(), rub(50));
        assert_eq!(rub(1690).percent(10).unwrap(), rub(169));
        assert_eq!(rub(199).percent(15).unwrap(), rub(30));
    }

    #[test]
    fn цена_за_месяц_как_в_макетах() {
        assert_eq!(usd(699).divide_round(3).unwrap(), usd(233));
        assert_eq!(usd(2399).divide_round(12).unwrap(), usd(200));
        assert_eq!(usd(899).divide_round(3).unwrap(), usd(300));
        assert_eq!(usd(2999).divide_round(12).unwrap(), usd(250));
        assert_eq!(rub(499).divide_round(3).unwrap(), rub(166));
        assert_eq!(rub(1690).divide_round(12).unwrap(), rub(141));
        assert_eq!(Stars(350).divide_round(3).unwrap(), Stars(117));
        assert_eq!(Stars(1200).divide_round(12).unwrap(), Stars(100));
    }

    #[test]
    fn разные_валюты_не_складываются() {
        assert!(matches!(
            usd(100).checked_add(rub(1)),
            Err(DomainError::CurrencyMismatch { .. })
        ));
    }

    #[test]
    fn звёзды_округляются_до_целой() {
        assert_eq!(Stars(350).percent(10).unwrap(), Stars(35));
        assert_eq!(Stars(175).percent(10).unwrap(), Stars(18));
        assert!(Stars(10).percent(101).is_err());
    }

    #[test]
    fn перевод_в_единицы_и_обратно() {
        assert_eq!(minor_to_units(599, PayCurrency::Usd), "5.99");
        assert_eq!(minor_to_units(5, PayCurrency::Rub), "0.05");
        assert_eq!(minor_to_units(-1250, PayCurrency::Usd), "-12.50");
        assert_eq!(minor_to_units(150, PayCurrency::Xtr), "150");
        assert_eq!(parse_units("5.99", PayCurrency::Usd), Some(599));
        assert_eq!(parse_units("5.9", PayCurrency::Usd), Some(590));
        assert_eq!(parse_units("19.0", PayCurrency::Rub), Some(1900));
        assert_eq!(parse_units("19.000000", PayCurrency::Rub), Some(1900));
        assert_eq!(parse_units("150", PayCurrency::Xtr), Some(150));
        assert_eq!(parse_units("150.0", PayCurrency::Xtr), Some(150));
    }

    #[test]
    fn строгий_разбор_не_пропускает_мусор() {
        for raw in [
            "",
            ".",
            ".5",
            "5.",
            "-1",
            "+1",
            "1e3",
            " 5",
            "5 ",
            "5.999",
            "5,99",
            "0x10",
            "99999999999999999999",
        ] {
            assert_eq!(parse_units(raw, PayCurrency::Usd), None, "{raw}");
        }
        assert_eq!(parse_units("150.5", PayCurrency::Xtr), None);
    }
}
