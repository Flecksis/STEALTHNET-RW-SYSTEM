use super::*;
use crate::subscriptions::tests::{ADMIN, Env, env};
use chrono::Duration;
use sn_domain::ids::{SquadId, TariffId};
use sn_domain::settings::{MultiSubscriptions, keys};
use sn_domain::subscription::AutoRenewState;
use sn_domain::tariff::CurrencyPrices;
use sn_domain::tariff::{DevicePolicy, ExtraDevices};
use sn_domain::traffic::{GIB, TrafficPolicy, TrafficReset};
use sn_remnawave::fake::{Fault, panel_time, sample_user};
use sn_remnawave::{SquadRef, UserStatus};
use sn_store::{NewCategory, NewPeriod, NewSubscription, NewTariff};
use std::sync::Arc;
use std::time::Duration as StdDuration;

fn request(user: &RemnaUser) -> LinkRequest {
    LinkRequest {
        user_id: user.id,
        username: user.username.clone(),
    }
}

fn code(err: &ServiceError) -> &'static str {
    err.code()
}

async fn unlinked(env: &Env, client: ClientId) -> Subscription {
    let mut tx = env.db.store.begin().await.unwrap();
    let subscription = tx
        .subscriptions()
        .create(&NewSubscription {
            client_id: client,
            tariff_id: None,
            kind: SubscriptionKind::Paid,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    subscription
}

async fn missing(env: &Env, id: SubscriptionId) -> Option<DateTime<Utc>> {
    let mut conn = env.db.store.conn().await.unwrap();
    conn.subscriptions().missing_since(id).await.unwrap()
}

async fn snapshot_name(env: &Env, id: SubscriptionId) -> Option<String> {
    let mut conn = env.db.store.conn().await.unwrap();
    sqlx::query_scalar("SELECT username FROM remna_snapshots WHERE subscription_id = $1")
        .bind(id.0)
        .fetch_optional(conn.conn())
        .await
        .unwrap()
}

async fn sync_jobs(env: &Env) -> Vec<SubscriptionId> {
    let mut conn = env.db.store.conn().await.unwrap();
    conn.jobs()
        .claim(50, StdDuration::from_secs(30))
        .await
        .unwrap()
        .into_iter()
        .filter_map(|job| match job.typed().ok()? {
            JobPayload::SyncSubscription { subscription_id } => Some(subscription_id),
            _ => None,
        })
        .collect()
}

#[test]
fn запрос_поиска_разбирается_по_виду() {
    assert_eq!(
        lookups("https://sub.example.com/abc123/clash?x=1#name"),
        vec![
            Lookup::Short("clash".into()),
            Lookup::Short("abc123".into())
        ]
    );
    assert_eq!(
        lookups("sub.example.com/abc123"),
        vec![Lookup::Short("abc123".into())]
    );
    assert_eq!(
        lookups(" @john_doe "),
        vec![Lookup::Username("john_doe".into())]
    );
    assert_eq!(
        lookups("Mail@Example.com"),
        vec![
            Lookup::Email("Mail@Example.com".into()),
            Lookup::Email("mail@example.com".into())
        ]
    );
    assert_eq!(
        lookups("123456789"),
        vec![
            Lookup::Telegram(123_456_789),
            Lookup::Username("123456789".into()),
            Lookup::Short("123456789".into())
        ]
    );
    assert!(lookups("ab").is_empty());
    assert!(lookups("два слова").is_empty());
    assert!(lookups("   ").is_empty());
}

#[tokio::test]
async fn поиск_находит_по_имени_ссылке_telegram_и_почте() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let subscription = unlinked(&env, client.id).await;
    let mut alice = sample_user(11, "alice_vpn");
    alice.telegram_id = Some(5_551_111);
    alice.email = Some("alice@example.com".into());
    alice.active_internal_squads = vec![SquadRef {
        uuid: Uuid::from_u128(7),
        name: "Европа".into(),
    }];
    alice.hwid_device_limit = Some(3);
    alice.traffic_limit_bytes = 50 * 1024 * 1024 * 1024;
    env.fake.insert_user(alice.clone());
    let mut second = sample_user(12, "alice_phone");
    second.telegram_id = Some(5_551_111);
    env.fake.insert_user(second);
    env.fake.insert_user(sample_user(13, "stranger"));
    let links = env.services.remna_links();

    let by_name = links.search(subscription.id, "alice_vpn").await.unwrap();
    assert_eq!(by_name.state, LinkState::Unlinked);
    assert_eq!(by_name.users.len(), 1);
    let found = &by_name.users[0];
    assert_eq!(found.id, RemnaUserId(11));
    assert_eq!(found.found_by, vec![FoundBy::Username]);
    assert_eq!(found.terms.squads, vec!["Европа".to_string()]);
    assert_eq!(found.terms.hwid_limit, Some(3));
    assert_eq!(
        found.terms.traffic_limit_bytes,
        Some(50 * 1024 * 1024 * 1024)
    );
    assert_eq!(found.holder, None);

    let by_link = links
        .search(subscription.id, "https://sub.example.com/short11")
        .await
        .unwrap();
    assert_eq!(
        by_link
            .users
            .iter()
            .map(|u| (u.id, u.found_by.clone()))
            .collect::<Vec<_>>(),
        vec![(RemnaUserId(11), vec![FoundBy::ShortLink])]
    );

    let by_telegram = links.search(subscription.id, "5551111").await.unwrap();
    let mut ids: Vec<i64> = by_telegram.users.iter().map(|u| u.id.0).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![11, 12]);
    assert!(
        by_telegram
            .users
            .iter()
            .all(|u| u.found_by == vec![FoundBy::Telegram])
    );

    let by_mail = links
        .search(subscription.id, "Alice@Example.com")
        .await
        .unwrap();
    assert_eq!(
        by_mail.users.iter().map(|u| u.id).collect::<Vec<_>>(),
        vec![RemnaUserId(11)]
    );
    assert_eq!(by_mail.users[0].found_by, vec![FoundBy::Email]);

    let none = links.search(subscription.id, "nobody_here").await.unwrap();
    assert!(none.users.is_empty());
    let err = links.search(subscription.id, "  ").await.unwrap_err();
    assert_eq!(code(&err), QUERY);
    env.finish().await;
}

