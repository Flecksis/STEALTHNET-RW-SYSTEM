use super::*;
use crate::{EnvFallbacks, ServiceDeps, SystemClock};
use chrono::Duration;
use sn_domain::client::{Client, IdentityKind, Locale, NewIdentity, RegistrationSource};
use sn_domain::ids::{RemnaUserId, SquadId, SubscriptionId};
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, Subscription, SubscriptionKind, remna_description,
};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, ExtraDevices};
use sn_domain::traffic::{GIB, TrafficPolicy, TrafficReset, TrafficStrategy};
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_remnawave::fake::{FakeRemnawave, Fault, sample_user};
use sn_remnawave::{
    InternalSquad, RemnaUser, RemnawaveApi, SquadInfo, SquadRef, UpdateUserRequest, UserStatus,
};
use sn_store::testing::{TestDb, test_store};
use sn_store::{NewCategory, NewClient, NewPeriod, NewSubscription, NewTariff, SecretBox};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Env {
    db: TestDb,
    services: Services,
    fake: Arc<FakeRemnawave>,
}

async fn env() -> Option<Env> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_key(&[7u8; 32]).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    let fake = Arc::new(FakeRemnawave::new());
    let api: Arc<dyn RemnawaveApi> = fake.clone();
    services.set_remna(Some(api));
    Some(Env { db, services, fake })
}

fn squad(uuid: Uuid, name: &str) -> InternalSquad {
    InternalSquad {
        uuid,
        name: name.into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }
}

fn uuid(n: u128) -> Uuid {
    Uuid::from_u128(0xa000_0000_0000_4000_8000_0000_0000_0000 + n)
}

impl Env {
    fn ops(&self) -> RemnaTransfer<'_> {
        self.services.remna_transfer().with_pace(Pace::quick())
    }

    fn switch(&self, fresh: &Arc<FakeRemnawave>) {
        let api: Arc<dyn RemnawaveApi> = fresh.clone();
        self.services.set_remna(Some(api));
    }

    async fn client(&self, telegram: Option<(i64, &str)>, email: Option<&str>) -> Client {
        let mut conn = self.db.store.conn().await.unwrap();
        let client = conn
            .clients()
            .create(&NewClient {
                locale: Locale::Ru,
                registration_source: RegistrationSource::Bot,
                note: None,
            })
            .await
            .unwrap();
        if let Some((id, username)) = telegram {
            conn.clients()
                .add_identity(
                    client.id,
                    &NewIdentity {
                        kind: IdentityKind::Telegram,
                        value: id.to_string(),
                        verified: true,
                        meta: serde_json::json!({ "username": username }),
                    },
                )
                .await
                .unwrap();
        }
        if let Some(email) = email {
            conn.clients()
                .add_identity(
                    client.id,
                    &NewIdentity {
                        kind: IdentityKind::Email,
                        value: email.into(),
                        verified: true,
                        meta: serde_json::json!({}),
                    },
                )
                .await
                .unwrap();
        }
        client
    }

    async fn linked(&self, client: &Client, user: RemnaUser) -> Subscription {
        self.fake.insert_user(user.clone());
        let mut tx = self.db.store.begin().await.unwrap();
        let subscription = tx
            .subscriptions()
            .create(&NewSubscription {
                client_id: client.id,
                tariff_id: None,
                kind: SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: AutoRenewState::default(),
            })
            .await
            .unwrap();
        tx.subscriptions()
            .link_remna(
                subscription.id,
                &RemnaLink {
                    user_id: user.id,
                    username: user.username.clone(),
                    short_uuid: Some(user.short_uuid.clone()),
                },
            )
            .await
            .unwrap();
        tx.subscriptions()
            .apply_mirror(
                subscription.id,
                &user.mirror(Some(0), Utc::now() - Duration::hours(1)),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(
            self.services
                .remna_sync()
                .refresh_subscription(subscription.id)
                .await
                .unwrap()
        );
        self.subscription(subscription.id).await
    }

    async fn subscription(&self, id: SubscriptionId) -> Subscription {
        let mut conn = self.db.store.conn().await.unwrap();
        conn.subscriptions().get(id).await.unwrap()
    }

    async fn tariff(&self, name: &str, squads: Vec<Uuid>) -> i64 {
        let mut tx = self.db.store.begin().await.unwrap();
        let category = tx
            .tariffs()
            .create_category(&NewCategory {
                name: format!("Категория {name}"),
                sort: 0,
                visible: true,
            })
            .await
            .unwrap();
        let tariff = tx
            .tariffs()
            .create(&NewTariff {
                traffic_addon: None,
                category_id: category.id,
                name: name.into(),
                description: None,
                on_sale: true,
                sort: 0,
                devices: DevicePolicy {
                    included: 3,
                    extra: Some(ExtraDevices {
                        max: 2,
                        monthly: CurrencyPrices {
                            usd: Some(99),
                            rub: Some(4_900),
                        },
                        monthly_stars: None,
                    }),
                },
                traffic: TrafficPolicy {
                    limit_bytes: Some(100 * GIB),
                    reset: TrafficReset::Month,
                },
                squads: squads.into_iter().map(SquadId).collect(),
                external_squad: None,
                periods: vec![NewPeriod {
                    days: 30,
                    prices: CurrencyPrices {
                        usd: Some(500),
                        rub: Some(29_900),
                    },
                    stars: None,
                    sort: 0,
                    active: true,
                }],
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        tariff.id.0
    }

    async fn tariff_squads(&self, tariff: i64) -> Vec<Uuid> {
        let mut conn = self.db.store.conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT squad_uuid FROM tariff_squads WHERE tariff_id = $1 ORDER BY squad_uuid",
        )
        .bind(tariff)
        .fetch_all(conn.conn())
        .await
        .unwrap()
    }

    async fn audit_actions(&self, id: i64) -> Vec<String> {
        let mut conn = self.db.store.conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT action FROM audit_log WHERE entity_kind = 'remna_transfer' AND entity_id = $1 ORDER BY id",
        )
        .bind(id)
        .fetch_all(conn.conn())
        .await
        .unwrap()
    }

    async fn row_id(&self, id: Uuid) -> i64 {
        let mut conn = self.db.store.conn().await.unwrap();
        runs::get(&mut conn, id).await.unwrap().id
    }
}

