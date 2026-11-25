use super::*;
use crate::clock::FixedClock;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

const HOUR: i64 = 3_600;

fn clock() -> ServerClock {
    ServerClock::new(Arc::new(FixedClock::new(Utc::now())))
}

fn after(start: Instant, seconds: u64) -> Instant {
    start + Duration::from_secs(seconds)
}

fn verdict(clock: &ServerClock, at: Instant) -> Option<Drift> {
    clock.lock().verdict(at)
}

fn telegram(seconds: i64) -> Option<Drift> {
    Some(Drift {
        seconds,
        evidence: Evidence::Telegram,
    })
}

fn login(seconds: i64) -> Option<Drift> {
    Some(Drift {
        seconds,
        evidence: Evidence::Login,
    })
}

fn logins(seconds: i64) -> Option<Drift> {
    Some(Drift {
        seconds,
        evidence: Evidence::Logins,
    })
}

#[test]
fn расхождение_признаётся_со_второго_ответа_телеграма() {
    let clock = clock();
    let now = Instant::now();
    clock.answered_at(now, 2 * HOUR);
    assert_eq!(verdict(&clock, now), None);
    clock.answered_at(after(now, 25), 2 * HOUR + 1);
    assert_eq!(verdict(&clock, after(now, 25)), telegram(2 * HOUR + 1));
    clock.answered_at(after(now, 50), 1);
    assert_eq!(verdict(&clock, after(now, 50)), None);
}

#[test]
fn меньше_минуты_расхождением_не_считается() {
    for (seconds, drift) in [(59, false), (60, true), (-59, false), (-60, true)] {
        let clock = clock();
        let now = Instant::now();
        clock.answered_at(now, seconds);
        clock.answered_at(after(now, 25), seconds);
        assert_eq!(
            verdict(&clock, after(now, 25)),
            telegram(seconds).filter(|_| drift),
            "{seconds}"
        );
    }
}

#[test]
fn у_порога_предупреждение_не_мигает() {
    let clock = clock();
    let now = Instant::now();
    for (step, seconds) in [60, 61, 59, 46].into_iter().enumerate() {
        clock.answered_at(after(now, 25 * step as u64), seconds);
    }
    assert_eq!(verdict(&clock, after(now, 75)), telegram(46));
    clock.answered_at(after(now, 100), 44);
    assert_eq!(verdict(&clock, after(now, 100)), None);
    clock.answered_at(after(now, 125), 59);
    clock.answered_at(after(now, 150), 59);
    assert_eq!(verdict(&clock, after(now, 150)), None);
}

#[test]
fn после_перевода_в_другую_сторону_нужен_новый_второй_ответ() {
    let clock = clock();
    let now = Instant::now();
    clock.answered_at(now, 2 * HOUR);
    clock.answered_at(after(now, 25), 2 * HOUR);
    clock.answered_at(after(now, 50), -300);
    assert_eq!(verdict(&clock, after(now, 50)), None);
    clock.answered_at(after(now, 75), -300);
    assert_eq!(verdict(&clock, after(now, 75)), telegram(-300));
}

#[test]
fn старые_ответы_телеграма_ничего_не_значат() {
    let clock = clock();
    let now = Instant::now();
    clock.answered_at(now, 2 * HOUR);
    clock.answered_at(after(now, 601), 2 * HOUR);
    assert_eq!(
        verdict(&clock, after(now, 601)),
        None,
        "между ответами больше десяти минут"
    );
    clock.answered_at(after(now, 626), 2 * HOUR);
    assert_eq!(verdict(&clock, after(now, 1_226)), telegram(2 * HOUR));
    assert_eq!(verdict(&clock, after(now, 1_227)), None);
}

#[test]
fn закрытие_мини_приложения_считается_по_порогам_входа() {
    for (seconds, closed) in [
        (HOUR, true),
        (HOUR - 1, false),
        (60, false),
        (-60, false),
        (-61, true),
    ] {
        let drift = telegram(seconds).unwrap();
        assert_eq!(drift.closes_miniapp(), closed, "{seconds}");
    }
    assert_eq!(telegram(7_380).unwrap().span(), "2 ч 3 мин");
    assert_eq!(telegram(-300).unwrap().span(), "5 минут");
    assert!(!telegram(i64::MIN).unwrap().span().is_empty());
}

#[test]
fn виноваты_часы_если_с_таким_расхождением_не_войти_никому() {
    let now = Instant::now();
    for (drift, age, blame) in [
        (2 * HOUR, 2 * HOUR + 3, Blame::Server),
        (2 * HOUR, 9 * HOUR, Blame::Server),
        (HOUR, HOUR + 1, Blame::Server),
        (1_800, HOUR + 600, Blame::Data),
        (1, 2 * HOUR, Blame::Data),
        (-300, 2 * HOUR, Blame::Server),
        (-60, 2 * HOUR, Blame::Data),
    ] {
        let clock = clock();
        clock.answered_at(now, drift);
        clock.answered_at(after(now, 25), drift);
        assert_eq!(
            clock.refused_at(after(now, 26), Some(1), age),
            blame,
            "расхождение {drift}, данным {age}"
        );
    }
}