#[tokio::test]
async fn привязка_пропавшей_подписки_обновляет_данные_снимок_и_журнал() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let mut old = sample_user(21, "sn_old");
    old.updated_at = Utc::now();
    let subscription = env.linked(client.id, old).await;
    env.fake.delete_user(RemnaUserId(21)).await.unwrap();
    assert!(
        !env.services
            .remna_sync()
            .refresh_subscription(subscription.id)
            .await
            .unwrap()
    );
    assert!(missing(&env, subscription.id).await.is_some());

    let mut fresh = sample_user(22, "sn_new");
    fresh.expire_at = panel_time(Utc::now() + Duration::days(200));
    fresh.updated_at = Utc::now() - Duration::days(40);
    fresh.hwid_device_limit = Some(5);
    env.fake.insert_user(fresh.clone());
    let links = env.services.remna_links();
    let found = links.search(subscription.id, "sn_new").await.unwrap();
    assert_eq!(found.state, LinkState::Missing);
    assert_eq!(
        found.current.as_ref().map(|l| l.username.as_str()),
        Some("sn_old")
    );
    assert!(found.missing_since.is_some());
    assert_eq!(found.panel.expire_at, subscription.mirror.expire_at);

    let linked = links
        .link(subscription.id, &request(&fresh), ADMIN)
        .await
        .unwrap();
    assert!(linked.refreshed);
    assert!(linked.released.is_empty());
    let stored = env.subscription(subscription.id).await;
    let link = stored.remna.clone().unwrap();
    assert_eq!(link.user_id, RemnaUserId(22));
    assert_eq!(link.username, "sn_new");
    assert_eq!(link.short_uuid.as_deref(), Some("short22"));
    assert_eq!(stored.mirror.expire_at, Some(fresh.expire_at));
    assert_eq!(stored.mirror.hwid_limit, Some(5));
    assert_eq!(missing(&env, subscription.id).await, None);
    assert_eq!(
        snapshot_name(&env, subscription.id).await.as_deref(),
        Some("sn_new")
    );
    let audit = env.audit("subscription", subscription.id.0).await;
    assert_eq!(audit[0].action, REMNA_LINKED);
    assert_eq!(audit[0].actor_kind, "admin");
    assert_eq!(audit[0].details["remna_username"], "sn_new");
    assert_eq!(audit[0].details["previous"]["username"], "sn_old");
    assert!(audit[0].details["missing_since"].is_string());

    let again = links
        .link(subscription.id, &request(&fresh), ADMIN)
        .await
        .unwrap();
    assert!(!again.refreshed);
    assert_eq!(env.audit("subscription", subscription.id.0).await.len(), 1);
    env.finish().await;
}

