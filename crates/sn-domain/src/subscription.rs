use crate::DomainError;
use crate::ids::{ClientId, PeriodId, RemnaUserId, SubscriptionId, TariffId};
use crate::traffic::TrafficStrategy;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const EXPIRING_SOON_DAYS: i64 = 3;
pub const TRIAL_TAG: &str = "TRIAL";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    Active,
    Limited,
    Expired,
    Disabled,
}

impl SubscriptionStatus {
    pub const ALL: [SubscriptionStatus; 4] = [
        SubscriptionStatus::Active,
        SubscriptionStatus::Limited,
        SubscriptionStatus::Expired,
        SubscriptionStatus::Disabled,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            SubscriptionStatus::Active => "active",
            SubscriptionStatus::Limited => "limited",
            SubscriptionStatus::Expired => "expired",
            SubscriptionStatus::Disabled => "disabled",
        }
    }

    pub const fn as_remna(self) -> &'static str {
        match self {
            SubscriptionStatus::Active => "ACTIVE",
            SubscriptionStatus::Limited => "LIMITED",
            SubscriptionStatus::Expired => "EXPIRED",
            SubscriptionStatus::Disabled => "DISABLED",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        SubscriptionStatus::ALL
            .into_iter()
            .find(|s| s.as_str() == value || s.as_remna() == value)
            .ok_or_else(|| DomainError::unknown("subscription_status", value))
    }

    pub const fn vpn_works(self) -> bool {
        matches!(self, SubscriptionStatus::Active)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionKind {
    Paid,
    Trial,
    Gift,
    Promo,
    Manual,
    Custom,
}

impl SubscriptionKind {
    pub const ALL: [SubscriptionKind; 6] = [
        SubscriptionKind::Paid,
        SubscriptionKind::Trial,
        SubscriptionKind::Gift,
        SubscriptionKind::Promo,
        SubscriptionKind::Manual,
        SubscriptionKind::Custom,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            SubscriptionKind::Paid => "paid",
            SubscriptionKind::Trial => "trial",
            SubscriptionKind::Gift => "gift",
            SubscriptionKind::Promo => "promo",
            SubscriptionKind::Manual => "manual",
            SubscriptionKind::Custom => "custom",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        SubscriptionKind::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| DomainError::unknown("subscription_kind", value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemnaLink {
    pub user_id: RemnaUserId,
    pub username: String,
    pub short_uuid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RemnaMirror {
    pub status: Option<SubscriptionStatus>,
    pub expire_at: Option<DateTime<Utc>>,
    pub traffic_used_bytes: Option<u64>,
    pub traffic_limit_bytes: Option<u64>,
    pub traffic_strategy: Option<TrafficStrategy>,
    pub hwid_limit: Option<u32>,
    pub devices_count: Option<u32>,
    pub online_at: Option<DateTime<Utc>>,
    pub first_connected_at: Option<DateTime<Utc>>,
    pub last_node: Option<Uuid>,
    pub remna_updated_at: Option<DateTime<Utc>>,
    pub synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AutoRenewState {
    pub enabled: bool,
    pub period_id: Option<PeriodId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtraTraffic {
    pub subscription_id: SubscriptionId,
    pub bytes: u64,
    pub window_started_at: Option<DateTime<Utc>>,
    pub window_lifetime: Option<u64>,
    pub window_ends_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: SubscriptionId,
    pub public_id: Uuid,
    pub client_id: ClientId,
    pub tariff_id: Option<TariffId>,
    pub kind: SubscriptionKind,
    pub remna: Option<RemnaLink>,
    pub extra_devices: u32,
    pub period_started_at: Option<DateTime<Utc>>,
    pub period_days: Option<u32>,
    pub autorenew: AutoRenewState,
    pub mirror: RemnaMirror,
    pub created_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub device_limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminStatus {
    Active,
    Expiring,
    Trial,
    Limited,
    Expired,
    Blocked,
    NoSubscription,
}

impl AdminStatus {
    pub const fn title_ru(self) -> &'static str {
        match self {
            AdminStatus::Active => "Активна",
            AdminStatus::Expiring => "Истекает",
            AdminStatus::Trial => "Пробный",
            AdminStatus::Limited => "Трафик исчерпан",
            AdminStatus::Expired => "Истекла",
            AdminStatus::Blocked => "Заблокирован",
            AdminStatus::NoSubscription => "Нет подписки",
        }
    }
}

pub fn admin_status(
    subscription: Option<&Subscription>,
    client_blocked: bool,
    now: DateTime<Utc>,
) -> AdminStatus {
    if client_blocked {
        return AdminStatus::Blocked;
    }
    let Some(sub) = subscription else {
        return AdminStatus::NoSubscription;
    };
    let status = match (sub.mirror.status, sub.mirror.expire_at) {
        (Some(status), _) => status,
        (None, Some(expire_at)) if expire_at <= now => SubscriptionStatus::Expired,
        (None, Some(_)) => SubscriptionStatus::Active,
        (None, None) => return AdminStatus::NoSubscription,
    };
    match status {
        SubscriptionStatus::Disabled => AdminStatus::Blocked,
        SubscriptionStatus::Expired => AdminStatus::Expired,
        SubscriptionStatus::Limited => AdminStatus::Limited,
        SubscriptionStatus::Active if sub.kind == SubscriptionKind::Trial => AdminStatus::Trial,
        SubscriptionStatus::Active => match sub.mirror.expire_at {
            Some(expire_at) if expire_at - now <= Duration::days(EXPIRING_SOON_DAYS) => {
                AdminStatus::Expiring
            }
            _ => AdminStatus::Active,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodProgress {
    pub days_left: u32,
    pub days_total: u32,
}

fn ceil_days(duration: Duration) -> i64 {
    let seconds = duration.num_seconds();
    if seconds <= 0 {
        0
    } else {
        (seconds + 86_399) / 86_400
    }
}

pub fn period_progress(subscription: &Subscription, now: DateTime<Utc>) -> Option<PeriodProgress> {
    let expire_at = subscription.mirror.expire_at?;
    let left = ceil_days(expire_at - now);
    let total = match subscription.period_started_at {
        Some(started) => ceil_days(expire_at - started),
        None => left.max(i64::from(subscription.period_days.unwrap_or_default())),
    }
    .max(1);
    Some(PeriodProgress {
        days_left: left.min(total) as u32,
        days_total: total as u32,
    })
}

pub fn remna_username(subscription_public_id: Uuid) -> String {
    format!("sn_{}", subscription_public_id.simple())
}

pub const REMNA_USERNAME_MIN: usize = 3;
pub const REMNA_USERNAME_MAX: usize = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameSource<'a> {
    pub telegram_username: Option<&'a str>,
    pub email: Option<&'a str>,
    pub telegram_id: Option<i64>,
    pub client_id: ClientId,
}

fn name_part(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.trim().trim_start_matches('@').chars() {
        let next = match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' => ch,
            _ if ch.is_ascii() => '_',
            _ => continue,
        };
        if next == '_' && (out.is_empty() || out.ends_with('_')) {
            continue;
        }
        out.push(next);
    }
    let head: String = out
        .trim_matches(['_', '-'])
        .chars()
        .take(REMNA_USERNAME_MAX)
        .collect();
    head.trim_end_matches(['_', '-']).to_string()
}

pub fn remna_name_base(source: &NameSource<'_>) -> String {
    let telegram = source.telegram_username.map(name_part);
    let email = source
        .email
        .and_then(|email| email.split('@').next())
        .map(name_part);
    if let Some(base) = [telegram, email]
        .into_iter()
        .flatten()
        .find(|base| base.len() >= REMNA_USERNAME_MIN)
    {
        return base;
    }
    match source.telegram_id {
        Some(id) => format!("tg{id}"),
        None => format!("id{}", source.client_id.0),
    }
}

fn with_suffix(base: &str, suffix: &str) -> String {
    let room = REMNA_USERNAME_MAX.saturating_sub(suffix.len());
    let head: String = base.chars().take(room).collect();
    format!("{}{suffix}", head.trim_end_matches(['_', '-']))
}

pub fn remna_name(base: &str, ordinal: u32) -> String {
    if ordinal <= 1 {
        with_suffix(base, "")
    } else {
        with_suffix(base, &format!("_{ordinal}"))
    }
}

pub fn remna_spare_name(base: &str, subscription_public_id: Uuid) -> String {
    let tail = subscription_public_id.simple().to_string();
    with_suffix(base, &format!("_{}", &tail[..10]))
}

pub fn remna_description(client_public_id: Uuid) -> String {
    format!("stealthnet client {client_public_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sub(
        status: Option<SubscriptionStatus>,
        expire_in_days: i64,
        kind: SubscriptionKind,
    ) -> Subscription {
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap();
        Subscription {
            id: SubscriptionId(1),
            public_id: Uuid::nil(),
            client_id: ClientId(1),
            tariff_id: Some(TariffId(1)),
            kind,
            remna: None,
            extra_devices: 0,
            period_started_at: Some(now),
            period_days: Some(90),
            autorenew: AutoRenewState::default(),
            mirror: RemnaMirror {
                status,
                expire_at: Some(now + Duration::days(expire_in_days)),
                ..RemnaMirror::default()
            },
            created_at: now,
            ended_at: None,
            device_limit: None,
        }
    }

    #[test]
    fn статус_для_админки() {
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap();
        let active = sub(Some(SubscriptionStatus::Active), 90, SubscriptionKind::Paid);
        assert_eq!(admin_status(Some(&active), false, now), AdminStatus::Active);
        assert_eq!(admin_status(Some(&active), true, now), AdminStatus::Blocked);
        let soon = sub(Some(SubscriptionStatus::Active), 2, SubscriptionKind::Paid);
        assert_eq!(admin_status(Some(&soon), false, now), AdminStatus::Expiring);
        let trial = sub(Some(SubscriptionStatus::Active), 2, SubscriptionKind::Trial);
        assert_eq!(admin_status(Some(&trial), false, now), AdminStatus::Trial);
        let limited = sub(
            Some(SubscriptionStatus::Limited),
            30,
            SubscriptionKind::Paid,
        );
        assert_eq!(
            admin_status(Some(&limited), false, now),
            AdminStatus::Limited
        );
        assert_eq!(admin_status(None, false, now), AdminStatus::NoSubscription);
    }

    #[test]
    fn прогресс_90_из_90_сразу_после_оплаты() {
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap();
        let s = sub(Some(SubscriptionStatus::Active), 90, SubscriptionKind::Paid);
        assert_eq!(
            period_progress(&s, now),
            Some(PeriodProgress {
                days_left: 90,
                days_total: 90
            })
        );
        assert_eq!(
            period_progress(&s, now + Duration::hours(1)),
            Some(PeriodProgress {
                days_left: 90,
                days_total: 90
            })
        );
    }

    #[test]
    fn у_перенесённой_подписки_без_начала_периода_видны_оставшиеся_дни() {
        let now = Utc.with_ymd_and_hms(2026, 11, 25, 18, 0, 0).unwrap();
        let mut s = sub(
            Some(SubscriptionStatus::Active),
            150,
            SubscriptionKind::Paid,
        );
        s.mirror.expire_at = Some(now + Duration::days(150));
        s.period_started_at = None;
        s.period_days = None;
        let progress = |left, total| {
            Some(PeriodProgress {
                days_left: left,
                days_total: total,
            })
        };
        assert_eq!(period_progress(&s, now), progress(150, 150));
        assert_eq!(
            period_progress(&s, now + Duration::days(140)),
            progress(10, 10)
        );
        s.period_days = Some(30);
        assert_eq!(
            period_progress(&s, now + Duration::days(140)),
            progress(10, 30)
        );
        assert_eq!(period_progress(&s, now), progress(150, 150));
        assert_eq!(
            period_progress(&s, now + Duration::days(151)),
            progress(0, 30)
        );
        s.period_days = None;
        assert_eq!(
            period_progress(&s, now + Duration::days(151)),
            progress(0, 1)
        );
        s.mirror.expire_at = None;
        assert_eq!(period_progress(&s, now), None);
    }

    #[test]
    fn имя_в_remnawave_детерминированное_и_короткое() {
        let id = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let name = remna_username(id);
        assert_eq!(name, "sn_0123456789abcdef0123456789abcdef");
        assert!(name.len() <= 36);
    }

    fn source<'a>(
        telegram_username: Option<&'a str>,
        email: Option<&'a str>,
        telegram_id: Option<i64>,
    ) -> NameSource<'a> {
        NameSource {
            telegram_username,
            email,
            telegram_id,
            client_id: ClientId(42),
        }
    }

    fn valid(name: &str) -> bool {
        (REMNA_USERNAME_MIN..=REMNA_USERNAME_MAX).contains(&name.len())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    #[test]
    fn основа_имени_из_ника_telegram() {
        let base = remna_name_base(&source(
            Some("Ivan_Petrov"),
            Some("other@mail.ru"),
            Some(5_093_118_422),
        ));
        assert_eq!(base, "Ivan_Petrov");
        assert_eq!(
            remna_name_base(&source(Some("@durov"), None, Some(1))),
            "durov"
        );
    }

    #[test]
    fn без_ника_основа_из_почты() {
        let base = remna_name_base(&source(
            None,
            Some("ivan.petrov+vpn@mail.ru"),
            Some(5_093_118_422),
        ));
        assert_eq!(base, "ivan_petrov_vpn");
        assert_eq!(
            remna_name_base(&source(Some("  "), Some("anna-k@example.com"), None)),
            "anna-k"
        );
    }

    #[test]
    fn без_ника_и_почты_понятная_замена() {
        assert_eq!(
            remna_name_base(&source(None, None, Some(5_093_118_422))),
            "tg5093118422"
        );
        assert_eq!(remna_name_base(&source(None, None, None)), "id42");
        assert_eq!(
            remna_name_base(&source(None, Some("иван@почта.рф"), Some(7))),
            "tg7"
        );
        assert_eq!(
            remna_name_base(&source(None, Some("ab@x.io"), None)),
            "id42"
        );
    }

    #[test]
    fn вторая_и_третья_подписки_с_номером() {
        let base = remna_name_base(&source(Some("ivan"), None, None));
        assert_eq!(remna_name(&base, 1), "ivan");
        assert_eq!(remna_name(&base, 2), "ivan_2");
        assert_eq!(remna_name(&base, 3), "ivan_3");
        assert_eq!(remna_name(&base, 12), "ivan_12");
    }

    #[test]
    fn длинное_имя_укорачивается_до_36_с_номером() {
        let long = "a".repeat(30) + "_bcdefghijk";
        let base = remna_name_base(&source(None, Some(&format!("{long}@mail.ru")), None));
        assert_eq!(base.len(), REMNA_USERNAME_MAX);
        for ordinal in [1, 2, 3, 10, 125] {
            let name = remna_name(&base, ordinal);
            assert!(valid(&name), "{name}");
        }
        assert_eq!(remna_name(&base, 2), format!("{}_bcd_2", "a".repeat(30)));
        let spare = remna_spare_name(&base, Uuid::from_u128(0xabcdef));
        assert!(valid(&spare), "{spare}");
        assert!(spare.ends_with("_0000000000"), "{spare}");
    }

    #[test]
    fn в_имени_только_допустимые_символы() {
        for raw in [
            "Иван Петров",
            "__x__y__",
            "a..b",
            "-dash-",
            "o'brien",
            "very.long.name.with.dots.and+plus@x",
        ] {
            let base = remna_name_base(&source(None, Some(&format!("{raw}@mail.ru")), Some(9)));
            assert!(valid(&base), "{raw} → {base}");
            assert!(valid(&remna_name(&base, 2)), "{raw} → {base}");
        }
        assert_eq!(
            remna_name_base(&source(None, Some("o'brien@mail.ru"), None)),
            "o_brien"
        );
    }
}