fn rich_user(id: i64, name: &str, client: &Client, squads: &[(Uuid, &str)]) -> RemnaUser {
    let mut user = sample_user(id, name);
    user.short_uuid = format!("keep{id}abcdefgh");
    user.subscription_url = format!("https://sub.example.com/{}", user.short_uuid);
    user.description = Some(remna_description(client.public_id));
    user.vless_uuid = Some(sn_remnawave::Secret::new(
        Uuid::from_u128(0xbeef_0000_0000_4000_8000_0000_0000_0000 + id as u128).to_string(),
    ));
    user.trojan_password = Some(sn_remnawave::Secret::new(format!("keep-trojan-{id:04}")));
    user.ss_password = Some(sn_remnawave::Secret::new(format!("keep-ss-{id:04}")));
    user.traffic_limit_bytes = (50 * GIB) as i64;
    user.traffic_limit_strategy = TrafficStrategy::Month;
    user.hwid_device_limit = Some(3);
    user.expire_at = sn_remnawave::fake::panel_time(Utc::now() + Duration::days(40 + id));
    user.active_internal_squads = squads
        .iter()
        .map(|(uuid, name)| SquadRef {
            uuid: *uuid,
            name: (*name).into(),
        })
        .collect();
    user
}

fn done(end: RunEnd) -> Progress {
    match end {
        RunEnd::Done(progress) => progress,
        other => panic!("перенос не закончился: {other:?}"),
    }
}

