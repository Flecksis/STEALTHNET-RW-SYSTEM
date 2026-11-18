use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

pub const HOURS: [u32; 7] = [1, 2, 3, 4, 6, 8, 12];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "every", rename_all = "snake_case")]
pub enum Schedule {
    Off,
    Hours { hours: u32 },
    Day { at: String },
    Week { weekday: u32, at: String },
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule::Day { at: "04:00".into() }
    }
}

fn time(at: &str) -> Option<NaiveTime> {
    let (h, m) = at.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
}

impl Schedule {
    pub fn clean(self) -> Result<Schedule, String> {
        match self {
            Schedule::Off => Ok(Schedule::Off),
            Schedule::Hours { hours } if HOURS.contains(&hours) => Ok(Schedule::Hours { hours }),
            Schedule::Hours { .. } => {
                Err("копию можно делать каждые 1, 2, 3, 4, 6, 8 или 12 часов".into())
            }
            Schedule::Day { at } => {
                let t = time(&at).ok_or("время копии: часы и минуты, например 04:00")?;
                Ok(Schedule::Day {
                    at: t.format("%H:%M").to_string(),
                })
            }
            Schedule::Week { weekday, at } => {
                if !(1..=7).contains(&weekday) {
                    return Err("день недели: от понедельника до воскресенья".into());
                }
                let t = time(&at).ok_or("время копии: часы и минуты, например 04:00")?;
                Ok(Schedule::Week {
                    weekday,
                    at: t.format("%H:%M").to_string(),
                })
            }
        }
    }

    fn slots_near(&self, now: DateTime<Utc>, offset: FixedOffset) -> Vec<DateTime<Utc>> {
        let local = now.with_timezone(&offset);
        let today = local.date_naive();
        let mut out = Vec::new();
        for shift in -8i64..=8 {
            let day = today + Duration::days(shift);
            match self {
                Schedule::Off => {}
                Schedule::Hours { hours } => {
                    let step = (*hours).clamp(1, 24);
                    for hour in (0..24).step_by(step as usize) {
                        if let Some(t) = NaiveTime::from_hms_opt(hour, 0, 0)
                            && let Some(at) = offset.from_local_datetime(&day.and_time(t)).single()
                        {
                            out.push(at.with_timezone(&Utc));
                        }
                    }
                }
                Schedule::Day { at } => {
                    if let Some(t) = time(at)
                        && let Some(at) = offset.from_local_datetime(&day.and_time(t)).single()
                    {
                        out.push(at.with_timezone(&Utc));
                    }
                }
                Schedule::Week { weekday, at } => {
                    if day.weekday().number_from_monday() == *weekday
                        && let Some(t) = time(at)
                        && let Some(at) = offset.from_local_datetime(&day.and_time(t)).single()
                    {
                        out.push(at.with_timezone(&Utc));
                    }
                }
            }
        }
        out.sort();
        out
    }

    pub fn previous(&self, now: DateTime<Utc>, offset: FixedOffset) -> Option<DateTime<Utc>> {
        self.slots_near(now, offset)
            .into_iter()
            .filter(|slot| *slot <= now)
            .max()
    }

    pub fn next(&self, now: DateTime<Utc>, offset: FixedOffset) -> Option<DateTime<Utc>> {
        self.slots_near(now, offset)
            .into_iter()
            .find(|slot| *slot > now)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    Now(DateTime<Utc>),
    At(DateTime<Utc>),
    Never,
}

pub fn due(
    schedule: &Schedule,
    since: Option<DateTime<Utc>>,
    last_slot: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Due {
    if *schedule == Schedule::Off {
        return Due::Never;
    }
    if let Some(prev) = schedule.previous(now, offset)
        && since.is_none_or(|since| prev >= since)
        && last_slot.is_none_or(|last| last < prev)
    {
        return Due::Now(prev);
    }
    match schedule.next(now, offset) {
        Some(at) => Due::At(at),
        None => Due::Never,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Offset;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn msk() -> FixedOffset {
        FixedOffset::east_opt(3 * 3600).unwrap()
    }

    #[test]
    fn ежедневная_копия_по_времени_панели() {
        let schedule = Schedule::Day { at: "04:00".into() };
        let now = utc(2026, 11, 18, 12, 0);
        assert_eq!(schedule.previous(now, msk()), Some(utc(2026, 11, 18, 1, 0)));
        assert_eq!(schedule.next(now, msk()), Some(utc(2026, 11, 19, 1, 0)));
    }

    #[test]
    fn копии_каждые_n_часов_от_полуночи() {
        let schedule = Schedule::Hours { hours: 6 };
        let now = utc(2026, 11, 18, 10, 30);
        assert_eq!(schedule.previous(now, msk()), Some(utc(2026, 11, 18, 9, 0)));
        assert_eq!(schedule.next(now, msk()), Some(utc(2026, 11, 18, 15, 0)));
    }

    #[test]
    fn еженедельная_копия_в_свой_день() {
        let schedule = Schedule::Week {
            weekday: 7,
            at: "23:30".into(),
        };
        let now = utc(2026, 11, 18, 12, 0);
        assert_eq!(
            schedule.next(now, Utc.fix()),
            Some(utc(2026, 11, 22, 23, 30))
        );
        assert_eq!(
            schedule.previous(now, Utc.fix()),
            Some(utc(2026, 11, 15, 23, 30))
        );
    }

    #[test]
    fn пропущенная_копия_делается_один_раз_после_запуска() {
        let schedule = Schedule::Day { at: "04:00".into() };
        let now = utc(2026, 11, 18, 12, 0);
        let slot = utc(2026, 11, 18, 1, 0);
        assert_eq!(due(&schedule, None, None, now, msk()), Due::Now(slot));
        assert_eq!(
            due(&schedule, None, Some(slot), now, msk()),
            Due::At(utc(2026, 11, 19, 1, 0))
        );
        assert_eq!(
            due(&schedule, Some(utc(2026, 11, 18, 11, 0)), None, now, msk()),
            Due::At(utc(2026, 11, 19, 1, 0))
        );
        assert_eq!(due(&Schedule::Off, None, None, now, msk()), Due::Never);
    }

    #[test]
    fn неверное_расписание_не_сохраняется() {
        assert!(Schedule::Hours { hours: 5 }.clean().is_err());
        assert!(Schedule::Day { at: "25:00".into() }.clean().is_err());
        assert!(
            Schedule::Week {
                weekday: 0,
                at: "04:00".into()
            }
            .clean()
            .is_err()
        );
        assert_eq!(
            Schedule::Day { at: "4:05".into() }.clean().unwrap(),
            Schedule::Day { at: "04:05".into() }
        );
        assert_eq!(
            serde_json::to_value(Schedule::Hours { hours: 6 }).unwrap(),
            serde_json::json!({"every": "hours", "hours": 6})
        );
    }
}
