use super::signals::{self, Signal};
use super::*;
use crate::catalog::testing::{TestCore, client, context};
use sn_domain::money::Currency;
use sn_store::testing::test_store;

#[test]
fn признаки_складываются_в_подозрение() {
    assert!(!signals::suspicious(&[Signal::YoungAccount], false));
    assert!(!signals::suspicious(&[Signal::NoUsername], false));
    assert!(signals::suspicious(
        &[Signal::YoungAccount, Signal::NoUsername],
        false
    ));
    assert!(!signals::suspicious(
        &[Signal::YoungAccount, Signal::NoUsername, Signal::Burst],
        true
    ));
    assert!(signals::suspicious(&[Signal::DisposableEmail], true));
    assert!(signals::suspicious(&[Signal::IpLimit], false));
    let mut merged = vec![Signal::NoUsername];
    signals::merge(&mut merged, &[Signal::YoungAccount, Signal::NoUsername]);
    assert_eq!(merged, [Signal::YoungAccount, Signal::NoUsername]);
    assert_eq!(
        signals::titles(&merged),
        "новый аккаунт Telegram, нет @username"
    );
}

#[test]
fn одноразовые_домены_из_списка_и_свои() {
    let extra = signals::parse_domains(" @Spam.Example , junk.test;bad domain, x");
    assert_eq!(extra, ["junk.test", "spam.example"]);
    assert!(signals::disposable("bot@mailinator.com", &[]));
    assert!(signals::disposable("bot@inbox.mailinator.com", &[]));
    assert!(!signals::disposable("bot@notmailinator.com", &[]));
    assert!(signals::disposable("me@spam.example", &extra));
    assert!(!signals::disposable("me@gmail.com", &extra));
    assert_eq!(
        signals::email_domain(" User@Example.ORG. ").as_deref(),
        Some("example.org")
    );
    assert_eq!(signals::email_domain("нет-собаки"), None);
}

#[test]
fn варианты_одного_ящика_дают_один_ключ() {
    assert_eq!(
        signals::email_key("Ivan.Petrov+vpn@googlemail.com"),
        "ivanpetrov@gmail.com"
    );
    assert_eq!(
        signals::email_key("ivanpetrov@gmail.com"),
        signals::email_key("i.v.a.n.petrov+1@gmail.com")
    );
    assert_eq!(
        signals::email_key("ivan.petrov+x@ya.ru"),
        "ivan-petrov@yandex.ru"
    );
    assert_eq!(signals::email_key("a.b+c@mail.ru"), "a.b@mail.ru");
    assert_eq!(signals::email_key("+tag@mail.ru"), "+tag@mail.ru");
}

#[test]
fn настройки_антибота_разбираются_и_проверяются() {
    let defaults = AntibotSettings::parse(&ModuleConfig::default()).unwrap();
    assert_eq!(defaults, AntibotSettings::default());
    assert_eq!(defaults.mode, Mode::Confirm);
    let config = ModuleConfig::from_value(&serde_json::json!({
        "mode": "block",
        "young_days": "14",
        "check_username": false,
        "burst_count": 0,
        "ip_limit": "",
        "extra_domains": "temp.example, junk.test",
        "referral_guard": "false",
        "mass_invites": 7
    }));
    let parsed = AntibotSettings::parse(&config).unwrap();
    assert_eq!(parsed.mode, Mode::Block);
    assert_eq!(parsed.young_days, 14);
    assert!(!parsed.check_username);
    assert_eq!(parsed.burst_count, 0);
    assert_eq!(parsed.ip_limit, 3);
    assert_eq!(parsed.extra_domains, ["junk.test", "temp.example"]);
    assert!(!parsed.referral_guard);
    assert_eq!(parsed.mass_invites, 7);
    let bad = ModuleConfig::from_value(&serde_json::json!({ "mode": "ban" }));
    assert!(AntibotModule.validate_config(&bad).is_err());
    let negative = ModuleConfig::from_value(&serde_json::json!({ "young_days": -1 }));
    assert!(AntibotSettings::parse(&negative).is_err());
}

async fn enable(store: &sn_store::Store, config: serde_json::Value) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO module_states (module, enabled, config) VALUES ('antibot', true, $1) \
         ON CONFLICT (module) DO UPDATE SET enabled = true, config = EXCLUDED.config",
    )
    .bind(config)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn session(store: &sn_store::Store, client_id: ClientId, ip: &str) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO client_sessions (client_id, token_hash, channel, ip, expires_at) \
         VALUES ($1, $2, 'miniapp', $3, now() + interval '1 day')",
    )
    .bind(client_id.0)
    .bind(format!("t-{}-{ip}", client_id.0).into_bytes())
    .bind(ip)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn invite(store: &sn_store::Store, client_id: ClientId, referrer: ClientId) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query("INSERT INTO referral_links (client_id, referrer_id) VALUES ($1, $2)")
        .bind(client_id.0)
        .bind(referrer.0)
        .execute(conn.conn())
        .await
        .unwrap();
}