#[test]
fn данные_из_будущего_означают_что_часы_отстают() {
    let clock = clock();
    let now = Instant::now();
    assert_eq!(clock.refused_at(now, Some(1), -61), Blame::Server);
    assert_eq!(verdict(&clock, now), login(-61));
    assert_eq!(
        clock.refused_at(after(now, 1), Some(2), -300),
        Blame::Server
    );
    assert_eq!(clock.refused_at(after(now, 2), Some(3), -90), Blame::Server);
    assert_eq!(
        verdict(&clock, after(now, 2)),
        login(-300),
        "видно самое большое отставание"
    );
    assert_eq!(
        clock.refused_at(after(now, 3), Some(4), 2 * HOUR),
        Blame::Server,
        "переоткрыть мини-приложение не поможет"
    );
    assert_eq!(verdict(&clock, after(now, 903)), None);
    assert_eq!(
        clock.refused_at(after(now, 904), Some(1), -61),
        Blame::Server
    );
    clock.admitted_at(after(now, 905));
    assert_eq!(verdict(&clock, after(now, 905)), None, "вход прошёл");
    assert_eq!(
        clock.refused_at(after(now, 906), Some(1), -61),
        Blame::Server
    );
    clock.answered_at(after(now, 907), 0);
    assert_eq!(
        verdict(&clock, after(now, 907)),
        None,
        "Telegram ответил тем же временем"
    );
}

#[test]
fn доля_секунды_за_порогом_считается_отставанием_больше_минуты() {
    let clock = clock();
    assert_eq!(
        clock.refused(Some(1), chrono::Duration::milliseconds(-60_400)),
        Blame::Server
    );
    let drift = verdict(&clock, Instant::now()).unwrap();
    assert_eq!(drift.seconds, -61);
    assert!(drift.closes_miniapp());
}

#[test]
fn без_ответов_телеграма_часы_подозреваются_после_трёх_разных_клиентов() {
    let clock = clock();
    let now = Instant::now();
    for step in 0..5 {
        assert_eq!(
            clock.refused_at(after(now, step), Some(1), 2 * HOUR),
            Blame::Data,
            "один и тот же клиент"
        );
    }
    assert_eq!(
        clock.refused_at(after(now, 10), Some(2), 5 * HOUR),
        Blame::Data
    );
    assert_eq!(verdict(&clock, after(now, 10)), None);
    assert_eq!(
        clock.refused_at(after(now, 20), Some(3), 2 * HOUR + 40),
        Blame::Server
    );
    assert_eq!(verdict(&clock, after(now, 20)), logins(2 * HOUR));
    clock.admitted_at(after(now, 30));
    assert_eq!(verdict(&clock, after(now, 30)), None);
    for (step, user) in [(40, 4), (41, 5), (42, 6)] {
        assert_eq!(
            clock.refused_at(after(now, step), Some(user), 2 * HOUR),
            Blame::Data,
            "только что кто-то вошёл"
        );
    }
}

#[test]
fn старые_отказы_и_сверенные_часы_подозрений_не_дают() {
    let now = Instant::now();
    let slow = clock();
    for (step, user) in [(0, 1), (10, 2), (905, 3)] {
        assert_eq!(
            slow.refused_at(after(now, step), Some(user), 2 * HOUR),
            Blame::Data,
            "{user}"
        );
    }
    assert_eq!(
        slow.refused_at(after(now, 906), Some(2), 2 * HOUR),
        Blame::Data
    );
    assert_eq!(
        slow.refused_at(after(now, 907), Some(4), 2 * HOUR),
        Blame::Server
    );

    let exact = clock();
    exact.answered_at(now, 1);
    for user in 1..10 {
        assert_eq!(exact.refused_at(now, Some(user), 2 * HOUR), Blame::Data);
    }
    assert_eq!(verdict(&exact, now), None);
}

fn report(clock: &ServerClock, at: Instant) -> Report {
    let report = clock.report(at);
    match report {
        Report::Save(_) => clock.reported(true),
        Report::Forget => clock.reported(false),
        Report::Nothing => {}
    }
    report
}

#[test]
fn запись_для_админки_держится_пока_часы_расходятся_и_снимается_один_раз() {
    let clock = clock();
    let now = Instant::now();
    assert_eq!(
        report(&clock, now),
        Report::Nothing,
        "ещё ничего не известно"
    );
    clock.answered_at(now, 2 * HOUR);
    clock.answered_at(after(now, 25), 2 * HOUR);
    let drift = telegram(2 * HOUR).unwrap();
    assert_eq!(report(&clock, after(now, 30)), Report::Save(drift));
    assert_eq!(report(&clock, after(now, 90)), Report::Save(drift));
    clock.answered_at(after(now, 100), 0);
    assert_eq!(clock.report(after(now, 150)), Report::Forget);
    assert_eq!(
        report(&clock, after(now, 210)),
        Report::Forget,
        "прошлый раз запись не снялась"
    );
    assert_eq!(report(&clock, after(now, 270)), Report::Nothing);
    clock.answered_at(after(now, 280), 0);
    assert_eq!(report(&clock, after(now, 330)), Report::Nothing);
}