#[tokio::test]
async fn перенос_в_новую_remnawave_сохраняет_ссылку_ключи_и_данные() {
    let Some(env) = env().await else { return };
    let old_squad = uuid(1);
    let new_squad = uuid(2);
    env.fake.set_squads(vec![squad(old_squad, "Германия")]);
    let tariff = env.tariff("Стандарт", vec![old_squad]).await;
    let ivan = env
        .client(Some((111, "ivan")), Some("ivan@example.com"))
        .await;
    let olga = env.client(Some((222, "olga")), None).await;
    let mut first = rich_user(1, "ivan_vpn", &ivan, &[(old_squad, "Германия")]);
    first.telegram_id = Some(111);
    first.email = Some("ivan@example.com".into());
    let mut second = rich_user(2, "olga_vpn", &olga, &[(old_squad, "Германия")]);
    second.status = UserStatus::Disabled;
    second.telegram_id = Some(222);
    let ivan_sub = env.linked(&ivan, first.clone()).await;
    let olga_sub = env.linked(&olga, second.clone()).await;

    let fresh = Arc::new(FakeRemnawave::new());
    fresh.set_squads(vec![squad(new_squad, "Германия")]);
    fresh.insert_user(sample_user(1, "stranger"));
    env.switch(&fresh);
    assert!(
        !env.services
            .remna_sync()
            .refresh_subscription(ivan_sub.id)
            .await
            .unwrap(),
        "под прежним номером теперь чужой пользователь — его данные не берём"
    );
    assert_eq!(
        env.subscription(ivan_sub.id).await.remna.unwrap().username,
        "ivan_vpn"
    );

    let checked = env.ops().check_now(Direction::Push, None).await.unwrap();
    let plan = checked.plan.clone().unwrap();
    assert_eq!(checked.state, TransferState::Ready);
    assert_eq!(plan.create.count, 2);
    assert_eq!(plan.keys_kept, 2);
    assert_eq!(plan.keys_new, 0);
    assert_eq!(plan.disabled, 1);
    assert_eq!(plan.link_change.count, 0);
    assert_eq!(plan.squads.len(), 1);
    assert_eq!(plan.squads[0].old, old_squad);
    assert_eq!(plan.squads[0].name.as_deref(), Some("Германия"));
    assert_eq!(plan.squads[0].suggested, Some(new_squad));
    assert_eq!(plan.squads[0].tariffs, vec!["Стандарт".to_string()]);
    assert!(
        fresh.calls_of("create_user").is_empty(),
        "проверка ничего не пишет"
    );

    let progress = done(
        env.ops()
            .start_now(checked.id, StartOptions::default(), None)
            .await
            .unwrap(),
    );
    assert_eq!(progress.created, 2);
    assert_eq!(progress.link_changed, 0);
    let made = fresh
        .users()
        .into_iter()
        .find(|u| u.username == "ivan_vpn")
        .unwrap();
    assert_eq!(made.short_uuid, first.short_uuid);
    assert_eq!(made.subscription_url, first.subscription_url);
    assert_eq!(made.vless_uuid, first.vless_uuid);
    assert_eq!(made.trojan_password, first.trojan_password);
    assert_eq!(made.ss_password, first.ss_password);
    assert_eq!(made.telegram_id, Some(111));
    assert_eq!(made.email.as_deref(), Some("ivan@example.com"));
    assert_eq!(made.hwid_device_limit, Some(3));
    assert_eq!(made.traffic_limit_bytes, (50 * GIB) as i64);
    assert_eq!(made.traffic_limit_strategy, TrafficStrategy::Month);
    assert_eq!(
        made.expire_at.timestamp_millis(),
        first.expire_at.timestamp_millis()
    );
    assert_eq!(
        made.description.as_deref(),
        Some(remna_description(ivan.public_id).as_str())
    );
    assert_eq!(made.squad_ids(), vec![SquadId(new_squad)]);
    let disabled = fresh
        .users()
        .into_iter()
        .find(|u| u.username == "olga_vpn")
        .unwrap();
    assert_eq!(disabled.status, UserStatus::Disabled);
    assert_eq!(disabled.short_uuid, second.short_uuid);

    let linked = env.subscription(ivan_sub.id).await;
    let link = linked.remna.unwrap();
    assert_eq!(link.user_id, made.id);
    assert_eq!(link.short_uuid.as_deref(), Some(first.short_uuid.as_str()));
    assert!(env.subscription(olga_sub.id).await.remna.is_some());
    assert_eq!(fresh.user(RemnaUserId(1)).unwrap().username, "stranger");
    assert_eq!(env.tariff_squads(tariff).await, vec![new_squad]);

    let id = env.row_id(checked.id).await;
    assert_eq!(
        env.audit_actions(id).await,
        vec![AUDIT_STARTED.to_string(), AUDIT_FINISHED.to_string()]
    );
    let alerts = env.services.alerts();
    alerts.collect().await.unwrap();
    let view = env.ops().view().await.unwrap();
    assert_eq!(view.run.unwrap().state, TransferState::Done);
    env.db.cleanup().await;
}

