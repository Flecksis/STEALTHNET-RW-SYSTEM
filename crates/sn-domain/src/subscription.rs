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
    let started = subscription.period_started_at?;
    let total = ceil_days(expire_at - started).max(1);
    let left = ceil_days(expire_at - now).min(total);
    Some(PeriodProgress {
        days_left: left as u32,
        days_total: total as u32,
    })
}

pub fn remna_username(subscription_public_id: Uuid) -> String {
    format!("sn_{}", subscription_public_id.simple())
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
    fn имя_в_remnawave_детерминированное_и_короткое() {
        let id = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let name = remna_username(id);
        assert_eq!(name, "sn_0123456789abcdef0123456789abcdef");
        assert!(name.len() <= 36);
    }
}
