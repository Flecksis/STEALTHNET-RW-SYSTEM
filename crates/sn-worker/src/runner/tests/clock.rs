use super::*;

const HOURS: i64 = 2;
const TASK_EVERY: i64 = 600;

struct Scheduled;

#[async_trait]
impl Module for Scheduled {
    fn id(&self) -> ModuleId {
        ModuleId::Broadcast
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: "digest",
            every: Duration::from_secs(TASK_EVERY as u64),
        }]
    }

    async fn run_task(&self, _ctx: &ModuleContext, _task: &str) -> Result<(), ModuleError> {
        Ok(())
    }
}

struct Panel {
    worker: Worker,
    clock: Arc<FixedClock>,
}

impl Panel {
    async fn starts(&self, periodic: Periodic) -> bool {
        for _ in 0..50 {
            if self.worker.run_periodic(periodic).await.unwrap().is_some() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    async fn rests(&self, periodic: Periodic) -> bool {
        self.worker.run_periodic(periodic).await.unwrap().is_none()
    }

    async fn started_at(&self, periodic: Periodic) -> Option<DateTime<Utc>> {
        periodic_state(&self.worker.services, periodic)
            .await
            .unwrap()
            .last_started_at
    }

    async fn schedules(&self) -> i64 {
        self.worker.schedule_module_tasks().await.unwrap()["scheduled"]
            .as_i64()
            .unwrap()
    }
}

struct Stand {
    setup: Setup,
    started: DateTime<Utc>,
}

impl Stand {
    async fn new() -> Option<Stand> {
        let started = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let setup = setup_with_clock(Vec::new(), Arc::new(FixedClock::new(started))).await?;
        Some(Stand { setup, started })
    }

    async fn panel(&self, ahead: chrono::Duration) -> Panel {
        let clock = Arc::new(FixedClock::new(self.started + ahead));
        let services = Services::new(ServiceDeps {
            store: self.setup.db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::new(vec![Arc::new(Scheduled)]).unwrap()),
            secrets: self.setup.services.secrets().cloned(),
            clock: clock.clone(),
            env: EnvFallbacks::default(),
        });
        services.reload().await.unwrap();
        Panel {
            worker: Worker::new(services, WorkerConfig::default()),
            clock,
        }
    }

    async fn database_passes(&self, seconds: i64) {
        let mut conn = self.setup.db.store.conn().await.unwrap();
        sqlx::query("UPDATE bot_state SET updated_at = updated_at - make_interval(secs => $1)")
            .bind(seconds as f64)
            .execute(conn.conn())
            .await
            .unwrap();
    }

    async fn pass(&self, seconds: i64, panels: &[&Panel]) {
        for panel in panels {
            panel
                .clock
                .set(panel.worker.services.now() + chrono::Duration::seconds(seconds));
        }
        self.database_passes(seconds).await;
    }

    async fn finish(self) {
        self.setup.db.cleanup().await;
    }
}

#[tokio::test]
async fn после_перевода_часов_назад_периодическая_работа_идёт_дальше() {
    let Some(stand) = Stand::new().await else {
        return;
    };
    let panel = stand.panel(chrono::Duration::hours(HOURS)).await;
    for periodic in [Periodic::ChainWatch, Periodic::EndExpired] {
        panel
            .clock
            .set(stand.started + chrono::Duration::hours(HOURS));
        assert!(panel.starts(periodic).await, "{periodic:?}: первый запуск");
        panel.clock.set(stand.started);
        assert!(panel.rests(periodic).await, "{periodic:?}: запись свежая");
        let every = i64::try_from(panel.worker.interval_of(periodic).as_secs()).unwrap();
        stand.pass(every, &[&panel]).await;
        assert!(panel.starts(periodic).await, "{periodic:?}: работа стоит");
        assert_eq!(
            panel.started_at(periodic).await,
            Some(panel.worker.services.now()),
            "{periodic:?}: время запуска осталось в будущем"
        );
        assert!(
            panel.rests(periodic).await,
            "{periodic:?}: второй раз подряд"
        );
        stand.pass(every, &[&panel]).await;
        assert!(
            panel.starts(periodic).await,
            "{periodic:?}: дальше как обычно"
        );
    }
    stand.finish().await;
}

#[tokio::test]
async fn после_перевода_назад_часов_всей_машины_работа_идёт_сразу() {
    let Some(stand) = Stand::new().await else {
        return;
    };
    let panel = stand.panel(chrono::Duration::hours(HOURS)).await;
    for periodic in [Periodic::ChainWatch, Periodic::EndExpired] {
        assert!(panel.starts(periodic).await, "{periodic:?}");
    }
    assert_eq!(panel.schedules().await, 1);
    panel.clock.set(stand.started);
    stand.database_passes(-HOURS * 3_600).await;
    for periodic in [Periodic::ChainWatch, Periodic::EndExpired] {
        assert!(panel.starts(periodic).await, "{periodic:?}: работа стоит");
        assert!(
            panel.rests(periodic).await,
            "{periodic:?}: второй раз подряд"
        );
    }
    assert_eq!(panel.schedules().await, 1, "задача модуля стоит");
    assert_eq!(panel.schedules().await, 0);
    stand.finish().await;
}

#[tokio::test]
async fn после_перевода_часов_назад_задачи_модулей_ставятся_через_свой_интервал() {
    let Some(stand) = Stand::new().await else {
        return;
    };
    let panel = stand.panel(chrono::Duration::hours(HOURS)).await;
    assert_eq!(panel.schedules().await, 1);
    panel.clock.set(stand.started);
    assert_eq!(panel.schedules().await, 0);
    stand.pass(TASK_EVERY - 30, &[&panel]).await;
    assert_eq!(panel.schedules().await, 0, "интервал ещё не прошёл");
    stand.pass(60, &[&panel]).await;
    assert_eq!(panel.schedules().await, 1, "задача стоит");
    assert_eq!(panel.schedules().await, 0, "второй раз подряд");
    stand.pass(TASK_EVERY, &[&panel]).await;
    assert_eq!(panel.schedules().await, 1, "дальше по обычному расписанию");
    stand.finish().await;
}

#[tokio::test]
async fn две_копии_с_разными_часами_ставят_задачу_модуля_один_раз() {
    for ahead in [-3_600, 700, 2 * 3_600, 400 * 24 * 3_600] {
        let Some(stand) = Stand::new().await else {
            return;
        };
        let other = stand.panel(chrono::Duration::seconds(ahead)).await;
        let exact = stand.panel(chrono::Duration::zero()).await;
        let mut tasks = 0;
        for _ in 0..8 {
            for panel in [&other, &exact] {
                tasks += panel.schedules().await;
                stand.pass(15, &[&other, &exact]).await;
            }
        }
        let first = i64::from(ahead < 0);
        assert_eq!(tasks, 1 + first, "расхождение {ahead} с");
        stand.finish().await;
    }
}

#[tokio::test]
async fn копия_с_точными_часами_подхватывает_работу_за_остановленной_спешащей() {
    let Some(stand) = Stand::new().await else {
        return;
    };
    let fast = stand.panel(chrono::Duration::hours(HOURS)).await;
    let exact = stand.panel(chrono::Duration::zero()).await;
    assert!(fast.starts(Periodic::ChainWatch).await);
    assert_eq!(fast.schedules().await, 1);
    stand.pass(7, &[&exact]).await;
    assert!(exact.rests(Periodic::ChainWatch).await);
    assert_eq!(exact.schedules().await, 0);
    stand.pass(13, &[&exact]).await;
    assert!(exact.starts(Periodic::ChainWatch).await);
    assert_eq!(exact.schedules().await, 0);
    stand.pass(TASK_EVERY, &[&exact]).await;
    assert_eq!(exact.schedules().await, 1);
    stand.finish().await;
}

#[derive(Debug, Default, PartialEq)]
struct Hour {
    housekeeping: i64,
    schedules: i64,
    tasks: i64,
}

#[derive(Clone, Copy)]
struct Saved {
    value: i64,
    written: i64,
}

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
}