#[tokio::test]
async fn расхождения_видны_в_проверке_и_правятся_только_по_выбору() {
    let Some(env) = env().await else { return };
    let main = uuid(1);
    let spare = uuid(3);
    env.fake
        .set_squads(vec![squad(main, "Основной"), squad(spare, "Запасной")]);
    let ivan = env.client(Some((111, "ivan")), None).await;
    let olga = env.client(Some((222, "olga")), None).await;
    let first = rich_user(1, "ivan_vpn", &ivan, &[(main, "Основной")]);
    let second = rich_user(2, "olga_vpn", &olga, &[(main, "Основной")]);
    env.linked(&ivan, first.clone()).await;
    env.linked(&olga, second.clone()).await;

    let mut patch = UpdateUserRequest::new(first.id);
    patch.expire_at = Some(first.expire_at - Duration::days(10));
    patch.traffic_limit_bytes = Some((10 * GIB) as i64);
    patch.active_internal_squads = Some(vec![spare]);
    patch.status = Some(UserStatus::Disabled);
    env.fake.update_user(&patch).await.unwrap();
    let mut later = UpdateUserRequest::new(second.id);
    later.expire_at = Some(second.expire_at + Duration::days(20));
    env.fake.update_user(&later).await.unwrap();

    let checked = env.ops().check_now(Direction::Push, None).await.unwrap();
    let plan = checked.plan.clone().unwrap();
    assert_eq!(plan.create.count, 0);
    assert_eq!(plan.fix.count, 1);
    assert_eq!(plan.same, 1);
    assert_eq!(plan.later_in_remna, 1);
    for field in ["Срок", "Лимит трафика", "Сквады", "Статус"] {
        assert_eq!(plan.fields.get(field), Some(&1), "{field}");
    }
    let example = &plan.fix.examples[0];
    assert!(example.who.contains("@ivan"), "{}", example.who);
    assert!(
        example
            .details
            .iter()
            .any(|line| line.starts_with("Сквады: в панели Основной, в Remnawave Запасной")),
        "{:?}",
        example.details
    );

    let progress = done(
        env.ops()
            .start_now(checked.id, StartOptions::default(), None)
            .await
            .unwrap(),
    );
    assert_eq!(progress.fixed, 0);
    assert_eq!(progress.differs, 1);
    let untouched = env.fake.user(first.id).unwrap();
    assert_eq!(untouched.traffic_limit_bytes, (10 * GIB) as i64);
    assert_eq!(untouched.status, UserStatus::Disabled);

    let again = env.ops().check_now(Direction::Push, None).await.unwrap();
    let progress = done(
        env.ops()
            .start_now(
                again.id,
                StartOptions {
                    mode: Mode::Fix,
                    ..StartOptions::default()
                },
                None,
            )
            .await
            .unwrap(),
    );
    assert_eq!(progress.fixed, 1);
    let fixed = env.fake.user(first.id).unwrap();
    assert_eq!(
        fixed.expire_at.timestamp_millis(),
        first.expire_at.timestamp_millis()
    );
    assert_eq!(fixed.traffic_limit_bytes, (50 * GIB) as i64);
    assert_eq!(fixed.squad_ids(), vec![SquadId(main)]);
    assert_eq!(fixed.status, UserStatus::Active);
    assert_eq!(
        env.fake
            .user(second.id)
            .unwrap()
            .expire_at
            .timestamp_millis(),
        (second.expire_at + Duration::days(20)).timestamp_millis(),
        "срок в Remnawave позже — не сокращаем"
    );
    let (_, csv) = env.ops().report(again.id).await.unwrap();
    let csv = String::from_utf8(csv).unwrap();
    assert!(csv.contains("исправлен"), "{csv}");
    assert!(csv.contains("срок в Remnawave позже"), "{csv}");
    env.db.cleanup().await;
}