#[tokio::test]
async fn без_включённого_модуля_проверки_пропускают_всех() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let id = client(&db.store).await;
    assert_eq!(trial_gate(&ctx, id).await.unwrap(), Gate::Allow);
    assert!(promo_allowed(&ctx, id).await.unwrap());
    assert!(bonus_allowed(&ctx, id).await.unwrap());
    assert_eq!(referral_gate(&ctx, id).await.unwrap(), ReferralGate::Allow);
    db.cleanup().await;
}

#[tokio::test]
async fn самоприглашение_с_общего_адреса_не_даёт_бонуса_пригласившему() {
    let Some(db) = test_store().await else {
        return;
    };
    enable(&db.store, serde_json::json!({ "mode": "confirm" })).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let referrer = client(&db.store).await;
    let payer = client(&db.store).await;
    let honest = client(&db.store).await;
    invite(&db.store, payer, referrer).await;
    invite(&db.store, honest, referrer).await;
    session(&db.store, referrer, "2001:db8:5:6::1").await;
    session(&db.store, payer, "2001:db8:5:6::99").await;
    session(&db.store, honest, "198.51.100.77").await;
    assert_eq!(
        referral_gate(&ctx, payer).await.unwrap(),
        ReferralGate::NoDirect(Signal::SelfInvite)
    );
    assert_eq!(
        referral_gate(&ctx, honest).await.unwrap(),
        ReferralGate::Allow
    );
    let mut conn = db.store.conn().await.unwrap();
    let mark = store::mark_of(&mut conn, payer).await.unwrap().unwrap();
    assert_eq!(mark.reasons, [Signal::SelfInvite]);
    drop(conn);
    record_no_bonus(
        &ctx,
        payer,
        PaymentId(1),
        &ReferralGate::NoDirect(Signal::SelfInvite),
        &[1],
    )
    .await
    .unwrap();
    record_no_bonus(
        &ctx,
        payer,
        PaymentId(1),
        &ReferralGate::NoDirect(Signal::SelfInvite),
        &[1],
    )
    .await
    .unwrap();
    let mut conn = db.store.conn().await.unwrap();
    let journal = store::journal(
        &mut conn,
        JournalFilter::Referral,
        None,
        sn_domain::paging::PageRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(journal.items.len(), 1);
    assert_eq!(journal.items[0].details["referrer_id"], referrer.0);
    drop(conn);
    clear(&ctx, payer, None).await.unwrap();
    assert_eq!(
        referral_gate(&ctx, payer).await.unwrap(),
        ReferralGate::Allow
    );
    db.cleanup().await;
}

#[tokio::test]
async fn массовые_приглашения_новых_аккаунтов_без_бонуса() {
    let Some(db) = test_store().await else {
        return;
    };
    enable(&db.store, serde_json::json!({ "mass_invites": 3 })).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut conn = db.store.conn().await.unwrap();
    for n in 0..25_i64 {
        let id = client(&db.store).await;
        conn.clients()
            .add_identity(
                id,
                &sn_domain::client::NewIdentity::telegram(&sn_domain::client::TelegramProfile {
                    id: 10_000 + n,
                    username: Some(format!("old{n}")),
                    first_name: None,
                    last_name: None,
                    language_code: None,
                    is_premium: false,
                }),
            )
            .await
            .unwrap();
    }
    sqlx::query("UPDATE client_identities SET created_at = now() - interval '100 days'")
        .execute(conn.conn())
        .await
        .unwrap();
    let referrer = client(&db.store).await;
    let mut invited = Vec::new();
    for n in 0..3_i64 {
        let id = client(&db.store).await;
        conn.clients()
            .add_identity(
                id,
                &sn_domain::client::NewIdentity::telegram(&sn_domain::client::TelegramProfile {
                    id: 8_000_000_000 + n * 50_000_000,
                    username: Some(format!("new{n}")),
                    first_name: None,
                    last_name: None,
                    language_code: None,
                    is_premium: false,
                }),
            )
            .await
            .unwrap();
        invite(&db.store, id, referrer).await;
        invited.push(id);
    }
    drop(conn);
    assert_eq!(
        referral_gate(&ctx, invited[0]).await.unwrap(),
        ReferralGate::NoDirect(Signal::MassInvites)
    );
    enable(&db.store, serde_json::json!({ "mass_invites": 4 })).await;
    assert_eq!(
        referral_gate(&ctx, invited[1]).await.unwrap(),
        ReferralGate::Allow
    );
    db.cleanup().await;
}

#[tokio::test]
async fn промо_ссылка_не_выдаётся_отмеченному_в_режиме_запрета() {
    let Some(db) = test_store().await else {
        return;
    };
    enable(&db.store, serde_json::json!({ "mode": "block" })).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let suspect = client(&db.store).await;
    let honest = client(&db.store).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.clients()
            .add_identity(
                suspect,
                &sn_domain::client::NewIdentity::new(
                    sn_domain::client::IdentityKind::Email,
                    "bot@mailinator.com",
                    true,
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let link = crate::catalog::promolinks::create(
        &ctx,
        &crate::catalog::promolinks::NewLink {
            name: "Блогер".into(),
            code: Some("blogger-ivan".into()),
            squads: vec![sn_domain::ids::SquadId(uuid::Uuid::from_u128(3))],
            days: 7,
            traffic_gb: None,
            traffic_reset: sn_domain::traffic::TrafficReset::NoReset,
            devices: 1,
            max_activations: None,
            expires_at: None,
        },
        None,
    )
    .await
    .unwrap();
    match crate::catalog::promolinks::activate(&ctx, suspect, &link.link.code).await {
        Err(crate::catalog::promolinks::LinkError::Rejected(rejection)) => {
            assert_eq!(rejection.code(), DENIED_CODE);
            assert!(rejection.message().contains("недоступны"));
        }
        other => panic!("ожидался отказ антибота, получено {other:?}"),
    }
    let mut conn = db.store.conn().await.unwrap();
    let journal = store::journal(
        &mut conn,
        JournalFilter::Promo,
        Some(suspect),
        sn_domain::paging::PageRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(journal.items.len(), 1);
    assert_eq!(journal.items[0].details["target"], "promo_link");
    assert_eq!(journal.items[0].title, "Бесплатные дни не выданы");
    drop(conn);
    assert!(free_allowed(&ctx, honest, "promo_link").await.unwrap());
    db.cleanup().await;
}

#[test]
fn защита_рулетки_и_конкурсов_включена_по_умолчанию() {
    let defaults = AntibotSettings::parse(&ModuleConfig::default()).unwrap();
    assert!(defaults.roulette_guard && defaults.contest_guard);
    assert!(Target::Roulette.guarded(&defaults) && Target::Contest.guarded(&defaults));
    let off = AntibotSettings::parse(&ModuleConfig::from_value(&serde_json::json!({
        "roulette_guard": "false",
        "contest_guard": false
    })))
    .unwrap();
    assert!(!Target::Roulette.guarded(&off) && !Target::Contest.guarded(&off));
    let keys: Vec<String> = AntibotModule
        .settings_schema()
        .into_iter()
        .map(|f| f.key.to_string())
        .collect();
    assert!(keys.contains(&"roulette_guard".to_string()));
    assert!(keys.contains(&"contest_guard".to_string()));
    assert_eq!(
        JournalFilter::parse(Some("roulette")),
        Some(JournalFilter::Roulette)
    );
    assert_eq!(
        JournalFilter::parse(Some("contest")),
        Some(JournalFilter::Contest)
    );
    for action in Action::ALL {
        assert_eq!(Action::parse(action.as_str()), Some(action));
    }
}

#[tokio::test]
async fn журнал_принимает_все_виды_срабатываний() {
    let Some(db) = test_store().await else {
        return;
    };
    let id = client(&db.store).await;
    let mut conn = db.store.conn().await.unwrap();
    for action in Action::ALL {
        assert!(
            store::event(
                &mut conn,
                &NewEvent {
                    client_id: id,
                    action,
                    reasons: &[Signal::IpLimit],
                    details: serde_json::json!({}),
                    dedupe: None,
                    admin: None,
                },
            )
            .await
            .unwrap()
        );
    }
    let all = store::journal(
        &mut conn,
        JournalFilter::All,
        Some(id),
        sn_domain::paging::PageRequest::new(Some(50), None),
    )
    .await
    .unwrap();
    assert_eq!(all.items.len(), Action::ALL.len());
    let roulette = store::journal(
        &mut conn,
        JournalFilter::Roulette,
        Some(id),
        sn_domain::paging::PageRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(roulette.items.len(), 3);
    let contest = store::journal(
        &mut conn,
        JournalFilter::Contest,
        Some(id),
        sn_domain::paging::PageRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(contest.items.len(), 4);
    drop(conn);
    db.cleanup().await;
}