fn hour_of(copies: &[(i64, i64)], busy: bool) -> Hour {
    let mut hour = Hour::default();
    let mut started = [None::<Saved>; 2];
    let mut task = None::<Saved>;
    for now in 0..3_600 {
        for (copy, (ahead, phase)) in copies.iter().enumerate() {
            if now % 15 != *phase || (busy && copy == 0 && now % 600 >= 500) {
                continue;
            }
            let local = now + ahead;
            let mut due = |work: usize, every: i64| {
                let pause = chrono::Duration::seconds(every) * 9 / 10;
                let free = started[work].is_none_or(|s: Saved| {
                    let age = chrono::Duration::seconds(now - s.written);
                    !rests(at(local), at(s.value), age, pause)
                });
                if free {
                    started[work] = Some(Saved {
                        value: local,
                        written: now,
                    });
                }
                free
            };
            hour.housekeeping += i64::from(due(0, 60));
            if !due(1, 15) {
                continue;
            }
            hour.schedules += 1;
            let slot = at(local).timestamp().div_euclid(TASK_EVERY);
            let taken = task.is_some_and(|t| {
                let age = chrono::Duration::seconds(now - t.written);
                slot_taken(slot, t.value, age, TASK_EVERY)
            });
            if !taken {
                hour.tasks += 1;
                task = Some(Saved {
                    value: slot,
                    written: now,
                });
            }
        }
    }
    hour
}

