use super::*;
use chrono::TimeZone;

fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, day, hour, minute, 0)
        .unwrap()
}

fn moscow() -> FixedOffset {
    offset_of(180)
}

fn sub(trial: bool, unlimited: bool, devices: Option<u32>) -> SubFacts {
    SubFacts {
        subscription_id: SubscriptionId(1),
        client_id: ClientId(2),
        trial,
        unlimited,
        devices,
    }
}

fn input() -> RuleInput {
    RuleInput {
        enabled: true,
        audience: Audience::All,
        traffic: TrafficScope::Any,
        devices_min: None,
        period: Period::Day,
        limit_gb: 50.0,
        per_device: false,
        warn: true,
        alert: false,
        restriction: Restriction::None,
        squad: None,
        note: None,
    }
}

#[test]
fn окно_суток_и_недели_считается_по_часам_панели() {
    let day = window(Period::Day, utc(18, 21, 30), moscow());
    assert_eq!(day.start, NaiveDate::from_ymd_opt(2026, 11, 19).unwrap());
    assert_eq!(day.starts_at, utc(18, 21, 0));
    assert_eq!(day.ends_at, utc(19, 21, 0));
    let week = window(Period::Week, utc(18, 12, 0), moscow());
    assert_eq!(week.start, NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
    assert_eq!(week.starts_at, utc(15, 21, 0));
    assert_eq!(week.ends_at, utc(22, 21, 0));
    let sunday_night = window(Period::Week, utc(22, 20, 59), moscow());
    assert_eq!(sunday_night.start, week.start);
    let monday = window(Period::Week, utc(22, 21, 0), moscow());
    assert_eq!(monday.start, NaiveDate::from_ymd_opt(2026, 11, 23).unwrap());
    assert_eq!(until_text(day.ends_at, moscow()), "20.11 в 00:00");
    assert_eq!(offset_of(5000), offset_of(1439));
}

#[test]
fn счётчик_начинает_окно_с_последнего_замера_у_границы() {
    let offset = offset_of(0);
    let first = advance(None, 100, utc(18, 23, 50), offset);
    assert_eq!(first.used(Period::Day), 0);
    let later = advance(Some(&first), 160, utc(18, 23, 55), offset);
    assert_eq!(later.used(Period::Day), 60);
    let after_midnight = advance(Some(&later), 170, utc(19, 0, 5), offset);
    assert_eq!(after_midnight.day_base, 160);
    assert_eq!(after_midnight.used(Period::Day), 10);
    assert_eq!(after_midnight.used(Period::Week), 70);
    let evening = advance(None, 100, utc(18, 20, 0), offset);
    let after_gap = advance(Some(&evening), 500, utc(19, 6, 0), offset);
    assert_eq!(after_gap.used(Period::Day), 0);
    assert_eq!(after_gap.used(Period::Week), 400);
    let reset = advance(Some(&later), 40, utc(18, 23, 58), offset);
    assert_eq!(reset.used(Period::Day), 0);
    assert_eq!(reset.used(Period::Week), 0);
    let next_week = advance(Some(&later), 900, utc(23, 9, 0), offset);
    assert_eq!(next_week.used(Period::Week), 0);
}

#[test]
fn правило_выбирает_подписки_по_свойствам_тарифа() {
    let mut rule = validate(&input(), utc(18, 0, 0)).unwrap();
    assert!(rule.matches(&sub(true, true, None)));
    assert!(rule.matches(&sub(false, false, Some(3))));
    rule.audience = Audience::Paid;
    rule.traffic = TrafficScope::Unlimited;
    assert!(!rule.matches(&sub(true, true, None)));
    assert!(rule.matches(&sub(false, true, None)));
    assert!(!rule.matches(&sub(false, false, None)));
    rule.devices_min = Some(3);
    assert!(!rule.matches(&sub(false, true, Some(2))));
    assert!(!rule.matches(&sub(false, true, None)));
    assert!(rule.matches(&sub(false, true, Some(5))));
    assert_eq!(rule.threshold(&sub(false, true, Some(5))), 50 * GIB);
    rule.per_device = true;
    assert_eq!(rule.threshold(&sub(false, true, Some(5))), 250 * GIB);
    assert_eq!(rule.threshold(&sub(false, true, None)), 50 * GIB);
    assert_eq!(
        rule.describe(),
        "Платные без лимита трафика, от 3 устройств · 50\u{a0}ГБ на устройство в сутки"
    );
    rule.enabled = false;
    assert!(!rule.matches(&sub(false, true, Some(5))));
}

#[test]
fn правило_проверяется_перед_сохранением() {
    let now = utc(18, 0, 0);
    let half = RuleInput {
        limit_gb: 0.5,
        ..input()
    };
    assert_eq!(validate(&half, now).unwrap().limit_bytes, GIB / 2);
    for bad in [
        RuleInput {
            limit_gb: 0.0,
            ..input()
        },
        RuleInput {
            limit_gb: f64::NAN,
            ..input()
        },
        RuleInput {
            devices_min: Some(0),
            ..input()
        },
        RuleInput {
            restriction: Restriction::Squad,
            ..input()
        },
        RuleInput {
            warn: false,
            ..input()
        },
        RuleInput {
            note: Some("я".repeat(201)),
            ..input()
        },
    ] {
        assert!(validate(&bad, now).is_err(), "{bad:?}");
    }
    let squad = Uuid::from_u128(7);
    let moved = validate(
        &RuleInput {
            restriction: Restriction::Squad,
            squad: Some(squad),
            warn: false,
            note: Some("  ночной сквад  ".into()),
            ..input()
        },
        now,
    )
    .unwrap();
    assert_eq!(moved.squad, Some(squad));
    assert_eq!(moved.note.as_deref(), Some("ночной сквад"));
    let plain = validate(
        &RuleInput {
            squad: Some(squad),
            ..input()
        },
        now,
    )
    .unwrap();
    assert_eq!(plain.squad, None);
}

#[test]
fn уведомление_клиенту_несёт_готовые_значения() {
    let exceed = store::Exceed {
        id: 1,
        rule_id: 2,
        subscription_id: SubscriptionId(3),
        client_id: ClientId(4),
        period: Period::Day,
        window_start: NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(),
        window_end: utc(18, 21, 0),
        limit_bytes: 50 * GIB,
        used_bytes: 63 * GIB + GIB / 5,
        warn: true,
        alert: true,
        restriction: Restriction::Disable,
        squad: None,
        previous_squads: Vec::new(),
        restrict_tries: 0,
        exceeded_at: utc(18, 12, 0),
        warned_at: None,
        restricted_at: None,
        restrict_note: None,
        lifted_at: None,
        lift_reason: None,
        lift_due_at: None,
    };
    let warning = notice(&exceed, TEMPLATE_RESTRICTED, moscow());
    assert_eq!(warning.template, TEMPLATE_RESTRICTED);
    assert_eq!(warning.params["used"], "63,2\u{a0}ГБ");
    assert_eq!(warning.params["limit"], "50\u{a0}ГБ");
    assert_eq!(warning.params["when"], "Сегодня");
    assert_eq!(warning.params["per"], "в сутки");
    assert_eq!(warning.params["until"], "19.11 в 00:00");
    assert_eq!(warning.params["also_mail"], true);
    let lifted = notice(&exceed, TEMPLATE_LIFTED, moscow());
    assert_eq!(lifted.params["also_mail"], false);
}
