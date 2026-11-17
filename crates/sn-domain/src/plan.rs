use crate::DomainError;
use crate::ids::{CategoryId, RemnaUserId, SquadId};
use crate::order::OrderKind;
use crate::subscription::{SubscriptionKind, SubscriptionStatus};
use crate::tariff::TariffSnapshot;
use crate::traffic::{TrafficReset, TrafficStrategy};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    New,
    Renewal,
    Change,
}

impl PlanKind {
    pub const fn order_kind(self) -> OrderKind {
        match self {
            PlanKind::New => OrderKind::New,
            PlanKind::Renewal => OrderKind::Renewal,
            PlanKind::Change => OrderKind::Change,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentTariff {
    pub kind: SubscriptionKind,
    pub category_id: Option<CategoryId>,
}

pub fn classify(target_category: CategoryId, current: Option<CurrentTariff>) -> PlanKind {
    match current {
        None => PlanKind::New,
        Some(c) if c.kind == SubscriptionKind::Trial => PlanKind::Change,
        Some(c) if c.category_id == Some(target_category) => PlanKind::Renewal,
        Some(_) => PlanKind::Change,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemnaUserState {
    pub user_id: RemnaUserId,
    pub status: SubscriptionStatus,
    pub expire_at: DateTime<Utc>,
    pub squads: Vec<SquadId>,
    pub traffic_limit_bytes: u64,
    pub traffic_used_bytes: u64,
    pub traffic_strategy: TrafficStrategy,
    pub hwid_device_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewUserProfile {
    pub username: String,
    pub telegram_id: Option<i64>,
    pub email: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldDevices {
    pub limit: u32,
    pub until: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
pub struct PlanInput<'a> {
    pub kind: PlanKind,
    pub snapshot: &'a TariffSnapshot,
    pub extra_devices: u32,
    pub held_devices: Option<HeldDevices>,
    pub paid_at: DateTime<Utc>,
    pub current: Option<&'a RemnaUserState>,
    pub keep_squads: &'a [SquadId],
    pub profile: &'a NewUserProfile,
    pub tag: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemnaTarget {
    pub expire_at: DateTime<Utc>,
    pub squads: Vec<SquadId>,
    pub external_squad: Option<SquadId>,
    pub traffic_limit_bytes: u64,
    pub traffic_strategy: TrafficStrategy,
    pub hwid_device_limit: Option<u32>,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanAction {
    Create {
        username: String,
        telegram_id: Option<i64>,
        email: Option<String>,
        description: String,
    },
    Update {
        user_id: RemnaUserId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FulfillmentPlan {
    pub kind: PlanKind,
    pub action: PlanAction,
    pub target: RemnaTarget,
    pub reset_traffic: bool,
    pub activate: bool,
    pub period_started_at: DateTime<Utc>,
    pub period_days: u32,
    #[serde(default)]
    pub base_expire_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub device_hold: Option<DeviceHold>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHold {
    pub held_limit: u32,
    pub until: DateTime<Utc>,
    pub limit_after: u32,
}

pub fn renewal_base(current_expire_at: DateTime<Utc>, paid_at: DateTime<Utc>) -> DateTime<Utc> {
    current_expire_at.max(paid_at)
}

pub fn whole_seconds(at: DateTime<Utc>) -> DateTime<Utc> {
    if at.timestamp_subsec_nanos() == 0 {
        return at;
    }
    DateTime::from_timestamp(at.timestamp() + 1, 0).unwrap_or(at)
}

pub fn new_expire_at(
    kind: PlanKind,
    current_expire_at: Option<DateTime<Utc>>,
    paid_at: DateTime<Utc>,
    days: u32,
) -> DateTime<Utc> {
    let base = match (kind, current_expire_at) {
        (PlanKind::Renewal, Some(current)) => renewal_base(current, paid_at),
        _ => paid_at,
    };
    whole_seconds(base + Duration::days(days as i64))
}

fn merge_squads(primary: &[SquadId], extra: &[SquadId]) -> Vec<SquadId> {
    let mut out: Vec<SquadId> = Vec::with_capacity(primary.len() + extra.len());
    for squad in primary.iter().chain(extra) {
        if !out.contains(squad) {
            out.push(*squad);
        }
    }
    out
}

pub fn plan(input: &PlanInput<'_>) -> Result<FulfillmentPlan, DomainError> {
    let snapshot = input.snapshot;
    if input.extra_devices > snapshot.max_extra_devices {
        return Err(DomainError::TooManyExtraDevices {
            max: snapshot.max_extra_devices,
        });
    }
    if snapshot.squads.is_empty() {
        return Err(DomainError::invalid(
            "squads",
            "у тарифа нет сквадов: подписка выдалась бы без серверов",
        ));
    }
    let traffic = snapshot.traffic;
    let tariff_limit = traffic.remna_limit_bytes();
    let strategy = traffic.reset.strategy();
    let paid_devices = snapshot
        .devices_included
        .saturating_add(input.extra_devices);
    let hwid_device_limit = (paid_devices > 0).then_some(paid_devices);
    let tag = input.tag.map(str::to_string);

    let Some(current) = input.current else {
        let expire_at = new_expire_at(input.kind, None, input.paid_at, snapshot.days);
        return Ok(FulfillmentPlan {
            kind: input.kind,
            action: PlanAction::Create {
                username: input.profile.username.clone(),
                telegram_id: input.profile.telegram_id,
                email: input.profile.email.clone(),
                description: input.profile.description.clone(),
            },
            target: RemnaTarget {
                expire_at,
                squads: snapshot.squads.clone(),
                external_squad: snapshot.external_squad,
                traffic_limit_bytes: tariff_limit,
                traffic_strategy: strategy,
                hwid_device_limit,
                tag,
            },
            reset_traffic: false,
            activate: false,
            period_started_at: input.paid_at,
            period_days: snapshot.days,
            base_expire_at: None,
            device_hold: None,
        });
    };

    let renewal = input.kind == PlanKind::Renewal;
    let expire_at = new_expire_at(
        input.kind,
        Some(current.expire_at),
        input.paid_at,
        snapshot.days,
    );
    let squads = if renewal {
        merge_squads(&snapshot.squads, input.keep_squads)
    } else {
        snapshot.squads.clone()
    };
    let traffic_mode_changed =
        current.traffic_limit_bytes == 0 || current.traffic_strategy != strategy;
    let (traffic_limit_bytes, reset_traffic) = if !renewal {
        (tariff_limit, true)
    } else if tariff_limit == 0 {
        (0, false)
    } else if traffic_mode_changed {
        (tariff_limit, true)
    } else {
        match traffic.reset {
            TrafficReset::OnPayment => (tariff_limit, true),
            TrafficReset::NoReset => (
                current
                    .traffic_limit_bytes
                    .checked_add(tariff_limit)
                    .ok_or(DomainError::AmountOverflow)?,
                false,
            ),
            _ => (tariff_limit, false),
        }
    };
    let device_hold = input
        .held_devices
        .filter(|held| {
            renewal && paid_devices > 0 && held.until > input.paid_at && held.limit > paid_devices
        })
        .map(|held| DeviceHold {
            held_limit: held.limit,
            until: held.until,
            limit_after: paid_devices,
        });
    let hwid_device_limit = match device_hold {
        Some(hold) => Some(hold.held_limit),
        None => hwid_device_limit,
    };
    let traffic_left = traffic_limit_bytes == 0
        || reset_traffic
        || traffic_limit_bytes > current.traffic_used_bytes;
    let activate = match current.status {
        SubscriptionStatus::Expired => true,
        SubscriptionStatus::Limited => traffic_left,
        SubscriptionStatus::Active | SubscriptionStatus::Disabled => false,
    };
    let period_days = ((expire_at - input.paid_at).num_seconds().max(0) + 86_399) / 86_400;

    Ok(FulfillmentPlan {
        kind: input.kind,
        action: PlanAction::Update {
            user_id: current.user_id,
        },
        target: RemnaTarget {
            expire_at,
            squads,
            external_squad: snapshot.external_squad,
            traffic_limit_bytes,
            traffic_strategy: strategy,
            hwid_device_limit,
            tag,
        },
        reset_traffic,
        activate,
        period_started_at: input.paid_at,
        period_days: period_days as u32,
        base_expire_at: Some(current.expire_at),
        device_hold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PeriodId;
    use crate::tariff::fixtures::{standard, unblock};
    use crate::traffic::GIB;
    use chrono::TimeZone;
    use uuid::Uuid;

    fn profile() -> NewUserProfile {
        NewUserProfile {
            username: "sn_test".into(),
            telegram_id: Some(5_093_118_422),
            email: None,
            description: "stealthnet client".into(),
        }
    }

    fn paid_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap()
    }

    fn current(status: SubscriptionStatus, expire_at: DateTime<Utc>) -> RemnaUserState {
        RemnaUserState {
            user_id: RemnaUserId(77),
            status,
            expire_at,
            squads: vec![SquadId(Uuid::from_u128(1)), SquadId(Uuid::from_u128(9))],
            traffic_limit_bytes: 300 * GIB,
            traffic_used_bytes: 300 * GIB,
            traffic_strategy: TrafficStrategy::Month,
            hwid_device_limit: Some(3),
        }
    }

    #[test]
    fn категория_решает_продление_или_смену() {
        let same = CurrentTariff {
            kind: SubscriptionKind::Paid,
            category_id: Some(CategoryId(1)),
        };
        assert_eq!(classify(CategoryId(1), Some(same)), PlanKind::Renewal);
        assert_eq!(classify(CategoryId(2), Some(same)), PlanKind::Change);
        assert_eq!(classify(CategoryId(1), None), PlanKind::New);
        let trial = CurrentTariff {
            kind: SubscriptionKind::Trial,
            category_id: None,
        };
        assert_eq!(classify(CategoryId(1), Some(trial)), PlanKind::Change);
    }

    #[test]
    fn продление_прибавляет_дни_к_дате_окончания() {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(11)).unwrap());
        let expire = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Active, expire);
        let p = plan(&PlanInput {
            kind: PlanKind::Renewal,
            snapshot: &snapshot,
            extra_devices: 2,
            held_devices: None,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[SquadId(Uuid::from_u128(9))],
            profile: &profile(),
            tag: None,
        })
        .unwrap();
        assert_eq!(
            p.target.expire_at,
            Utc.with_ymd_and_hms(2027, 3, 16, 0, 0, 0).unwrap()
        );
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(p.target.squads.len(), 3);
        assert!(!p.reset_traffic);
        assert!(!p.activate);
    }

    #[test]
    fn истёкшая_подписка_продлевается_от_даты_оплаты() {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(11)).unwrap());
        let expired = Utc.with_ymd_and_hms(2026, 11, 10, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Expired, expired);
        let p = plan(&PlanInput {
            kind: PlanKind::Renewal,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[],
            profile: &profile(),
            tag: None,
        })
        .unwrap();
        assert_eq!(p.target.expire_at, paid_at() + Duration::days(30));
        assert!(p.activate);
    }

    #[test]
    fn смена_тарифа_с_даты_оплаты_и_сбросом_трафика() {
        let tariff = unblock();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(32)).unwrap());
        let expire = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Limited, expire);
        let p = plan(&PlanInput {
            kind: PlanKind::Change,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[SquadId(Uuid::from_u128(9))],
            profile: &profile(),
            tag: None,
        })
        .unwrap();
        assert_eq!(p.target.expire_at, paid_at() + Duration::days(90));
        assert_eq!(p.target.squads, snapshot.squads);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert_eq!(p.target.traffic_strategy, TrafficStrategy::Month);
        assert!(p.reset_traffic);
        assert!(p.activate);
    }

    #[test]
    fn без_пользователя_в_remnawave_создаём_нового() {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(12)).unwrap());
        let p = plan(&PlanInput {
            kind: PlanKind::New,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid_at(),
            current: None,
            keep_squads: &[],
            profile: &profile(),
            tag: None,
        })
        .unwrap();
        assert!(matches!(p.action, PlanAction::Create { .. }));
        assert_eq!(p.target.expire_at, paid_at() + Duration::days(90));
        assert_eq!(p.period_days, 90);
    }

    fn renewal(snapshot: &TariffSnapshot, state: &RemnaUserState) -> FulfillmentPlan {
        plan(&PlanInput {
            kind: PlanKind::Renewal,
            snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid_at(),
            current: Some(state),
            keep_squads: &[],
            profile: &profile(),
            tag: None,
        })
        .unwrap()
    }

    fn traffic_state(
        status: SubscriptionStatus,
        limit: u64,
        used: u64,
        strategy: TrafficStrategy,
    ) -> RemnaUserState {
        RemnaUserState {
            traffic_limit_bytes: limit,
            traffic_used_bytes: used,
            traffic_strategy: strategy,
            ..current(status, Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap())
        }
    }

    fn limited_snapshot(reset: TrafficReset) -> TariffSnapshot {
        let tariff = unblock();
        let mut snapshot = tariff.snapshot(tariff.period(PeriodId(31)).unwrap());
        snapshot.traffic.reset = reset;
        snapshot
    }

    #[test]
    fn продление_из_безлимита_в_тариф_с_лимитом_сбрасывает_счётчик() {
        let snapshot = limited_snapshot(TrafficReset::Month);
        let state = traffic_state(
            SubscriptionStatus::Active,
            0,
            1024 * GIB,
            TrafficStrategy::NoReset,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert_eq!(p.target.traffic_strategy, TrafficStrategy::Month);
        assert!(p.reset_traffic);
    }

    #[test]
    fn продление_со_сменой_сброса_трафика_сбрасывает_счётчик_и_включает() {
        let snapshot = limited_snapshot(TrafficReset::Month);
        let state = traffic_state(
            SubscriptionStatus::Limited,
            300 * GIB,
            300 * GIB,
            TrafficStrategy::NoReset,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert!(p.reset_traffic);
        assert!(p.activate);

        let snapshot = limited_snapshot(TrafficReset::NoReset);
        let state = traffic_state(
            SubscriptionStatus::Active,
            300 * GIB,
            250 * GIB,
            TrafficStrategy::Month,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert_eq!(p.target.traffic_strategy, TrafficStrategy::NoReset);
        assert!(p.reset_traffic);
    }

    #[test]
    fn продление_с_тем_же_сбросом_трафика_не_трогает_счётчик() {
        let snapshot = limited_snapshot(TrafficReset::Month);
        let state = traffic_state(
            SubscriptionStatus::Active,
            300 * GIB,
            120 * GIB,
            TrafficStrategy::Month,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert!(!p.reset_traffic);
        assert!(!p.activate);

        let snapshot = limited_snapshot(TrafficReset::NoReset);
        let state = traffic_state(
            SubscriptionStatus::Limited,
            300 * GIB,
            300 * GIB,
            TrafficStrategy::NoReset,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 600 * GIB);
        assert!(!p.reset_traffic);
        assert!(p.activate);

        let snapshot = limited_snapshot(TrafficReset::OnPayment);
        let state = traffic_state(
            SubscriptionStatus::Active,
            300 * GIB,
            10 * GIB,
            TrafficStrategy::NoReset,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert!(p.reset_traffic);
    }

    fn renewal_holding(extra_devices: u32, held: Option<HeldDevices>) -> FulfillmentPlan {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(11)).unwrap());
        let expire = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Active, expire);
        plan(&PlanInput {
            kind: PlanKind::Renewal,
            snapshot: &snapshot,
            extra_devices,
            held_devices: held,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[],
            profile: &profile(),
            tag: None,
        })
        .unwrap()
    }

    #[test]
    fn оплаченные_доп_устройства_держатся_до_конца_прежнего_срока() {
        let until = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let held = HeldDevices { limit: 5, until };
        let p = renewal_holding(0, Some(held));
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(
            p.device_hold,
            Some(DeviceHold {
                held_limit: 5,
                until,
                limit_after: 3,
            })
        );
        assert_eq!(p.base_expire_at, Some(until));

        let p = renewal_holding(1, Some(held));
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(p.device_hold.map(|h| h.limit_after), Some(4));

        let p = renewal_holding(2, Some(held));
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(p.device_hold, None);

        let p = renewal_holding(2, Some(HeldDevices { limit: 4, until }));
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(p.device_hold, None);

        let ended = HeldDevices {
            limit: 5,
            until: paid_at(),
        };
        let p = renewal_holding(0, Some(ended));
        assert_eq!(p.target.hwid_device_limit, Some(3));
        assert_eq!(p.device_hold, None);

        let p = renewal_holding(0, None);
        assert_eq!(p.target.hwid_device_limit, Some(3));
        assert_eq!(p.device_hold, None);
    }

    #[test]
    fn смена_тарифа_и_новая_подписка_доп_устройства_не_держат() {
        let tariff = unblock();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(31)).unwrap());
        let expire = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Active, expire);
        let held = Some(HeldDevices {
            limit: 5,
            until: expire,
        });
        let change = plan(&PlanInput {
            kind: PlanKind::Change,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: held,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[],
            profile: &profile(),
            tag: None,
        })
        .unwrap();
        assert_eq!(change.target.hwid_device_limit, Some(3));
        assert_eq!(change.device_hold, None);
        assert_eq!(change.base_expire_at, Some(expire));
        let fresh = plan(&PlanInput {
            kind: PlanKind::New,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: held,
            paid_at: paid_at(),
            current: None,
            keep_squads: &[],
            profile: &profile(),
            tag: None,
        })
        .unwrap();
        assert_eq!(fresh.device_hold, None);
        assert_eq!(fresh.base_expire_at, None);
    }

    #[test]
    fn сохранённый_раньше_план_читается_без_новых_полей() {
        let p = renewal_holding(0, None);
        let mut json = serde_json::to_value(&p).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("base_expire_at");
        object.remove("device_hold");
        let old: FulfillmentPlan = serde_json::from_value(json).unwrap();
        assert_eq!(old.base_expire_at, None);
        assert_eq!(old.device_hold, None);
        assert_eq!(old.target, p.target);
    }

    fn paid_with_microseconds() -> DateTime<Utc> {
        paid_at() + Duration::microseconds(600_007)
    }

    #[test]
    fn цель_плана_округляется_вверх_до_целой_секунды() {
        let exact = paid_at();
        assert_eq!(whole_seconds(exact), exact);
        assert_eq!(
            whole_seconds(exact + Duration::microseconds(1)),
            exact + Duration::seconds(1)
        );
        assert_eq!(
            whole_seconds(exact + Duration::nanoseconds(999_999_999)),
            exact + Duration::seconds(1)
        );
        let before_epoch = DateTime::from_timestamp(-10, 250_000_000).unwrap();
        assert_eq!(
            whole_seconds(before_epoch),
            DateTime::from_timestamp(-9, 0).unwrap()
        );
        let paid = paid_with_microseconds();
        let target = paid_at() + Duration::seconds(1) + Duration::days(30);
        assert_eq!(new_expire_at(PlanKind::Change, None, paid, 30), target);
        let expired = Utc.with_ymd_and_hms(2026, 11, 10, 0, 0, 0).unwrap();
        assert_eq!(
            new_expire_at(PlanKind::Renewal, Some(expired), paid, 30),
            target
        );
        let active =
            Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap() + Duration::milliseconds(250);
        assert_eq!(
            new_expire_at(PlanKind::Renewal, Some(active), paid, 30),
            Utc.with_ymd_and_hms(2027, 3, 16, 0, 0, 1).unwrap()
        );
    }

    #[test]
    fn план_от_оплаты_с_микросекундами_даёт_срок_без_долей_секунды() {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(11)).unwrap());
        let paid = paid_with_microseconds();
        let target = paid_at() + Duration::seconds(1) + Duration::days(30);
        let fresh = plan(&PlanInput {
            kind: PlanKind::New,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid,
            current: None,
            keep_squads: &[],
            profile: &profile(),
            tag: None,
        })
        .unwrap();
        assert_eq!(fresh.target.expire_at, target);
        assert_eq!(fresh.period_started_at, paid);
        assert_eq!(fresh.period_days, 30);
        let expired = current(
            SubscriptionStatus::Expired,
            Utc.with_ymd_and_hms(2026, 11, 10, 0, 0, 0).unwrap(),
        );
        for kind in [PlanKind::Renewal, PlanKind::Change] {
            let p = plan(&PlanInput {
                kind,
                snapshot: &snapshot,
                extra_devices: 0,
                held_devices: None,
                paid_at: paid,
                current: Some(&expired),
                keep_squads: &[],
                profile: &profile(),
                tag: None,
            })
            .unwrap();
            assert_eq!(p.target.expire_at, target);
            assert_eq!(p.target.expire_at.timestamp_subsec_nanos(), 0);
            assert_eq!(p.period_days, 30);
            assert!(p.target.expire_at - paid >= Duration::days(30));
        }
    }

    #[test]
    fn тариф_без_сквадов_не_выдаётся() {
        let mut tariff = standard();
        tariff.squads.clear();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(12)).unwrap());
        assert!(
            plan(&PlanInput {
                kind: PlanKind::New,
                snapshot: &snapshot,
                extra_devices: 0,
                held_devices: None,
                paid_at: paid_at(),
                current: None,
                keep_squads: &[],
                profile: &profile(),
                tag: None,
            })
            .is_err()
        );
    }
}