#[test]
fn за_час_копии_с_разными_часами_работают_как_одна() {
    let one = hour_of(&[(0, 0)], false);
    assert_eq!(
        one,
        Hour {
            housekeeping: 60,
            schedules: 240,
            tasks: 6
        }
    );
    for ahead in [0, 1, 40, 700, 2 * 3_600, 400 * 24 * 3_600, -40, -2 * 3_600] {
        let two = hour_of(&[(ahead, 0), (0, 7)], false);
        assert!((60..=62).contains(&two.housekeeping), "{ahead}: {two:?}");
        assert!((240..=242).contains(&two.schedules), "{ahead}: {two:?}");
        assert!((6..=8).contains(&two.tasks), "{ahead}: {two:?}");
        let handover = hour_of(&[(ahead, 0), (0, 7)], true);
        assert!(
            (54..=72).contains(&handover.housekeeping),
            "{ahead}: {handover:?}"
        );
        assert!(
            (230..=260).contains(&handover.schedules),
            "{ahead}: {handover:?}"
        );
        assert!((6..=14).contains(&handover.tasks), "{ahead}: {handover:?}");
    }
}

#[test]
fn время_из_будущего_решается_по_часам_базы() {
    let minute = chrono::Duration::seconds(60);
    let seconds = chrono::Duration::seconds;
    for (started, age, resting) in [
        (-59, 0, true),
        (-60, 0, false),
        (0, 0, true),
        (7_200, 0, true),
        (7_200, 59, true),
        (7_200, 60, false),
        (7_200, 3_600, false),
        (7_200, -1, false),
        (7_200, -7_200, false),
        (-60, -7_200, false),
        (-10, 7_200, true),
    ] {
        assert_eq!(
            rests(at(0), at(started), seconds(age), minute),
            resting,
            "запуск {started} с, запись {age} с"
        );
    }
    for (last, age, taken) in [
        (99, 0, false),
        (100, 0, true),
        (100, 7_200, true),
        (100, -7_200, true),
        (112, 0, true),
        (112, 599, true),
        (112, 600, false),
        (112, -1, false),
    ] {
        assert_eq!(
            slot_taken(100, last, seconds(age), TASK_EVERY),
            taken,
            "отрезок {last}, запись {age} с"
        );
    }
    assert!(slot_taken(0, 1, seconds(7_200), i64::MAX));
}