#[tokio::test]
async fn повторный_перенос_не_создаёт_дублей() {
    let Some(env) = env().await else { return };
    let ivan = env.client(Some((111, "ivan")), None).await;
    let olga = env.client(Some((222, "olga")), None).await;
    env.linked(&ivan, rich_user(1, "ivan_vpn", &ivan, &[]))
        .await;
    env.linked(&olga, rich_user(2, "olga_vpn", &olga, &[]))
        .await;
    let fresh = Arc::new(FakeRemnawave::new());
    env.switch(&fresh);
    let first = env.ops().check_now(Direction::Push, None).await.unwrap();
    done(
        env.ops()
            .start_now(first.id, StartOptions::default(), None)
            .await
            .unwrap(),
    );
    assert_eq!(fresh.users().len(), 2);
    let second = env.ops().check_now(Direction::Push, None).await.unwrap();
    let plan = second.plan.clone().unwrap();
    assert_eq!(plan.create.count, 0);
    assert_eq!(plan.same, 2);
    let progress = done(
        env.ops()
            .start_now(second.id, StartOptions::default(), None)
            .await
            .unwrap(),
    );
    assert_eq!(progress.created, 0);
    assert_eq!(progress.same, 2);
    assert_eq!(fresh.users().len(), 2);
    assert_eq!(fresh.calls_of("create_user").len(), 2);
    env.db.cleanup().await;
}

#[tokio::test]
async fn перенос_можно_прервать_и_продолжить() {
    let Some(env) = env().await else { return };
    for n in 1..=5 {
        let client = env.client(Some((100 + n, "user")), None).await;
        env.linked(&client, rich_user(n, &format!("user_{n}"), &client, &[]))
            .await;
    }
    let fresh = Arc::new(FakeRemnawave::new());
    env.switch(&fresh);
    let store = env.db.store.clone();
    let once = Arc::new(AtomicBool::new(false));
    fresh.before_create_response(move |_| {
        let store = store.clone();
        let once = once.clone();
        Box::pin(async move {
            if !once.swap(true, Ordering::SeqCst) {
                sqlx::query(
                    "UPDATE remna_transfers SET pause_requested = true WHERE state = 'running'",
                )
                .execute(store.pool())
                .await
                .unwrap();
            }
        })
    });
    let checked = env.ops().check_now(Direction::Push, None).await.unwrap();
    let paused = env
        .ops()
        .start_now(checked.id, StartOptions::default(), None)
        .await
        .unwrap();
    let RunEnd::Paused(progress) = paused else {
        panic!("перенос не остановился: {paused:?}");
    };
    assert_eq!(progress.created, 2);
    assert_eq!(fresh.users().len(), 2);
    assert_eq!(
        env.ops().get(checked.id).await.unwrap().state,
        TransferState::Paused
    );

    for _ in 0..3 {
        fresh.fail_on("create_user", Fault::Unavailable);
    }
    let stopped = env
        .ops()
        .start_now(checked.id, StartOptions::default(), None)
        .await
        .unwrap();
    let RunEnd::Stopped(progress, reason) = stopped else {
        panic!("перенос не остановился на сбое: {stopped:?}");
    };
    assert!(reason.contains("Remnawave не отвечает"), "{reason}");
    assert_eq!(progress.created, 2);
    let failed = env.ops().get(checked.id).await.unwrap();
    assert_eq!(failed.state, TransferState::Failed);
    assert!(failed.error.unwrap().contains("дублей не будет"));

    let progress = done(
        env.ops()
            .start_now(checked.id, StartOptions::default(), None)
            .await
            .unwrap(),
    );
    assert_eq!(progress.created, 5);
    assert_eq!(fresh.users().len(), 5);
    let mut names: Vec<String> = fresh.users().into_iter().map(|u| u.username).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 5);
    env.db.cleanup().await;
}