#[tokio::test]
async fn занятого_пользователя_не_привязывает_и_показывает_подписку() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let other = env.client().await;
    let subscription = unlinked(&env, client.id).await;
    let holder = env.linked(other.id, sample_user(31, "sn_taken")).await;
    let own = env.linked(client.id, sample_user(32, "sn_own")).await;
    let links = env.services.remna_links();

    let found = links.search(subscription.id, "sn_taken").await.unwrap();
    let candidate = &found.users[0];
    let shown = candidate.holder.clone().unwrap();
    assert_eq!(shown.subscription_id, holder.id);
    assert_eq!(shown.client.client_id, other.id);
    assert!(!shown.same_client);

    let err = links
        .link(
            subscription.id,
            &request(&env.fake.user(RemnaUserId(31)).unwrap()),
            ADMIN,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&err), TAKEN);
    assert!(err.to_string().contains(&format!("№{}", holder.id.0)));
    assert!(err.to_string().contains(&format!("№{}", other.id.0)));

    let same = links.search(subscription.id, "short32").await.unwrap();
    assert!(same.users[0].holder.as_ref().unwrap().same_client);
    let err = links
        .link(
            subscription.id,
            &request(&env.fake.user(RemnaUserId(32)).unwrap()),
            ADMIN,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&err), TAKEN);
    assert!(err.to_string().contains("этого клиента"));

    assert!(env.subscription(subscription.id).await.remna.is_none());
    assert_eq!(
        env.subscription(holder.id).await.remna.map(|l| l.user_id),
        Some(RemnaUserId(31))
    );
    assert_eq!(
        env.subscription(own.id).await.remna.map(|l| l.user_id),
        Some(RemnaUserId(32))
    );
    assert!(
        env.audit("subscription", subscription.id.0)
            .await
            .is_empty()
    );
    env.finish().await;
}

#[tokio::test]
async fn привязка_снимает_устаревший_номер_и_завершённую_подписку() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let other = env.client().await;
    let stale = env.linked(other.id, sample_user(41, "sn_before")).await;
    let mut reused = sample_user(41, "sn_after");
    reused.short_uuid = "short41b".into();
    env.fake.insert_user(reused.clone());
    let ended = env.linked(other.id, sample_user(42, "sn_ended")).await;
    {
        let mut tx = env.db.store.begin().await.unwrap();
        tx.subscriptions().end(ended.id, Utc::now()).await.unwrap();
        tx.commit().await.unwrap();
    }
    let first = unlinked(&env, client.id).await;
    let second = unlinked(&env, client.id).await;
    let links = env.services.remna_links();

    let found = links.search(first.id, "sn_after").await.unwrap();
    assert_eq!(found.users[0].holder, None);
    let linked = links
        .link(first.id, &request(&reused), ADMIN)
        .await
        .unwrap();
    assert_eq!(linked.released, vec![stale.id]);
    let before = env.subscription(stale.id).await;
    assert!(before.remna.is_none());
    assert!(missing(&env, stale.id).await.is_some());
    assert_eq!(
        env.subscription(first.id).await.remna.map(|l| l.user_id),
        Some(RemnaUserId(41))
    );

    let user = env.fake.user(RemnaUserId(42)).unwrap();
    let linked = links.link(second.id, &request(&user), ADMIN).await.unwrap();
    assert_eq!(linked.released, vec![ended.id]);
    assert!(env.subscription(ended.id).await.remna.is_none());
    assert_eq!(
        env.subscription(second.id).await.remna.map(|l| l.username),
        Some("sn_ended".to_string())
    );
    let audit = env.audit("subscription", second.id.0).await;
    assert_eq!(
        audit[0].details["released"][0]["subscription_id"],
        ended.id.0
    );
    assert_eq!(audit[0].details["released"][0]["ended"], true);
    env.finish().await;
}

