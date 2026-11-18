use chrono::{DateTime, FixedOffset, Utc};
use sn_domain::format;
use sn_domain::traffic::TrafficStrategy;
use sn_remnawave::UserStatus;
use std::collections::HashMap;
use uuid::Uuid;

pub(crate) fn date(at: DateTime<Utc>, offset: FixedOffset) -> String {
    at.with_timezone(&offset)
        .format("%d.%m.%Y %H:%M")
        .to_string()
}

pub(crate) fn traffic(bytes: i64) -> String {
    if bytes <= 0 {
        "без лимита".into()
    } else {
        format::traffic(u64::try_from(bytes).unwrap_or(0))
    }
}

pub(crate) const fn strategy(value: TrafficStrategy) -> &'static str {
    match value {
        TrafficStrategy::NoReset => "без сброса",
        TrafficStrategy::Day => "каждый день",
        TrafficStrategy::Week => "каждую неделю",
        TrafficStrategy::Month => "каждый месяц, 1-го числа",
        TrafficStrategy::MonthRolling => "каждый месяц, в день оформления",
    }
}

pub(crate) fn devices(limit: Option<i64>) -> String {
    match limit {
        None => "без ограничения".into(),
        Some(n) => count(
            u64::try_from(n).unwrap_or(0),
            "устройство",
            "устройства",
            "устройств",
        ),
    }
}

pub(crate) const fn status(value: UserStatus) -> &'static str {
    match value {
        UserStatus::Active => "включена",
        UserStatus::Disabled => "отключена",
        UserStatus::Limited => "трафик закончился",
        UserStatus::Expired => "истекла",
    }
}

pub(crate) fn squads(list: &[Uuid], names: &HashMap<Uuid, String>) -> String {
    if list.is_empty() {
        return "нет".into();
    }
    list.iter()
        .map(|uuid| {
            names
                .get(uuid)
                .cloned()
                .unwrap_or_else(|| short_id(&uuid.to_string()))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn short_id(value: &str) -> String {
    let tail: String = value
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.chars().count() < value.chars().count() {
        format!("…{tail}")
    } else {
        tail
    }
}

pub(crate) fn count(n: u64, one: &str, few: &str, many: &str) -> String {
    format!(
        "{}{}{}",
        format::group_digits(n),
        format::NBSP,
        format::plural_ru(n, one, few, many)
    )
}

pub(crate) fn who(
    subscription_id: i64,
    telegram_username: Option<&str>,
    telegram_id: Option<&str>,
    email: Option<&str>,
    client_id: i64,
) -> String {
    let client = telegram_username
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(|u| format!("@{}", u.trim_start_matches('@')))
        .or_else(|| telegram_id.map(|id| format!("Telegram {id}")))
        .or_else(|| email.map(str::to_string))
        .unwrap_or_else(|| format!("клиент №{client_id}"));
    format!("Подписка №{subscription_id} · {client}")
}