#[tokio::test]
async fn подписка_без_ссылки_получает_новую_и_её_можно_разослать() {
    let Some(env) = env().await else { return };
    let ivan = env.client(Some((111, "ivan")), None).await;
    let subscription = env
        .linked(&ivan, rich_user(1, "ivan_vpn", &ivan, &[]))
        .await;
    let fresh = Arc::new(FakeRemnawave::new());
    env.switch(&fresh);
    let mut conn = env.db.store.conn().await.unwrap();
    sqlx::query("UPDATE subscriptions SET short_uuid = NULL WHERE id = $1")
        .bind(subscription.id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    let checked = env.ops().check_now(Direction::Push, None).await.unwrap();
    assert_eq!(checked.plan.unwrap().link_change.count, 1);
    let progress = done(
        env.ops()
            .start_now(checked.id, StartOptions::default(), None)
            .await
            .unwrap(),
    );
    assert_eq!(progress.link_changed, 1);
    let view = env.ops().get(checked.id).await.unwrap();
    assert_eq!(view.link_changed, 1);
    assert_eq!(env.ops().notify_links(checked.id, None).await.unwrap(), 1);
    assert!(env.ops().notify_links(checked.id, None).await.is_err());
    let mut conn = env.db.store.conn().await.unwrap();
    let (client, template, url): (i64, String, String) = sqlx::query_as(
        "SELECT (payload->>'client_id')::bigint, payload->>'template', payload->'params'->>'url' \
         FROM jobs WHERE kind = 'notify' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(client, ivan.id.0);
    assert_eq!(template, LINK_TEMPLATE);
    let made = fresh
        .users()
        .into_iter()
        .find(|u| u.username == "ivan_vpn")
        .unwrap();
    assert_eq!(url, made.subscription_url);
    env.db.cleanup().await;
}

#[tokio::test]
async fn забор_находит_клиентов_по_telegram_почте_имени_и_отметке() {
    let Some(env) = env().await else { return };
    let by_tg = env.client(Some((111, "ivan")), None).await;
    let by_mail = env.client(None, Some("olga@example.com")).await;
    let by_name = env.client(Some((333, "Petya")), None).await;
    let by_marker = env.client(None, None).await;
    let linked = env.client(Some((444, "linked")), None).await;
    env.linked(&linked, sample_user(50, "already_here")).await;

    let mut tg_user = sample_user(1, "random_one");
    tg_user.telegram_id = Some(111);
    let mut mail_user = sample_user(2, "random_two");
    mail_user.email = Some("Olga@Example.com".into());
    let name_user = sample_user(3, "petya");
    let mut marker_user = sample_user(4, "random_four");
    marker_user.description = Some(remna_description(by_marker.public_id));
    let mut new_user = sample_user(5, "brand_new");
    new_user.telegram_id = Some(555);
    for user in [&tg_user, &mail_user, &name_user, &marker_user, &new_user] {
        env.fake.insert_user((*user).clone());
    }

    let checked = env.ops().check_now(Direction::Pull, None).await.unwrap();
    let plan = checked.plan.clone().unwrap();
    assert_eq!(plan.remna_users, 6);
    assert_eq!(plan.same, 1);
    assert_eq!(plan.attach.count, 4);
    assert_eq!(plan.create.count, 1);
    for by in ["telegram", "email", "name", "marker"] {
        assert_eq!(plan.matched_by.get(by), Some(&1), "{by}");
    }

    let progress = done(
        env.ops()
            .start_now(checked.id, StartOptions::default(), None)
            .await
            .unwrap(),
    );
    assert_eq!(progress.attached, 5);
    assert_eq!(progress.clients, 1);
    let mut conn = env.db.store.conn().await.unwrap();
    for (user, client) in [
        (&tg_user, by_tg.id),
        (&mail_user, by_mail.id),
        (&name_user, by_name.id),
        (&marker_user, by_marker.id),
    ] {
        let sub = conn
            .subscriptions()
            .by_remna_user(user.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sub.client_id, client, "{}", user.username);
        assert_eq!(
            sub.remna.unwrap().short_uuid.as_deref(),
            Some(user.short_uuid.as_str())
        );
        assert!(sub.mirror.expire_at.is_some());
    }
    let created = conn
        .clients()
        .find_by_identity(IdentityKind::Telegram, "555")
        .await
        .unwrap()
        .unwrap();
    let sub = conn
        .subscriptions()
        .by_remna_user(new_user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sub.client_id, created.id);
    assert_eq!(created.registration_source, RegistrationSource::Import);
    drop(conn);

    let again = env.ops().check_now(Direction::Pull, None).await.unwrap();
    let plan = again.plan.unwrap();
    assert_eq!(plan.same, 6);
    assert_eq!(plan.attach.count + plan.create.count, 0);
    env.db.cleanup().await;
}

#[tokio::test]
async fn забор_подбирает_тариф_по_сквадам_и_лимитам() {
    let Some(env) = env().await else { return };
    let main = uuid(1);
    env.fake.set_squads(vec![squad(main, "Основной")]);
    let tariff = env.tariff("Стандарт", vec![main]).await;
    let mut fits = sample_user(1, "fits");
    fits.active_internal_squads = vec![SquadRef {
        uuid: main,
        name: "Основной".into(),
    }];
    fits.traffic_limit_bytes = (100 * GIB) as i64;
    fits.traffic_limit_strategy = TrafficStrategy::Month;
    fits.hwid_device_limit = Some(5);
    let mut too_many = fits.clone();
    too_many.id = RemnaUserId(2);
    too_many.username = "too_many".into();
    too_many.short_uuid = "short2".into();
    too_many.hwid_device_limit = Some(9);
    let mut trial = fits.clone();
    trial.id = RemnaUserId(3);
    trial.username = "trial_user".into();
    trial.short_uuid = "short3".into();
    trial.hwid_device_limit = Some(3);
    trial.tag = Some(sn_domain::subscription::TRIAL_TAG.into());
    for user in [&fits, &too_many, &trial] {
        env.fake.insert_user((*user).clone());
    }
    let checked = env.ops().check_now(Direction::Pull, None).await.unwrap();
    let plan = checked.plan.clone().unwrap();
    assert_eq!(plan.tariff_matched, 2);
    assert_eq!(plan.tariff_none, 1);
    done(
        env.ops()
            .start_now(checked.id, StartOptions::default(), None)
            .await
            .unwrap(),
    );
    let mut conn = env.db.store.conn().await.unwrap();
    let matched = conn
        .subscriptions()
        .by_remna_user(fits.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(matched.tariff_id.map(|t| t.0), Some(tariff));
    assert_eq!(matched.extra_devices, 2);
    assert_eq!(matched.kind, SubscriptionKind::Paid);
    let none = conn
        .subscriptions()
        .by_remna_user(too_many.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(none.tariff_id, None);
    assert_eq!(none.kind, SubscriptionKind::Manual);
    let trial_sub = conn
        .subscriptions()
        .by_remna_user(trial.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(trial_sub.kind, SubscriptionKind::Trial);
    assert_eq!(trial_sub.tariff_id.map(|t| t.0), Some(tariff));
    drop(conn);
    let (_, csv) = env.ops().report(checked.id).await.unwrap();
    let csv = String::from_utf8(csv).unwrap();
    assert!(csv.contains("без тарифа"), "{csv}");
    assert!(csv.contains("too_many"), "{csv}");
    env.db.cleanup().await;
}

#[tokio::test]
async fn вторая_проверка_не_начинается_пока_идёт_перенос() {
    let Some(env) = env().await else { return };
    let mut conn = env.db.store.conn().await.unwrap();
    runs::create(
        &mut conn,
        Direction::Push,
        TransferState::Running,
        None,
        None,
    )
    .await
    .unwrap();
    drop(conn);
    let err = env
        .ops()
        .check_now(Direction::Pull, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "remna_sync.busy");
    env.services.remna_transfer().recover().await.unwrap();
    let view = env.ops().view().await.unwrap();
    assert_eq!(view.run.unwrap().state, TransferState::Interrupted);
    assert!(env.ops().check_now(Direction::Pull, None).await.is_ok());
    env.db.cleanup().await;
}

#[tokio::test]
async fn одноимённый_чужой_пользователь_не_привязывается() {
    let Some(env) = env().await else { return };
    let ivan = env.client(Some((111, "ivan")), None).await;
    let subscription = env.linked(&ivan, rich_user(1, "ivan", &ivan, &[])).await;
    let mut conn = env.db.store.conn().await.unwrap();
    sqlx::query("UPDATE subscriptions SET short_uuid = NULL, remna_user_id = NULL WHERE id = $1")
        .bind(subscription.id.0)
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    let fresh = Arc::new(FakeRemnawave::new());
    let mut namesake = sample_user(5, "ivan");
    namesake.description = Some("чужой бот".into());
    fresh.insert_user(namesake);
    env.switch(&fresh);

    let pull = env.ops().check_now(Direction::Pull, None).await.unwrap();
    let plan = pull.plan.unwrap();
    assert_eq!(plan.relink.count, 0);
    assert_eq!(plan.create.count + plan.attach.count, 0);
    assert!(
        plan.skipped.iter().any(|r| r.code == "name_unverified"),
        "{:?}",
        plan.skipped
    );
    env.ops().discard(pull.id).await.unwrap();

    let push = env.ops().check_now(Direction::Push, None).await.unwrap();
    let plan = push.plan.unwrap();
    assert_eq!(plan.create.count, 0);
    assert!(
        plan.skipped.iter().any(|r| r.code == "name_taken"),
        "{:?}",
        plan.skipped
    );
    assert!(env.subscription(subscription.id).await.remna.is_none());
    env.db.cleanup().await;
}