#[tokio::test]
async fn работающую_подписку_не_перепривязывает() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let subscription = env.linked(client.id, sample_user(51, "sn_works")).await;
    env.fake.insert_user(sample_user(52, "sn_other"));
    let links = env.services.remna_links();
    let found = links.search(subscription.id, "sn_other").await.unwrap();
    assert_eq!(found.state, LinkState::Linked);
    let err = links
        .link(
            subscription.id,
            &request(&env.fake.user(RemnaUserId(52)).unwrap()),
            ADMIN,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&err), WORKING);
    assert_eq!(
        env.subscription(subscription.id)
            .await
            .remna
            .map(|l| l.user_id),
        Some(RemnaUserId(51))
    );

    env.fake.delete_user(RemnaUserId(51)).await.unwrap();
    let linked = links
        .link(
            subscription.id,
            &request(&env.fake.user(RemnaUserId(52)).unwrap()),
            ADMIN,
        )
        .await
        .unwrap();
    assert!(linked.refreshed);
    assert_eq!(
        env.subscription(subscription.id)
            .await
            .remna
            .map(|l| l.user_id),
        Some(RemnaUserId(52))
    );
    env.finish().await;
}

#[tokio::test]
async fn сменившегося_или_удалённого_пользователя_не_привязывает() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let subscription = unlinked(&env, client.id).await;
    let user = sample_user(61, "sn_seen");
    env.fake.insert_user(user.clone());
    let mut renamed = user.clone();
    renamed.username = "sn_renamed".into();
    env.fake.insert_user(renamed);
    let links = env.services.remna_links();
    let err = links
        .link(subscription.id, &request(&user), ADMIN)
        .await
        .unwrap_err();
    assert_eq!(code(&err), USER_CHANGED);
    env.fake.delete_user(RemnaUserId(61)).await.unwrap();
    let err = links
        .link(subscription.id, &request(&user), ADMIN)
        .await
        .unwrap_err();
    assert_eq!(code(&err), USER_GONE);
    let err = links
        .link(subscription.id, &request(&user), Actor::Client(client.id))
        .await
        .unwrap_err();
    assert_eq!(code(&err), "auth.forbidden");
    assert!(env.subscription(subscription.id).await.remna.is_none());
    env.finish().await;
}

#[tokio::test]
async fn заблокированному_клиенту_и_завершённой_подписке_нельзя() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let subscription = unlinked(&env, client.id).await;
    let user = sample_user(71, "sn_blocked");
    env.fake.insert_user(user.clone());
    {
        let mut conn = env.db.store.conn().await.unwrap();
        sqlx::query("UPDATE clients SET status = 'blocked', blocked_at = now() WHERE id = $1")
            .bind(client.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let links = env.services.remna_links();
    let err = links
        .search(subscription.id, "sn_blocked")
        .await
        .unwrap_err();
    assert_eq!(code(&err), BLOCKED);
    let err = links
        .link(subscription.id, &request(&user), ADMIN)
        .await
        .unwrap_err();
    assert_eq!(code(&err), BLOCKED);

    let other = env.client().await;
    let ended = unlinked(&env, other.id).await;
    {
        let mut tx = env.db.store.begin().await.unwrap();
        tx.subscriptions().end(ended.id, Utc::now()).await.unwrap();
        tx.commit().await.unwrap();
    }
    let err = links
        .link(ended.id, &request(&user), ADMIN)
        .await
        .unwrap_err();
    assert_eq!(code(&err), ENDED);
    assert!(env.subscription(ended.id).await.remna.is_none());
    env.finish().await;
}

#[tokio::test]
async fn без_ответа_remnawave_после_привязки_ставит_сверку() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let subscription = unlinked(&env, client.id).await;
    let mut user = sample_user(81, "sn_later");
    user.status = UserStatus::Disabled;
    env.fake.insert_user(user.clone());
    env.fake.fail_on("user_devices", Fault::Unavailable);
    let linked = env
        .services
        .remna_links()
        .link(subscription.id, &request(&user), ADMIN)
        .await
        .unwrap();
    assert!(!linked.refreshed);
    assert_eq!(
        env.subscription(subscription.id)
            .await
            .remna
            .map(|l| l.user_id),
        Some(RemnaUserId(81))
    );
    assert_eq!(sync_jobs(&env).await, vec![subscription.id]);
    assert!(
        env.services
            .remna_sync()
            .refresh_subscription(subscription.id)
            .await
            .unwrap()
    );
    assert_eq!(
        env.subscription(subscription.id).await.mirror.status,
        Some(SubscriptionStatus::Disabled)
    );
    env.finish().await;
}

