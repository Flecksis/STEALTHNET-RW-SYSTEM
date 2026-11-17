use crate::DomainError;
use chrono::{DateTime, Datelike, Duration, FixedOffset, Months, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

pub const GIB: u64 = 1024 * 1024 * 1024;
pub const TIB: u64 = 1024 * GIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficReset {
    #[default]
    NoReset,
    Day,
    Week,
    Month,
    MonthRolling,
    OnPayment,
}

impl TrafficReset {
    pub const ALL: [TrafficReset; 6] = [
        TrafficReset::Month,
        TrafficReset::MonthRolling,
        TrafficReset::Week,
        TrafficReset::Day,
        TrafficReset::OnPayment,
        TrafficReset::NoReset,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            TrafficReset::NoReset => "no_reset",
            TrafficReset::Day => "day",
            TrafficReset::Week => "week",
            TrafficReset::Month => "month",
            TrafficReset::MonthRolling => "month_rolling",
            TrafficReset::OnPayment => "on_payment",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        TrafficReset::ALL
            .into_iter()
            .find(|r| r.as_str() == value)
            .ok_or_else(|| DomainError::unknown("traffic_reset", value))
    }

    pub const fn strategy(self) -> TrafficStrategy {
        match self {
            TrafficReset::NoReset | TrafficReset::OnPayment => TrafficStrategy::NoReset,
            TrafficReset::Day => TrafficStrategy::Day,
            TrafficReset::Week => TrafficStrategy::Week,
            TrafficReset::Month => TrafficStrategy::Month,
            TrafficReset::MonthRolling => TrafficStrategy::MonthRolling,
        }
    }

    pub const fn resets_on_payment(self) -> bool {
        matches!(self, TrafficReset::OnPayment)
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            TrafficReset::Month => "Каждый месяц, 1-го числа",
            TrafficReset::MonthRolling => "Каждый месяц, в день оформления",
            TrafficReset::Week => "Каждую неделю, по понедельникам",
            TrafficReset::Day => "Каждый день",
            TrafficReset::OnPayment => "При каждой оплате",
            TrafficReset::NoReset => "Без сброса",
        }
    }

    pub const fn short_ru(self) -> &'static str {
        match self {
            TrafficReset::Month => "сброс 1-го числа",
            TrafficReset::MonthRolling => "сброс в день оформления",
            TrafficReset::Week => "сброс по понедельникам",
            TrafficReset::Day => "сброс каждый день",
            TrafficReset::OnPayment => "сброс при оплате",
            TrafficReset::NoReset => "без сброса",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TrafficStrategy {
    #[default]
    NoReset,
    Day,
    Week,
    Month,
    MonthRolling,
}

impl TrafficStrategy {
    pub const ALL: [TrafficStrategy; 5] = [
        TrafficStrategy::NoReset,
        TrafficStrategy::Day,
        TrafficStrategy::Week,
        TrafficStrategy::Month,
        TrafficStrategy::MonthRolling,
    ];

    pub const fn as_remna(self) -> &'static str {
        match self {
            TrafficStrategy::NoReset => "NO_RESET",
            TrafficStrategy::Day => "DAY",
            TrafficStrategy::Week => "WEEK",
            TrafficStrategy::Month => "MONTH",
            TrafficStrategy::MonthRolling => "MONTH_ROLLING",
        }
    }

    pub fn parse_remna(value: &str) -> Result<Self, DomainError> {
        TrafficStrategy::ALL
            .into_iter()
            .find(|s| s.as_remna() == value)
            .ok_or_else(|| DomainError::unknown("traffic_strategy", value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TrafficPolicy {
    pub limit_bytes: Option<u64>,
    pub reset: TrafficReset,
}

impl TrafficPolicy {
    pub const UNLIMITED: TrafficPolicy = TrafficPolicy {
        limit_bytes: None,
        reset: TrafficReset::NoReset,
    };

    pub const fn is_limited(&self) -> bool {
        matches!(self.limit_bytes, Some(n) if n > 0)
    }

    pub const fn remna_limit_bytes(&self) -> u64 {
        match self.limit_bytes {
            Some(n) => n,
            None => 0,
        }
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.limit_bytes == Some(0) {
            return Err(DomainError::invalid(
                "traffic_limit_bytes",
                "лимит 0 байт не имеет смысла: оставьте поле пустым для безлимита",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "at", rename_all = "snake_case")]
pub enum NextReset {
    At(DateTime<Utc>),
    OnPayment,
    Never,
}

fn local_instant(
    offset: FixedOffset,
    date: NaiveDate,
    hour: u32,
    minute: u32,
) -> Option<DateTime<Utc>> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    offset
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.with_timezone(&Utc))
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let next = first.checked_add_months(Months::new(1))?;
    Some((next - first).num_days() as u32)
}

pub fn next_reset(
    reset: TrafficReset,
    user_created_at: DateTime<Utc>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> NextReset {
    let today = now.with_timezone(&offset).date_naive();
    let found = match reset {
        TrafficReset::NoReset => return NextReset::Never,
        TrafficReset::OnPayment => return NextReset::OnPayment,
        TrafficReset::Day => local_instant(offset, today, 0, 5)
            .filter(|at| *at > now)
            .or_else(|| local_instant(offset, today + Duration::days(1), 0, 5)),
        TrafficReset::Week => {
            let shift = (7 - today.weekday().num_days_from_monday()) % 7;
            let monday = today + Duration::days(shift as i64);
            local_instant(offset, monday, 0, 15)
                .filter(|at| *at > now)
                .or_else(|| local_instant(offset, monday + Duration::days(7), 0, 15))
        }
        TrafficReset::Month => {
            let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1);
            first
                .and_then(|d| local_instant(offset, d, 0, 20))
                .filter(|at| *at > now)
                .or_else(|| {
                    first
                        .and_then(|d| d.checked_add_months(Months::new(1)))
                        .and_then(|d| local_instant(offset, d, 0, 20))
                })
        }
        TrafficReset::MonthRolling => {
            let created = user_created_at.with_timezone(&offset).date_naive();
            let earliest = created.checked_add_months(Months::new(1));
            let anchor = created.day();
            let mut cursor = NaiveDate::from_ymd_opt(today.year(), today.month(), 1);
            let mut result = None;
            for _ in 0..15 {
                let Some(month_start) = cursor else { break };
                let day = days_in_month(month_start.year(), month_start.month())
                    .map(|max| anchor.min(max));
                let candidate = day.and_then(|d| {
                    NaiveDate::from_ymd_opt(month_start.year(), month_start.month(), d)
                });
                if let Some(date) = candidate
                    && earliest.is_none_or(|e| date >= e)
                    && let Some(at) = local_instant(offset, date, 0, 0)
                    && at > now
                {
                    result = Some(at);
                    break;
                }
                cursor = month_start.checked_add_months(Months::new(1));
            }
            result
        }
    };
    found.map_or(NextReset::Never, NextReset::At)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn zero() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    #[test]
    fn месячный_сброс_первого_числа() {
        let now = utc(2026, 11, 16, 14, 52);
        assert_eq!(
            next_reset(TrafficReset::Month, now, now, zero()),
            NextReset::At(utc(2026, 12, 1, 0, 20))
        );
        assert_eq!(
            next_reset(TrafficReset::Month, now, utc(2026, 12, 1, 0, 10), zero()),
            NextReset::At(utc(2026, 12, 1, 0, 20))
        );
    }

    #[test]
    fn недельный_сброс_в_понедельник() {
        let monday_early = utc(2026, 11, 16, 0, 10);
        assert_eq!(
            next_reset(TrafficReset::Week, monday_early, monday_early, zero()),
            NextReset::At(utc(2026, 11, 16, 0, 15))
        );
        let monday_late = utc(2026, 11, 16, 14, 52);
        assert_eq!(
            next_reset(TrafficReset::Week, monday_late, monday_late, zero()),
            NextReset::At(utc(2026, 11, 23, 0, 15))
        );
    }

    #[test]
    fn дневной_сброс_завтра() {
        let now = utc(2026, 11, 16, 14, 52);
        assert_eq!(
            next_reset(TrafficReset::Day, now, now, zero()),
            NextReset::At(utc(2026, 11, 17, 0, 5))
        );
    }

    #[test]
    fn сброс_в_день_оформления_с_учётом_короткого_месяца() {
        let created = utc(2027, 1, 31, 10, 0);
        assert_eq!(
            next_reset(
                TrafficReset::MonthRolling,
                created,
                utc(2027, 2, 1, 0, 0),
                zero()
            ),
            NextReset::At(utc(2027, 2, 28, 0, 0))
        );
        let created = utc(2026, 11, 16, 10, 0);
        assert_eq!(
            next_reset(
                TrafficReset::MonthRolling,
                created,
                utc(2026, 11, 20, 0, 0),
                zero()
            ),
            NextReset::At(utc(2026, 12, 16, 0, 0))
        );
    }

    #[test]
    fn без_сброса_и_при_оплате() {
        let now = utc(2026, 11, 16, 14, 52);
        assert_eq!(
            next_reset(TrafficReset::NoReset, now, now, zero()),
            NextReset::Never
        );
        assert_eq!(
            next_reset(TrafficReset::OnPayment, now, now, zero()),
            NextReset::OnPayment
        );
    }

    #[test]
    fn сброс_при_оплате_это_no_reset_в_remnawave() {
        assert_eq!(TrafficReset::OnPayment.strategy(), TrafficStrategy::NoReset);
        assert_eq!(TrafficReset::Month.strategy().as_remna(), "MONTH");
        assert_eq!(
            TrafficReset::parse("month_rolling").unwrap(),
            TrafficReset::MonthRolling
        );
    }
}
