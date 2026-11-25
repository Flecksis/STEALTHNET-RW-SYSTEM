use super::signals::{self, Signal};
use super::*;
use crate::catalog::testing::{TestCore, client, context, pooled};
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
async fn общее_устройство_ищется_без_удержания_соединения_с_базой() {
    let Some(db) = test_store().await else {
        return;
    };
    let single = pooled(&db, 1, std::time::Duration::from_secs(2)).await;
    let core = TestCore::new(&single);
    let ctx = context(&single, &core, Currency::Usd);
    enable(&single, serde_json::json!({ "mode": "confirm" })).await;
    let referrer = client(&single).await;
    let friend = client(&single).await;
    let twin = client(&single).await;
    invite(&single, friend, referrer).await;
    invite(&single, twin, referrer).await;
    if let Ok(mut devices) = core.devices.lock() {
        devices.push((referrer, "phone-1".into()));
        devices.push((friend, "phone-2".into()));
        devices.push((twin, "phone-1".into()));
    }
    assert_eq!(
        referral_gate(&ctx, friend).await.unwrap(),
        ReferralGate::Allow
    );
    assert_eq!(
        referral_gate(&ctx, twin).await.unwrap(),
        ReferralGate::NoDirect(Signal::SelfInvite)
    );
    single.close().await;
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
async fn массовые_приглашения_не_лишают_бонуса_за_обычных_друзей() {
    let Some(db) = test_store().await else {
        return;
    };
    enable(&db.store, serde_json::json!({ "mass_invites": 3 })).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    brought(&db.store, 7_000_000_000, 1_000, 25, 400).await;
    let referrer = client(&db.store).await;
    let mut usual = Vec::new();
    for n in 0..4_i64 {
        let id = newcomer(&db.store, 1_000_000_000 + n * 50_000_000).await;
        invite(&db.store, id, referrer).await;
        usual.push(id);
    }
    let mut fresh = Vec::new();
    for n in 0..3_i64 {
        let id = newcomer(&db.store, 8_000_000_000 + n * 50_000_000).await;
        invite(&db.store, id, referrer).await;
        fresh.push(id);
    }
    for id in &usual {
        assert_eq!(referral_gate(&ctx, *id).await.unwrap(), ReferralGate::Allow);
    }
    assert_eq!(
        referral_gate(&ctx, fresh[0]).await.unwrap(),
        ReferralGate::NoDirect(Signal::MassInvites)
    );
    let mut conn = db.store.conn().await.unwrap();
    let now = db_now(&mut conn).await.unwrap();
    let invites: Vec<Invite> = usual
        .iter()
        .chain(&fresh)
        .map(|id| Invite {
            referrer,
            invitee: *id,
            linked_at: now,
        })
        .collect();
    let settings = AntibotSettings {
        mode: Mode::Mark,
        mass_invites: 3,
        ..AntibotSettings::default()
    };
    let skipped = screen_invites(&mut conn, &settings, &invites)
        .await
        .unwrap();
    assert_eq!(skipped, fresh.iter().copied().collect::<HashSet<_>>());
    for id in &usual {
        assert!(store::mark_of(&mut conn, *id).await.unwrap().is_none());
    }
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn перенесённые_друзья_не_считаются_массовыми_приглашениями() {
    let Some(db) = test_store().await else {
        return;
    };
    enable(&db.store, serde_json::json!({ "mass_invites": 3 })).await;
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    brought(&db.store, 7_000_000_000, 1_000, 25, 400).await;
    brought(&db.store, 8_000_000_000, 50_000_000, 4, 1).await;
    let referrer = client(&db.store).await;
    let mut friends = Vec::new();
    for n in 0..4_i64 {
        let id = of_telegram(&db.store, 8_000_000_000 + n * 50_000_000).await;
        invite(&db.store, id, referrer).await;
        friends.push(id);
    }
    for id in &friends {
        assert_eq!(referral_gate(&ctx, *id).await.unwrap(), ReferralGate::Allow);
    }
    let live = newcomer(&db.store, 8_900_000_000).await;
    invite(&db.store, live, referrer).await;
    assert_eq!(
        referral_gate(&ctx, live).await.unwrap(),
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
            assert!(rejection.message().contains("недоступна"));
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
    assert_eq!(journal.items[0].title, "Подписка по промо-ссылке не выдана");
    assert_eq!(journal.items[0].details["name"], "Блогер");
    drop(conn);
    assert_eq!(
        promo_link_gate(&ctx, honest, link.link.id, &link.link.name)
            .await
            .unwrap(),
        Gate::Allow
    );
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
    let titles = |page: sn_domain::paging::Page<JournalItem>| -> Vec<&'static str> {
        page.items.iter().map(|i| i.action.as_str()).collect()
    };
    let trial = store::journal(
        &mut conn,
        JournalFilter::Trial,
        Some(id),
        sn_domain::paging::PageRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(titles(trial), ["trial_denied", "confirmed", "asked"]);
    let promo = store::journal(
        &mut conn,
        JournalFilter::Promo,
        Some(id),
        sn_domain::paging::PageRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        titles(promo),
        ["promo_link_denied", "promo_link_asked", "promo_denied"]
    );
    drop(conn);
    db.cleanup().await;
}

async fn telegram_client(store: &sn_store::Store, telegram_id: i64, premium: bool) -> ClientId {
    let id = client(store).await;
    let mut conn = store.conn().await.unwrap();
    conn.clients()
        .add_identity(
            id,
            &sn_domain::client::NewIdentity::telegram(&sn_domain::client::TelegramProfile {
                id: telegram_id,
                username: Some(format!("farm{telegram_id}")),
                first_name: Some("Ферма".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: premium,
            }),
        )
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn наплыв_регистраций_отмечает_новых_только_при_включённом_модуле() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let early = telegram_client(&db.store, 6_100_001, false).await;
    assert!(
        !note_surge(&ctx, early, 400).await.unwrap(),
        "модуль выключен"
    );
    enable(&db.store, serde_json::json!({ "mode": "confirm" })).await;
    let newcomer = telegram_client(&db.store, 6_100_002, false).await;
    let premium = telegram_client(&db.store, 6_100_003, true).await;
    assert!(note_surge(&ctx, newcomer, 400).await.unwrap());
    assert!(!note_surge(&ctx, premium, 400).await.unwrap());
    let mut conn = db.store.conn().await.unwrap();
    let mark = store::mark_of(&mut conn, newcomer).await.unwrap().unwrap();
    assert_eq!(mark.reasons, [Signal::Surge]);
    assert_eq!(mark.details["surge"], 400);
    assert!(store::mark_of(&mut conn, premium).await.unwrap().is_none());
    assert!(store::mark_of(&mut conn, early).await.unwrap().is_none());
    drop(conn);
    assert_eq!(trial_gate(&ctx, newcomer).await.unwrap(), Gate::Confirm);
    assert_eq!(trial_gate(&ctx, premium).await.unwrap(), Gate::Allow);
    assert_eq!(
        Signal::Surge.title_ru(),
        "регистрация во время наплыва новых аккаунтов"
    );
    db.cleanup().await;
}

#[test]
fn капча_в_настройках_требует_оба_ключа_и_не_показывает_секрет() {
    let off = AntibotSettings::parse(&ModuleConfig::default()).unwrap();
    assert_eq!(off.captcha, captcha::Provider::None);
    assert_eq!(off.captcha_trial, captcha::Scope::All);
    assert!(off.captcha_signup);
    let full = ModuleConfig::from_value(&serde_json::json!({
        "captcha": "turnstile",
        "captcha_site_key": " 0x4AAAAAAAsite ",
        "captcha_secret": "0x4AAAAAAAsecret",
        "captcha_trial": "suspicious",
        "captcha_signup": false,
    }));
    assert!(AntibotModule.validate_config(&full).is_ok());
    let settings = AntibotSettings::parse(&full).unwrap();
    assert_eq!(settings.captcha_site_key, "0x4AAAAAAAsite");
    assert_eq!(settings.captcha_trial, captcha::Scope::Suspicious);
    assert!(!settings.captcha_signup);
    let shown = serde_json::to_string(&settings).unwrap();
    assert!(!shown.contains("0x4AAAAAAAsecret"), "{shown}");
    let ready = captcha::Captcha::of(&full).unwrap();
    assert_eq!(ready.provider, captcha::Provider::Turnstile);
    assert!(!format!("{ready:?}").contains("0x4AAAAAAAsecret"));
    let widget = ready.widget("trial", Some("abc".into()));
    let json = serde_json::to_value(&widget).unwrap();
    assert_eq!(json["provider"], "turnstile");
    assert_eq!(json["action"], "trial");
    assert!(json.get("secret").is_none());
    for (config, needle) in [
        (
            serde_json::json!({ "captcha": "yandex", "captcha_secret": "ysc2_secret" }),
            "ключ сайта",
        ),
        (
            serde_json::json!({ "captcha": "yandex", "captcha_site_key": "ysc1_site" }),
            "секретный ключ",
        ),
        (serde_json::json!({ "captcha": "hcaptcha" }), "captcha:"),
        (
            serde_json::json!({ "captcha": "turnstile", "captcha_site_key": "<script>" }),
            "captcha_site_key",
        ),
    ] {
        let err = AntibotModule
            .validate_config(&ModuleConfig::from_value(&config))
            .unwrap_err()
            .to_string();
        assert!(err.contains(needle), "{err}");
    }
    let without_secret = ModuleConfig::from_value(&serde_json::json!({
        "mode": "block",
        "captcha": "turnstile",
        "captcha_site_key": "0x4AAAAAAAsite",
    }));
    assert_eq!(
        AntibotSettings::lenient(&without_secret).mode,
        Mode::Block,
        "настройки без секрета в базе не должны сбрасываться"
    );
    assert!(captcha::Captcha::of(&without_secret).is_none());
    assert!(AntibotModule.config_warning(&without_secret).is_some());
    assert!(AntibotModule.config_warning(&full).is_none());
}

#[test]
fn перепутанные_ключи_капчи_не_сохраняются() {
    let check = |provider: &str, site: &str, secret: &str| {
        AntibotModule.validate_config(&ModuleConfig::from_value(&serde_json::json!({
            "captcha": provider,
            "captcha_site_key": site,
            "captcha_secret": secret,
        })))
    };
    let turnstile = (
        "0x4AAF00AAAABn0R22HWm-YUc",
        "0x4AAF00AAAABn0R22HWm098HVBjhdsYUc",
    );
    let yandex = ("ysc1_yzF85abcdef", "ysc2_yzF85abcdef");
    for (provider, site, secret) in [
        ("turnstile", turnstile.0, turnstile.1),
        (
            "turnstile",
            "1x00000000000000000000AA",
            "1x0000000000000000000000000000000AA",
        ),
        ("yandex", yandex.0, yandex.1),
        ("none", yandex.1, yandex.0),
    ] {
        assert!(check(provider, site, secret).is_ok(), "{provider} {site}");
    }
    for (provider, site, secret, needle) in [
        ("turnstile", turnstile.1, turnstile.0, "местами"),
        ("yandex", yandex.1, yandex.0, "ysc2_"),
        ("yandex", "0123456789", yandex.0, "ysc1_"),
        ("turnstile", yandex.0, yandex.1, "Яндекс"),
        ("yandex", turnstile.0, turnstile.1, "Cloudflare"),
        ("turnstile", turnstile.0, turnstile.0, "один и тот же"),
        ("yandex", yandex.0, yandex.0, "ysc1_"),
    ] {
        let err = check(provider, site, secret).unwrap_err().to_string();
        assert!(err.contains(needle), "{provider} {site}: {err}");
        assert!(
            !err.contains("yzF85abcdef") && !err.contains("BVjhdsYUc"),
            "{err}"
        );
    }
}

#[test]
fn при_смене_сервиса_капчи_секрет_вставляется_заново() {
    let turnstile = ModuleConfig::from_value(&serde_json::json!({
        "captcha": "turnstile",
        "captcha_site_key": "0x4AAAAAAAsite",
        "captcha_secret": "0x4AAAAAAAsecret",
    }));
    let yandex = ModuleConfig::from_value(&serde_json::json!({
        "captcha": "yandex",
        "captcha_site_key": "ysc1_site",
        "captcha_secret": "0x4AAAAAAAsecret",
    }));
    assert!(matches!(
        AntibotModule.validate_change(&turnstile, &yandex, &[]),
        Err(ModuleError::SecretsAgain(_))
    ));
    let moved = ModuleConfig::from_value(&serde_json::json!({
        "captcha": "yandex",
        "captcha_site_key": "ysc1_site",
        "captcha_secret": "ysc2_secret",
    }));
    assert!(
        AntibotModule
            .validate_change(&turnstile, &moved, &["captcha_secret"])
            .is_ok()
    );
    assert!(
        AntibotModule
            .validate_change(&turnstile, &turnstile, &[])
            .is_ok()
    );
    let off = ModuleConfig::from_value(&serde_json::json!({ "captcha": "none" }));
    assert!(AntibotModule.validate_change(&turnstile, &off, &[]).is_ok());
    assert!(AntibotModule.validate_change(&off, &turnstile, &[]).is_ok());
    assert!(matches!(
        AntibotModule.validate_change(&off, &yandex, &[]),
        Err(ModuleError::SecretsAgain(_))
    ));
    let pasted = ModuleConfig::from_value(&serde_json::json!({
        "captcha": "turnstile",
        "captcha_site_key": "0x4AAAAAAAsite",
        "captcha_secret": "ysc2_secret",
    }));
    assert!(matches!(
        AntibotModule.validate_change(&turnstile, &pasted, &["captcha_secret"]),
        Err(ModuleError::Invalid(_))
    ));
}

#[test]
fn доказательство_работы_проверяется_по_числу_нулевых_битов() {
    assert_eq!(checks::leading_zero_bits(&[0, 0, 0x0f, 0xff]), 20);
    assert_eq!(checks::leading_zero_bits(&[0x80]), 0);
    assert_eq!(checks::leading_zero_bits(&[0, 0]), 16);
    let seed = "0f1e2d3c4b5a69788796a5b4c3d2e1f0";
    let proof = checks::solve(seed, 10);
    assert!(checks::work_done(seed, &proof, 10));
    assert!(!checks::work_done("другой-токен", &proof, 30));
    for bad in ["", "12a", "-1", "123456789012345678901", " "] {
        assert!(!checks::work_done(seed, bad, 0), "{bad:?}");
    }
    assert!(checks::work_done(seed, "7", 0));
    assert_eq!(checks::cdata(seed).len(), 32);
    assert_ne!(checks::cdata(seed), checks::cdata("другой"));
}

#[test]
fn проверка_в_ответе_сервера_несёт_вид_и_не_несёт_секрета() {
    let work = serde_json::to_value(Check::Work { bits: 18 }).unwrap();
    assert_eq!(work, serde_json::json!({ "kind": "work", "bits": 18 }));
    let button = serde_json::to_value(Check::Button).unwrap();
    assert_eq!(button, serde_json::json!({ "kind": "button" }));
    let captcha = serde_json::to_value(Check::Captcha(captcha::Widget {
        provider: captcha::Provider::Yandex,
        site_key: "ysc1_site".into(),
        action: "trial".into(),
        cdata: None,
    }))
    .unwrap();
    assert_eq!(
        captcha,
        serde_json::json!({ "kind": "captcha", "provider": "yandex", "site_key": "ysc1_site", "action": "trial" })
    );
}

async fn brought(store: &sn_store::Store, first: i64, step: i64, count: i64, age_days: i32) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "WITH made AS (INSERT INTO clients (locale, registration_source, created_at) \
           SELECT 'ru', CASE WHEN n % 2 = 0 THEN 'migration' ELSE 'import' END, \
                  now() - make_interval(days => $4) FROM generate_series(1, $3) n RETURNING id) \
         INSERT INTO client_identities (client_id, kind, value, verified, meta) \
         SELECT id, 'telegram', ($1 + (row_number() OVER (ORDER BY id) - 1) * $2)::text, true, \
                '{\"username\": null}'::jsonb FROM made",
    )
    .bind(first)
    .bind(step)
    .bind(count)
    .bind(age_days)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn of_telegram(store: &sn_store::Store, telegram_id: i64) -> ClientId {
    let mut conn = store.conn().await.unwrap();
    ClientId(
        sqlx::query_scalar(
            "SELECT client_id FROM client_identities WHERE kind = 'telegram' AND value = $1",
        )
        .bind(telegram_id.to_string())
        .fetch_one(conn.conn())
        .await
        .unwrap(),
    )
}

async fn newcomer(store: &sn_store::Store, telegram_id: i64) -> ClientId {
    let id = client(store).await;
    let mut conn = store.conn().await.unwrap();
    conn.clients()
        .add_identity(
            id,
            &sn_domain::client::NewIdentity::telegram(&sn_domain::client::TelegramProfile {
                id: telegram_id,
                username: None,
                first_name: Some("Новый".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            }),
        )
        .await
        .unwrap();
    id
}

async fn found(store: &sn_store::Store, id: ClientId) -> Vec<Signal> {
    let mut conn = store.conn().await.unwrap();
    let now = db_now(&mut conn).await.unwrap();
    assess(&mut conn, id, &AntibotSettings::default(), now)
        .await
        .unwrap()
        .signals
}

#[tokio::test]
async fn перенесённые_клиенты_не_считаются_серией_регистраций() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    enable(&db.store, serde_json::json!({ "mode": "block" })).await;
    brought(&db.store, 7_000_000_000, 1_000, 30, 400).await;
    let moved = of_telegram(&db.store, 7_000_005_000).await;
    assert_eq!(found(&db.store, moved).await, []);
    assert_eq!(trial_gate(&ctx, moved).await.unwrap(), Gate::Allow);
    assert!(promo_allowed(&ctx, moved).await.unwrap());
    assert_eq!(
        referral_gate(&ctx, moved).await.unwrap(),
        ReferralGate::Allow
    );
    let live = newcomer(&db.store, 7_000_005_500).await;
    assert_eq!(found(&db.store, live).await, [Signal::NoUsername]);
    let mut farm = Vec::new();
    for n in 0..5 {
        farm.push(newcomer(&db.store, 7_000_006_100 + n).await);
    }
    assert!(
        found(&db.store, farm[4]).await.contains(&Signal::Burst),
        "серия настоящих регистраций по-прежнему видна"
    );
    assert!(!found(&db.store, moved).await.contains(&Signal::Burst));
    db.cleanup().await;
}

#[tokio::test]
async fn граница_новых_аккаунтов_учитывает_дату_регистрации_в_старом_боте() {
    let Some(db) = test_store().await else {
        return;
    };
    brought(&db.store, 7_000_000_000, 10_000_000, 25, 400).await;
    let known = newcomer(&db.store, 7_100_000_500).await;
    assert_eq!(found(&db.store, known).await, [Signal::NoUsername]);
    let young = newcomer(&db.store, 8_900_000_000).await;
    assert_eq!(
        found(&db.store, young).await,
        [Signal::YoungAccount, Signal::NoUsername]
    );
    brought(&db.store, 8_950_000_000, 10_000_000, 1, 3).await;
    let moved = of_telegram(&db.store, 8_950_000_000).await;
    assert_eq!(found(&db.store, moved).await, []);
    db.cleanup().await;
}

#[tokio::test]
async fn граница_новых_аккаунтов_не_пересчитывается_на_каждую_проверку() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    enable(&db.store, serde_json::json!({ "mode": "confirm" })).await;
    let settings = AntibotSettings::default();
    let boundary = async |settings: &AntibotSettings| {
        let mut conn = db.store.conn().await.unwrap();
        let now = db_now(&mut conn).await.unwrap();
        young_boundary(&mut conn, settings, now).await.unwrap()
    };
    let someone = client(&db.store).await;
    assert_eq!(trial_gate(&ctx, someone).await.unwrap(), Gate::Allow);
    assert_eq!(boundary(&settings).await, None, "истории ещё мало");
    brought(&db.store, 6_000_000_000, 1_000_000, 25, 200).await;
    assert_eq!(boundary(&settings).await, Some(6_024_000_000));
    assert_eq!(trial_gate(&ctx, someone).await.unwrap(), Gate::Allow);
    brought(&db.store, 6_500_000_000, 1, 1, 200).await;
    assert_eq!(
        boundary(&settings).await,
        Some(6_024_000_000),
        "граница взята из сохранённого"
    );
    let shorter = AntibotSettings {
        young_days: 14,
        ..AntibotSettings::default()
    };
    assert_eq!(boundary(&shorter).await, Some(6_500_000_000));
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query(
        "UPDATE bot_state SET updated_at = now() - interval '61 minutes' WHERE key = 'antibot.young'",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    assert_eq!(boundary(&settings).await, Some(6_500_000_000));
    assert_eq!(trial_gate(&ctx, someone).await.unwrap(), Gate::Allow);
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        store::saved_boundary(&mut conn, 30).await.unwrap(),
        Some(6_500_000_000)
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn лимит_регистраций_с_адреса_видит_все_регистрации_за_сутки() {
    let Some(db) = test_store().await else {
        return;
    };
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query(
        "WITH made AS (INSERT INTO clients (locale, registration_source, created_at) \
           SELECT 'ru', 'web', now() - make_interval(secs => 80000 - n) \
           FROM generate_series(1, 2100) n RETURNING id) \
         INSERT INTO client_sessions (client_id, token_hash, channel, ip, expires_at, created_at) \
         SELECT id, decode(md5(id::text), 'hex'), 'cabinet', '198.51.100.' || (id % 250), \
                now() + interval '1 day', now() - interval '22 hours' FROM made",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    let mut made = Vec::new();
    for ip in [
        "203.0.113.7",
        "203.0.113.7",
        "203.0.113.7",
        "203.0.113.7",
        "2001:db8:1:2::1",
        "2001:db8:1:2::2",
        "2001:db8:1:2:ffff::3",
        "2001:db8:1:2::4",
        "2001:db8:1:3::5",
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO clients (locale, registration_source) VALUES ('ru', 'web') RETURNING id",
        )
        .fetch_one(conn.conn())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO client_sessions (client_id, token_hash, channel, ip, expires_at) \
             VALUES ($1, decode(md5('s' || $1::text), 'hex'), 'cabinet', $2, now() + interval '1 day')",
        )
        .bind(id)
        .bind(ip)
        .execute(conn.conn())
        .await
        .unwrap();
        made.push(ClientId(id));
    }
    drop(conn);
    let mut limited = Vec::new();
    for id in &made {
        limited.push(found(&db.store, *id).await.contains(&Signal::IpLimit));
    }
    assert_eq!(
        limited,
        [false, false, false, true, false, false, false, true, false]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn поспешное_нажатие_не_отодвигает_следующую_попытку() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let id = client(&db.store).await;
    let press = async |token: &str| {
        checks::confirm(&ctx, id, token, PROMO_LINK, Answer::button())
            .await
            .unwrap()
    };
    let ask = async || {
        checks::challenge(&ctx, id, PROMO_LINK, Surface::Bot)
            .await
            .unwrap()
            .token
    };
    let first = ask().await;
    assert_eq!(press(&first).await, Confirmation::TooFast);
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query("UPDATE antibot_checks SET created_at = now() - interval '3 seconds'")
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    let second = ask().await;
    assert_ne!(first, second);
    assert_eq!(
        press(&first).await,
        Confirmation::Expired,
        "старая кнопка погашена"
    );
    assert_eq!(press(&second).await, Confirmation::Confirmed);
    let third = ask().await;
    assert_eq!(
        press(&third).await,
        Confirmation::TooFast,
        "после пройденной проверки отсчёт новый"
    );
    let other = checks::challenge(&ctx, id, Target::Contest.as_str(), Surface::Bot)
        .await
        .unwrap()
        .token;
    assert_eq!(
        checks::confirm(&ctx, id, &other, Target::Contest.as_str(), Answer::button())
            .await
            .unwrap(),
        Confirmation::TooFast,
        "у другого действия отсчёт свой"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn удалённые_клиенты_не_висят_в_списке_отмеченных() {
    let Some(db) = test_store().await else {
        return;
    };
    let kept = client(&db.store).await;
    let gone = client(&db.store).await;
    let mut conn = db.store.conn().await.unwrap();
    for id in [kept, gone] {
        store::save_mark(
            &mut conn,
            id,
            &[Signal::DisposableEmail],
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }
    conn.clients().soft_delete(gone).await.unwrap();
    let page = store::open_marks(&mut conn, sn_domain::paging::PageRequest::default())
        .await
        .unwrap();
    assert_eq!(
        page.items.iter().map(|m| m.client_id).collect::<Vec<_>>(),
        [kept]
    );
    assert_eq!(store::summary(&mut conn).await.unwrap().marked_open, 1);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn почта_привязанная_без_кода_не_отмечает_других_клиентов() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let first = newcomer(store, 9_100_001).await;
    let second = newcomer(store, 9_100_002).await;
    let mut conn = store.conn().await.unwrap();
    conn.clients()
        .add_identity(
            first,
            &sn_domain::client::NewIdentity::email_without_code("ivan.petrov+vpn@gmail.com")
                .unwrap(),
        )
        .await
        .unwrap();
    conn.clients()
        .add_identity(
            second,
            &sn_domain::client::NewIdentity::new(
                sn_domain::client::IdentityKind::Email,
                "ivanpetrov@gmail.com",
                true,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        super::store::email_twin(&mut conn, second, "ivanpetrov@gmail.com")
            .await
            .unwrap(),
        None
    );
    sqlx::query("UPDATE client_identities SET verified = true WHERE client_id = $1")
        .bind(first.0)
        .execute(conn.conn())
        .await
        .unwrap();
    assert_eq!(
        super::store::email_twin(&mut conn, second, "ivanpetrov@gmail.com")
            .await
            .unwrap(),
        Some(first)
    );
    drop(conn);
    db.cleanup().await;
}

#[test]
fn пределы_сайта_по_умолчанию_выключены_и_проверяются_по_границам() {
    let parse =
        |config: serde_json::Value| AntibotSettings::parse(&ModuleConfig::from_value(&config));
    let defaults = parse(serde_json::json!({})).unwrap();
    assert_eq!(
        (
            defaults.site_address_day,
            defaults.site_network_day,
            defaults.site_minute
        ),
        (0, 0, 0)
    );
    let set = parse(serde_json::json!({
        "site_address_day": "10", "site_network_day": 30, "site_minute": 100_000
    }))
    .unwrap();
    assert_eq!(
        (set.site_address_day, set.site_network_day, set.site_minute),
        (10, 30, 100_000)
    );
    for field in ["site_address_day", "site_network_day", "site_minute"] {
        for bad in [
            serde_json::json!(100_001),
            serde_json::json!(-1),
            serde_json::json!("много"),
        ] {
            let error = parse(serde_json::json!({ field: bad })).unwrap_err();
            assert!(error.starts_with(field), "{error}");
        }
    }
    let schema = AntibotModule.settings_schema();
    for field in ["site_address_day", "site_network_day", "site_minute"] {
        let found = schema.iter().find(|f| f.key == field).unwrap();
        assert!(found.label.starts_with("Предел"), "{}", found.label);
    }
}

#[test]
fn адрес_регистрации_разбирается_на_адрес_и_сеть() {
    use signups::{Hidden, Origin, cloudflare};
    for (ip, edge) in [
        ("172.64.0.0", true),
        ("172.71.255.255", true),
        ("172.72.0.0", false),
        ("172.63.255.255", false),
        ("104.16.0.1", true),
        ("104.28.0.1", false),
        ("131.0.72.5", true),
        ("131.0.76.0", false),
        ("2606:4700::1", true),
        ("2606:4701::1", false),
        ("2a06:98c7:ffff::1", true),
        ("2a06:98c8::1", false),
        ("::ffff:162.158.1.1", true),
        ("8.8.8.8", false),
    ] {
        assert_eq!(cloudflare(ip.parse().unwrap()), edge, "{ip}");
    }
    assert_eq!(
        Origin::parse(Some(" 203.0.113.9 ")),
        Origin::Known {
            address: "203.0.113.9".into(),
            network: None
        }
    );
    assert_eq!(
        Origin::parse(Some("::ffff:203.0.113.9")),
        Origin::Known {
            address: "203.0.113.9".into(),
            network: None
        }
    );
    assert_eq!(
        Origin::parse(Some("2a00:1234:5678:9abc:def0::1")),
        Origin::Known {
            address: "2a00:1234:5678:9abc::/64".into(),
            network: Some("2a00:1234:5678::/48".into())
        }
    );
    for (ip, kind) in [
        ("172.68.1.1", Hidden::Cloudflare),
        ("10.0.0.5", Hidden::Internal),
        ("127.0.0.1", Hidden::Internal),
        ("100.64.0.1", Hidden::Internal),
        ("fd00::1", Hidden::Internal),
    ] {
        assert!(
            matches!(Origin::parse(Some(ip)), Origin::Hidden { kind: found, .. } if found == kind),
            "{ip}"
        );
    }
    for unknown in [
        None,
        Some(""),
        Some("unknown"),
        Some("203.0.113.9, 1.1.1.1"),
    ] {
        assert_eq!(Origin::parse(unknown), Origin::Unknown);
    }
    let line = signups::trusted_line();
    assert!(line.starts_with("SN_TRUSTED_PROXIES=127.0.0.1,172.16.0.0/12,173.245.48.0/20,"));
    assert_eq!(line.matches('/').count(), 23);
}