#[tokio::test]
async fn карточка_подписки_помечает_чужого_пользователя_под_номером() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let subscription = env.linked(client.id, sample_user(91, "sn_mine")).await;
    let view = env
        .services
        .subscriptions()
        .admin_view(subscription.id, ADMIN)
        .await
        .unwrap();
    assert_eq!(view.remna_missing_at, None);
    let mut someone = sample_user(91, "sn_someone");
    someone.short_uuid = "short91b".into();
    someone.active_internal_squads = vec![SquadRef {
        uuid: Uuid::from_u128(9),
        name: "Чужой сквад".into(),
    }];
    env.fake.insert_user(someone);
    let view = env
        .services
        .subscriptions()
        .admin_view(subscription.id, ADMIN)
        .await
        .unwrap();
    assert!(view.remna_missing_at.is_some());
    assert!(view.squads.is_empty());
    assert_eq!(view.view.subscription_url, None);
    let stored = env.subscription(subscription.id).await;
    assert_eq!(
        stored.remna.map(|l| l.username),
        Some("sn_mine".to_string())
    );

    let mut back = sample_user(91, "sn_mine");
    back.updated_at = Utc::now() + Duration::minutes(1);
    env.fake.insert_user(back);
    let view = env
        .services
        .subscriptions()
        .admin_view(subscription.id, ADMIN)
        .await
        .unwrap();
    assert_eq!(view.remna_missing_at, None);
    env.finish().await;
}

async fn tariff(env: &Env, name: &str, squad: Uuid) -> i64 {
    let mut tx = env.db.store.begin().await.unwrap();
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
            squads: vec![SquadId(squad)],
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

fn fitting(id: i64, name: &str, squad: Uuid) -> RemnaUser {
    let mut user = sample_user(id, name);
    user.active_internal_squads = vec![SquadRef {
        uuid: squad,
        name: "Европа".into(),
    }];
    user.traffic_limit_bytes = i64::try_from(100 * GIB).unwrap();
    user.traffic_limit_strategy = TrafficStrategy::Month;
    user.hwid_device_limit = Some(4);
    user
}

async fn set_multi(env: &Env, enabled: bool) {
    {
        let mut conn = env.db.store.conn().await.unwrap();
        conn.settings()
            .set_as(
                keys::MULTI_SUBSCRIPTIONS,
                &MultiSubscriptions { enabled },
                None,
            )
            .await
            .unwrap();
    }
    env.services.reload().await.unwrap();
    let api: Arc<dyn RemnawaveApi> = env.fake.clone();
    env.services.set_remna(Some(api));
}

#[tokio::test]
async fn добавление_создаёт_подписку_и_подбирает_тариф_как_забор() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let squad = Uuid::from_u128(5);
    let standard = tariff(&env, "Стандарт", squad).await;
    let user = fitting(101, "sn_from_remna", squad);
    env.fake.insert_user(user.clone());
    let links = env.services.remna_links();

    let found = links.add_search(client.id, "sn_from_remna").await.unwrap();
    assert_eq!(found.users.len(), 1);
    assert_eq!(
        found.users[0].tariff,
        Some(TariffPick::One {
            id: standard,
            name: "Стандарт".into(),
            extra_devices: 1
        })
    );
    assert_eq!(found.users[0].holder, None);

    let added = links.add(client.id, &request(&user), ADMIN).await.unwrap();
    assert_eq!(added.tariff.as_deref(), Some("Стандарт"));
    assert!(added.refreshed);
    let stored = env.subscription(added.subscription_id).await;
    assert_eq!(stored.client_id, client.id);
    assert_eq!(stored.tariff_id, Some(TariffId(standard)));
    assert_eq!(stored.kind, SubscriptionKind::Paid);
    assert_eq!(stored.extra_devices, 1);
    assert!(!stored.autorenew.enabled);
    assert_eq!(
        stored
            .remna
            .as_ref()
            .map(|l| (l.user_id, l.short_uuid.clone())),
        Some((RemnaUserId(101), Some("short101".to_string())))
    );
    assert_eq!(stored.mirror.expire_at, Some(user.expire_at));
    assert_eq!(stored.mirror.hwid_limit, Some(4));
    assert_eq!(
        snapshot_name(&env, added.subscription_id).await.as_deref(),
        Some("sn_from_remna")
    );
    let audit = env.audit("subscription", added.subscription_id.0).await;
    assert_eq!(audit[0].action, REMNA_ADDED);
    assert_eq!(audit[0].details["client_id"], client.id.0);
    assert_eq!(audit[0].details["tariff"]["kind"], "one");
    assert_eq!(audit[0].details["kind"], "paid");

    let again = links.add_search(client.id, "short101").await;
    assert_eq!(code(&again.unwrap_err()), SINGLE);
    let other = env.client().await;
    let shown = links.add_search(other.id, "sn_from_remna").await.unwrap();
    let holder = shown.users[0].holder.clone().unwrap();
    assert_eq!(holder.subscription_id, added.subscription_id);
    assert!(!holder.same_client);
    let err = links
        .add(other.id, &request(&user), ADMIN)
        .await
        .unwrap_err();
    assert_eq!(code(&err), TAKEN);
    assert!(
        err.to_string()
            .contains(&format!("№{}", added.subscription_id.0))
    );
    env.finish().await;
}