#[test]
fn после_перезапуска_старая_запись_снимается_первым_верным_ответом_или_входом() {
    let now = Instant::now();
    let answered = clock();
    answered.answered_at(now, 0);
    assert_eq!(report(&answered, now), Report::Forget);
    assert_eq!(report(&answered, after(now, 60)), Report::Nothing);
    let admitted = clock();
    admitted.admitted_at(now);
    assert_eq!(report(&admitted, now), Report::Forget);
    assert_eq!(report(&admitted, after(now, 60)), Report::Nothing);
}

struct Lines(Arc<Mutex<Vec<String>>>);

struct Line(String);

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl Subscriber for Lines {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = Line(event.metadata().level().to_string());
        event.record(&mut line);
        self.0.lock().unwrap().push(line.0);
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

fn journal(work: impl FnOnce()) -> Vec<String> {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let second = tracing::Dispatch::new(Lines(Arc::default()));
    tracing::dispatcher::with_default(&tracing::Dispatch::new(Lines(lines.clone())), work);
    drop(second);
    let lines = lines.lock().unwrap();
    lines
        .iter()
        .filter(|line| !line.starts_with("DEBUG") && !line.starts_with("TRACE"))
        .cloned()
        .collect()
}

#[test]
fn в_журнале_видно_насколько_часы_расходятся_с_телеграмом() {
    let clock = clock();
    let now = Instant::now();
    let lines = journal(|| {
        for step in 0..200 {
            clock.answered_at(after(now, 25 * step), 7_380);
        }
        clock.answered_at(after(now, 5_000), 1);
        clock.answered_at(after(now, 5_025), 1);
    });
    assert_eq!(lines.len(), 3, "раз в час и при исправлении: {lines:#?}");
    for line in &lines[..2] {
        assert!(line.starts_with("WARN"), "{line}");
        assert!(line.contains("drift_seconds=7380"), "{line}");
        assert!(
            line.contains("часы сервера спешат на 2 ч 3 мин по сравнению с Telegram"),
            "{line}"
        );
        assert!(line.contains("мини-приложение никого не пускает"), "{line}");
        assert!(line.contains("timedatectl set-ntp true"), "{line}");
    }
    assert!(lines[2].contains("снова сходятся"), "{}", lines[2]);
}

#[test]
fn небольшое_расхождение_в_журнале_без_слов_о_мини_приложении() {
    let clock = clock();
    let now = Instant::now();
    let lines = journal(|| {
        clock.answered_at(now, 300);
        clock.answered_at(after(now, 25), 300);
    });
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains("спешат на 5 минут"), "{}", lines[0]);
    assert!(!lines[0].contains("мини-приложение"), "{}", lines[0]);
    let behind = ServerClock::new(Arc::new(FixedClock::new(Utc::now())));
    let lines = journal(|| {
        behind.answered_at(now, -300);
        behind.answered_at(after(now, 25), -300);
    });
    assert!(lines[0].contains("отстают на 5 минут"), "{}", lines[0]);
    assert!(lines[0].contains("мини-приложение"), "{}", lines[0]);
}

#[test]
fn отказы_во_входе_пишутся_не_чаще_раза_в_пять_минут_и_с_величиной() {
    let now = Instant::now();
    let behind = clock();
    let lines = journal(|| {
        for step in 0..60 {
            behind.refused_at(after(now, 10 * step), Some(1), -125);
        }
    });
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert!(
        lines[0].contains("часы сервера отстают не меньше чем на 2 минуты"),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("drift_seconds=-125"), "{}", lines[0]);

    let unknown = clock();
    let lines = journal(|| {
        unknown.refused_at(now, Some(1), 7_380);
    });
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains("данным от Telegram 2 ч 3 мин, принимаются не старше часа"),
        "{}",
        lines[0]
    );
    assert!(
        lines[0].contains("если так у всех клиентов"),
        "{}",
        lines[0]
    );
    let lines = journal(|| {
        unknown.refused_at(after(now, 1), Some(2), 7_380);
        unknown.refused_at(after(now, 2), Some(3), 7_390);
        unknown.refused_at(after(now, 3), Some(4), 7_390);
    });
    assert_eq!(lines.len(), 1, "вывод пишется сразу: {lines:#?}");
    assert!(
        lines[0].contains("часы сервера, похоже, спешат на 2 ч 3 мин"),
        "{}",
        lines[0]
    );

    let exact = clock();
    let lines = journal(|| {
        exact.answered_at(now, 0);
        exact.refused_at(now, Some(1), 7_380);
    });
    assert!(
        lines.is_empty(),
        "часы верные, данные просто старые: {lines:#?}"
    );
}