#[tokio::test]
async fn без_подходящего_тарифа_подписка_без_тарифа_а_метка_trial_даёт_пробную() {
    let Some(env) = env().await else { return };
    let squad = Uuid::from_u128(6);
    tariff(&env, "Первый", squad).await;
    tariff(&env, "Второй", squad).await;
    let links = env.services.remna_links();

    let twice = fitting(121, "sn_twice", squad);
    env.fake.insert_user(twice.clone());
    let first = env.client().await;
    let found = links.add_search(first.id, "sn_twice").await.unwrap();
    match &found.users[0].tariff {
        Some(TariffPick::Many { names }) => {
            let mut names = names.clone();
            names.sort();
            assert_eq!(names, vec!["Второй".to_string(), "Первый".to_string()]);
        }
        other => panic!("ожидались несколько тарифов, а не {other:?}"),
    }
    let added = links.add(first.id, &request(&twice), ADMIN).await.unwrap();
    assert_eq!(added.tariff, None);
    let stored = env.subscription(added.subscription_id).await;
    assert_eq!(stored.tariff_id, None);
    assert_eq!(stored.kind, SubscriptionKind::Manual);

    let mut trial = sample_user(122, "sn_trial");
    trial.tag = Some(sn_domain::subscription::TRIAL_TAG.into());
    env.fake.insert_user(trial.clone());
    let second = env.client().await;
    let found = links.add_search(second.id, "sn_trial").await.unwrap();
    assert_eq!(found.users[0].tariff, Some(TariffPick::None));
    let added = links.add(second.id, &request(&trial), ADMIN).await.unwrap();
    assert_eq!(
        env.subscription(added.subscription_id).await.kind,
        SubscriptionKind::Trial
    );
    env.finish().await;
}

#[tokio::test]
async fn одна_подписка_на_клиента_не_даёт_добавить_вторую() {
    let Some(env) = env().await else { return };
    let client = env.client().await;
    let existing = unlinked(&env, client.id).await;
    let user = sample_user(131, "sn_second");
    env.fake.insert_user(user.clone());
    let links = env.services.remna_links();
    let err = links.add_search(client.id, "sn_second").await.unwrap_err();
    assert_eq!(code(&err), SINGLE);
    assert!(
        err.to_string()
            .contains("Привязать к пользователю Remnawave")
    );
    let err = links
        .add(client.id, &request(&user), ADMIN)
        .await
        .unwrap_err();
    assert_eq!(code(&err), SINGLE);

    set_multi(&env, true).await;
    let added = links.add(client.id, &request(&user), ADMIN).await.unwrap();
    assert_ne!(added.subscription_id, existing.id);
    let mut conn = env.db.store.conn().await.unwrap();
    let live = conn
        .subscriptions()
        .by_client(client.id)
        .await
        .unwrap()
        .into_iter()
        .filter(|s| s.ended_at.is_none())
        .count();
    assert_eq!(live, 2);
    drop(conn);

    let blocked = env.client().await;
    {
        let mut conn = env.db.store.conn().await.unwrap();
        sqlx::query("UPDATE clients SET status = 'blocked', blocked_at = now() WHERE id = $1")
            .bind(blocked.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    env.fake.insert_user(sample_user(132, "sn_blocked_add"));
    let err = links
        .add_search(blocked.id, "sn_blocked_add")
        .await
        .unwrap_err();
    assert_eq!(code(&err), BLOCKED);
    let err = links
        .add(
            blocked.id,
            &request(&env.fake.user(RemnaUserId(132)).unwrap()),
            Actor::Client(blocked.id),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&err), "auth.forbidden");
    env.finish().await;
}
