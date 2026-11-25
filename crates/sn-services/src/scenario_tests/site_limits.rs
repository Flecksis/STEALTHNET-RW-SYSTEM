use super::mail::{Letters, code_in, configure_with, fake_smtp, readable};
use super::siteverify::siteverify;
use super::*;
use crate::clients::Entered;
use crate::signup::{Arrival, Registration};
use crate::two_factor::CabinetLogin;
use sn_modules::catalog::antibot::signups::{self, Hidden, Refusal};
use sn_modules::catalog::antibot::{self, AntibotModule};
use sn_modules::catalog::mail::MailModule;
use std::time::Duration as Pause;

const PASSWORD: &str = "длинный пароль 1";

async fn site(login: &str, confirm: bool) -> Option<(Env, AdminId, Letters)> {
    let modules: Vec<Arc<dyn Module>> = vec![Arc::new(MailModule), Arc::new(AntibotModule)];
    let env = env_with(ModuleRegistry::new(modules).unwrap()).await?;
    let admin = owner(&env.services).await;
    let (port, letters) = fake_smtp().await;
    configure_with(
        &env.services,
        admin,
        port,
        serde_json::json!({ "login": login, "confirm_signup": confirm }),
    )
    .await;
    env.services
        .settings_ops()
        .toggle_module(ModuleId::Antibot, true, admin)
        .await
        .unwrap();
    {
        let mut conn = env.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                sn_domain::settings::keys::BOT_USERNAME,
                &serde_json::json!("stealth_test_bot"),
                None,
            )
            .await
            .unwrap();
    }
    Some((env, admin, letters))
}

async fn limits(services: &Services, config: serde_json::Value) {
    services
        .modules()
        .set_config(
            services.store(),
            ModuleId::Antibot,
            sn_modules::ModuleConfig::from_value(&config),
            None,
        )
        .await
        .unwrap();
}

async fn captcha(services: &Services, admin: AdminId, config: serde_json::Value) {
    let mut all = serde_json::json!({
        "captcha": "turnstile",
        "captcha_site_key": "0x4AAAAAAAsite",
        "captcha_secret": "0x4AAAAAAAsecret-key",
        "captcha_signup": false,
        "burst_count": 0,
    })
    .as_object()
    .cloned()
    .unwrap();
    all.extend(config.as_object().cloned().unwrap());
    services
        .settings_ops()
        .update_module_config(
            ModuleId::Antibot,
            &ModuleConfigUpdate {
                config: all,
                clear: Vec::new(),
            },
            admin,
        )
        .await
        .unwrap();
}

async fn forget_tries(services: &Services) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query("DELETE FROM auth_rate_limits")
        .execute(conn.conn())
        .await
        .unwrap();
}

async fn early_in_the_minute(services: &Services, until: f64) {
    let mut conn = services.store().conn().await.unwrap();
    let second: f64 = sqlx::query_scalar("SELECT extract(second FROM now())::float8")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    drop(conn);
    if second > until {
        tokio::time::sleep(Pause::from_secs_f64(60.3 - second)).await;
    }
}

async fn number(services: &Services, sql: &str) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(sql)
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

async fn made(services: &Services) -> i64 {
    number(services, "SELECT count(*) FROM clients").await
}

async fn day(services: &Services) -> signups::Day {
    let mut conn = services.store().conn().await.unwrap();
    signups::day(&mut conn).await.unwrap()
}

async fn register(services: &Services, email: &str, ip: &str) -> ServiceResult<CabinetLogin> {
    register_with(services, email, ip, None).await
}

async fn register_with(
    services: &Services,
    email: &str,
    ip: &str,
    token: Option<&str>,
) -> ServiceResult<CabinetLogin> {
    match services
        .auth()
        .email_signup_with(email, PASSWORD, &meta(ip), Arrival::default(), token)
        .await?
    {
        Registration::Login(login) => Ok(login),
        Registration::CodeSent(_) => panic!("ожидался аккаунт сразу"),
    }
}

fn last_code(letters: &Letters) -> String {
    code_in(&readable(letters.lock().unwrap().last().unwrap()))
}

async fn by_code(
    services: &Services,
    letters: &Letters,
    email: &str,
    ip: &str,
) -> ServiceResult<CabinetLogin> {
    let auth = services.auth();
    auth.email_login_start(email, &meta(ip)).await.unwrap();
    let code = last_code(letters);
    auth.email_login_verify(email, &code, &meta(ip)).await
}

async fn from_telegram(services: &Services, id: i64, ip: &str) -> Entered {
    let auth = services.auth();
    let start = auth.telegram_login_start(&meta(ip)).await.unwrap();
    let param = start.link.rsplit_once("?start=").unwrap().1.to_string();
    let visit = auth.telegram_login_visit(&param).await.unwrap();
    assert!(visit.is_some());
    services
        .clients()
        .ensure_telegram_from(
            &profile(id),
            RegistrationSource::Bot,
            Some(&param),
            visit.as_ref(),
            || true,
        )
        .await
        .unwrap()
}

async fn filled(services: &Services, address: &str, network: Option<&str>, rows: i32) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "INSERT INTO site_signups (via, address, network) \
         SELECT 'password', $1, $2 FROM generate_series(1, $3)",
    )
    .bind(address)
    .bind(network)
    .bind(rows)
    .execute(conn.conn())
    .await
    .unwrap();
}

#[tokio::test]
async fn сорок_регистраций_разом_дают_ровно_предел_сети() {
    let Some((env, ..)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    limits(services, serde_json::json!({ "site_network_day": 7 })).await;
    let mut tasks = Vec::new();
    for n in 0..40 {
        let services = services.clone();
        tasks.push(tokio::spawn(async move {
            register(
                &services,
                &format!("wave{n}@example.com"),
                &format!("2a00:1234:5678:{n:x}::1"),
            )
            .await
        }));
    }
    let mut created = 0;
    let mut limited = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => created += 1,
            Err(err) if err.code() == signups::LIMIT_CODE => limited += 1,
            Err(err) => assert_eq!(err.code(), crate::limits::RATE_LIMITED),
        }
    }
    assert_eq!(created, 7);
    assert_eq!(made(services).await, 7);
    let day = day(services).await;
    assert_eq!((day.created, day.network, day.address), (7, limited, 0));
    assert!(limited > 0);
    register(services, "elsewhere@example.com", "2a00:1234:5679:1::1")
        .await
        .unwrap();
    register(services, "v4@example.com", "203.0.113.5")
        .await
        .unwrap();
    env.db.cleanup().await;
}

#[tokio::test]
async fn сорок_регистраций_разом_дают_ровно_предел_в_минуту() {
    let Some((env, ..)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    limits(services, serde_json::json!({ "site_minute": 5 })).await;
    early_in_the_minute(services, 20.0).await;
    let mut tasks = Vec::new();
    for n in 0..40 {
        let services = services.clone();
        tasks.push(tokio::spawn(async move {
            register(
                &services,
                &format!("rush{n}@example.com"),
                &format!("203.0.113.{}", n + 1),
            )
            .await
        }));
    }
    let mut created = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => created += 1,
            Err(err) => assert_eq!(err.code(), signups::BUSY_CODE),
        }
    }
    assert_eq!(created, 5);
    assert_eq!(made(services).await, 5);
    assert_eq!(day(services).await.minute, 35);
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE site_signups SET created_at = created_at - interval '1 minute'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    register(services, "next@example.com", "203.0.113.99")
        .await
        .unwrap();
    env.db.cleanup().await;
}

#[tokio::test]
async fn одновременные_регистрации_с_одного_адреса_не_проходят_сверх_предела() {
    let Some((env, ..)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    limits(services, serde_json::json!({ "site_address_day": 3 })).await;
    let mut tasks = Vec::new();
    for n in 0..40_i64 {
        let services = services.clone();
        tasks.push(tokio::spawn(async move {
            let mut tx = services.store().begin().await.unwrap();
            let Ok(admission) = signups::admit(&mut tx, Some("203.0.113.7"), false)
                .await
                .unwrap()
            else {
                return false;
            };
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO clients (locale, registration_source) VALUES ('ru', 'web') RETURNING id",
            )
            .fetch_one(tx.conn())
            .await
            .unwrap();
            tokio::time::sleep(Pause::from_millis((n % 5) as u64)).await;
            signups::record(
                &mut tx,
                admission,
                ClientId(id),
                signups::Via::Password,
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            true
        }));
    }
    let mut created = 0;
    for task in tasks {
        created += i64::from(task.await.unwrap());
    }
    assert_eq!(created, 3);
    assert_eq!(made(services).await, 3);
    env.db.cleanup().await;
}

#[tokio::test]
async fn каждый_путь_сайта_считается_и_упирается_в_предел_с_адреса() {
    let Some((env, _, letters)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    limits(services, serde_json::json!({ "site_address_day": 10 })).await;
    let ip = "203.0.113.40";
    register(services, "one@example.com", ip).await.unwrap();
    assert!(matches!(
        from_telegram(services, 41_001, ip).await,
        Entered::Ready(_, true)
    ));
    configure_mail(services, admin, &letters, "code", false).await;
    by_code(services, &letters, "two@example.com", ip)
        .await
        .unwrap();
    configure_mail(services, admin, &letters, "password", true).await;
    let started = services
        .auth()
        .email_signup("three@example.com", PASSWORD, &meta(ip), Arrival::default())
        .await
        .unwrap();
    let Registration::CodeSent(started) = started else {
        panic!("ожидалось письмо")
    };
    let code = last_code(&letters);
    services
        .auth()
        .email_signup_confirm("three@example.com", &started.signup_token, &code, &meta(ip))
        .await
        .unwrap();
    let rows: Vec<(String, String)> = {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query_as("SELECT via, address FROM site_signups ORDER BY id")
            .fetch_all(conn.conn())
            .await
            .unwrap()
    };
    assert_eq!(
        rows,
        ["password", "telegram", "code", "password"].map(|via| (via.to_string(), ip.to_string()))
    );
    limits(services, serde_json::json!({ "site_address_day": 4 })).await;
    forget_tries(services).await;
    let before = made(services).await;
    assert_eq!(
        from_telegram(services, 41_002, ip).await,
        Entered::Refused(Refusal::Address)
    );
    let started = services
        .auth()
        .email_signup("four@example.com", PASSWORD, &meta(ip), Arrival::default())
        .await
        .unwrap();
    let Registration::CodeSent(started) = started else {
        panic!("ожидалось письмо")
    };
    let confirm = last_code(&letters);
    for _ in 0..2 {
        assert_eq!(
            super::code(
                services
                    .auth()
                    .email_signup_confirm(
                        "four@example.com",
                        &started.signup_token,
                        &confirm,
                        &meta(ip)
                    )
                    .await
            ),
            signups::LIMIT_CODE
        );
    }
    configure_mail(services, admin, &letters, "code", false).await;
    assert_eq!(
        super::code(by_code(services, &letters, "five@example.com", ip).await),
        signups::LIMIT_CODE
    );
    configure_mail(services, admin, &letters, "password", false).await;
    assert_eq!(
        super::code(register(services, "six@example.com", ip).await),
        signups::LIMIT_CODE
    );
    assert_eq!(made(services).await, before);
    assert_eq!(day(services).await.address, 5);
    limits(services, serde_json::json!({ "site_address_day": 5 })).await;
    services
        .auth()
        .email_signup_confirm(
            "four@example.com",
            &started.signup_token,
            &confirm,
            &meta(ip),
        )
        .await
        .expect("код остаётся годным, когда предел подняли");
    env.db.cleanup().await;
}

async fn configure_mail(
    services: &Services,
    admin: AdminId,
    letters: &Letters,
    login: &str,
    confirm: bool,
) {
    let _ = letters;
    services
        .settings_ops()
        .update_module_config(
            ModuleId::Mail,
            &ModuleConfigUpdate {
                config: serde_json::json!({ "login": login, "confirm_signup": confirm })
                    .as_object()
                    .cloned()
                    .unwrap(),
                clear: Vec::new(),
            },
            admin,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn существующие_аккаунты_входят_сверх_всех_пределов() {
    let Some((env, _, letters)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let ip = "2a00:aaaa:bbbb:1::5";
    register(services, "old@example.com", ip).await.unwrap();
    let Entered::Ready(telegram, true) = from_telegram(services, 42_001, ip).await else {
        panic!("клиент не создан")
    };
    limits(
        services,
        serde_json::json!({ "site_address_day": 1, "site_network_day": 1, "site_minute": 1 }),
    )
    .await;
    filled(
        services,
        "2a00:aaaa:bbbb:1::/64",
        Some("2a00:aaaa:bbbb::/48"),
        50,
    )
    .await;
    forget_tries(services).await;
    assert_eq!(
        super::code(register(services, "new@example.com", ip).await),
        signups::LIMIT_CODE
    );
    assert_eq!(
        super::code(register(services, "old@example.com", ip).await),
        "auth.email_taken"
    );
    let auth = services.auth();
    auth.email_password_login("old@example.com", PASSWORD, &meta(ip))
        .await
        .unwrap();
    assert!(matches!(
        from_telegram(services, 42_001, ip).await,
        Entered::Ready(client, false) if client.id == telegram.id
    ));
    let start = auth.telegram_login_start(&meta(ip)).await.unwrap();
    let param = start.link.rsplit_once("?start=").unwrap().1.to_string();
    let confirmed = auth
        .confirm_telegram_login(&param, &profile(42_001))
        .await
        .unwrap();
    assert!(!confirmed.created);
    configure_mail(services, admin, &letters, "code", false).await;
    by_code(services, &letters, "old@example.com", ip)
        .await
        .unwrap();
    let day = day(services).await;
    assert_eq!((day.address, day.network, day.minute), (0, 1, 0));
    env.db.cleanup().await;
}

#[tokio::test]
async fn ноль_и_выключенный_модуль_ничего_не_меняют() {
    let Some((env, admin, _)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    let ip = "203.0.113.60";
    for n in 0..5 {
        register(services, &format!("free{n}@example.com"), ip)
            .await
            .unwrap();
    }
    assert_eq!(
        number(services, "SELECT count(*) FROM site_signups").await,
        5
    );
    limits(
        services,
        serde_json::json!({ "site_address_day": 1, "site_network_day": 1, "site_minute": 1 }),
    )
    .await;
    services
        .settings_ops()
        .toggle_module(ModuleId::Antibot, false, admin)
        .await
        .unwrap();
    forget_tries(services).await;
    for n in 0..3 {
        register(services, &format!("off{n}@example.com"), ip)
            .await
            .unwrap();
    }
    assert!(matches!(
        from_telegram(services, 43_001, ip).await,
        Entered::Ready(_, true)
    ));
    assert_eq!(
        number(services, "SELECT count(*) FROM site_signups").await,
        5
    );
    assert_eq!(day(services).await.address, 0);
    let defaults = antibot::AntibotSettings::default();
    assert_eq!(
        (
            defaults.site_address_day,
            defaults.site_network_day,
            defaults.site_minute
        ),
        (0, 0, 0)
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn две_копии_панели_считают_вместе() {
    let Some((env, admin, _)) = site("password", false).await else {
        return;
    };
    let first = &env.services;
    let second = Services::new(ServiceDeps {
        store: env.db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(
            ModuleRegistry::new(vec![
                Arc::new(MailModule) as Arc<dyn Module>,
                Arc::new(AntibotModule),
            ])
            .unwrap(),
        ),
        secrets: None,
        clock: env.clock.clone(),
        env: EnvFallbacks::default(),
    });
    limits(first, serde_json::json!({ "site_address_day": 3 })).await;
    let _ = admin;
    second.reload().await.unwrap();
    let ip = "203.0.113.70";
    let mut created = 0;
    for n in 0..5 {
        let services = if n % 2 == 0 { first } else { &second };
        match register(services, &format!("copy{n}@example.com"), ip).await {
            Ok(_) => created += 1,
            Err(err) => assert_eq!(err.code(), signups::LIMIT_CODE),
        }
    }
    assert_eq!(created, 3);
    let seen = (day(first).await, day(&second).await);
    assert_eq!(seen.0, seen.1);
    assert_eq!(seen.0.address, 2);
    env.db.cleanup().await;
}

#[tokio::test]
async fn сверх_минутного_предела_просят_капчу_если_она_настроена() {
    let Some((env, admin, letters)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    let verify = siteverify(services).await;
    captcha(services, admin, serde_json::json!({ "site_minute": 1 })).await;
    early_in_the_minute(services, 30.0).await;
    let ip = "203.0.113.80";
    register(services, "calm@example.com", ip).await.unwrap();
    assert_eq!(verify.calls(), 0, "до предела капчу не спрашивают");
    assert_eq!(
        super::code(register(services, "hot@example.com", ip).await),
        antibot::CAPTCHA_REQUIRED
    );
    assert_eq!(
        super::code(register_with(services, "hot@example.com", ip, Some("ok:login")).await),
        antibot::FAILED
    );
    register_with(services, "hot@example.com", ip, Some("ok:signup"))
        .await
        .unwrap();
    assert_eq!(
        super::code(
            register_with(
                services,
                "again@example.com",
                "203.0.113.81",
                Some("ok:signup")
            )
            .await
        ),
        antibot::FAILED,
        "ответ капчи одноразовый"
    );
    assert_eq!(
        super::code(
            register_with(services, "down@example.com", "203.0.113.82", Some("down")).await
        ),
        antibot::UNAVAILABLE
    );
    configure_mail(services, admin, &letters, "code", false).await;
    let auth = services.auth();
    let who = meta("203.0.113.83");
    auth.email_login_start("late@example.com", &who)
        .await
        .unwrap();
    let code = last_code(&letters);
    assert_eq!(
        super::code(
            auth.email_login_verify("late@example.com", &code, &who)
                .await
        ),
        antibot::CAPTCHA_REQUIRED
    );
    auth.email_login_verify_from(
        "late@example.com",
        &code,
        &who,
        Arrival::default(),
        Some("ok:signup:late"),
    )
    .await
    .unwrap();
    auth.email_login_start("late@example.com", &who)
        .await
        .unwrap();
    let code = last_code(&letters);
    auth.email_login_verify("late@example.com", &code, &who)
        .await
        .expect("вход в существующий аккаунт капчу не спрашивает");
    let Entered::Refused(Refusal::Minute) = from_telegram(services, 44_001, "203.0.113.84").await
    else {
        panic!("вход через Telegram ждёт следующей минуты")
    };
    let day = day(services).await;
    assert_eq!((day.created, day.asked, day.minute), (3, 2, 1));
    env.db.cleanup().await;
}

#[tokio::test]
async fn капча_на_каждую_регистрацию_уже_снимает_минутный_предел() {
    let Some((env, admin, _)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    let verify = siteverify(services).await;
    captcha(
        services,
        admin,
        serde_json::json!({ "site_minute": 1, "captcha_signup": true }),
    )
    .await;
    early_in_the_minute(services, 40.0).await;
    for n in 0..3 {
        register_with(
            services,
            &format!("always{n}@example.com"),
            "203.0.113.90",
            Some(&format!("ok:signup:{n}")),
        )
        .await
        .unwrap();
    }
    assert_eq!(verify.calls(), 3);
    assert_eq!(day(services).await.minute, 0);
    env.db.cleanup().await;
}

#[tokio::test]
async fn без_капчи_сверх_минутного_предела_отказ_до_следующей_минуты() {
    let Some((env, _, letters)) = site("code", false).await else {
        return;
    };
    let services = &env.services;
    limits(services, serde_json::json!({ "site_minute": 1 })).await;
    early_in_the_minute(services, 40.0).await;
    by_code(services, &letters, "quick@example.com", "203.0.113.100")
        .await
        .unwrap();
    let auth = services.auth();
    let who = meta("203.0.113.101");
    auth.email_login_start("slow@example.com", &who)
        .await
        .unwrap();
    let code = last_code(&letters);
    let refused = auth
        .email_login_verify("slow@example.com", &code, &who)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), signups::BUSY_CODE);
    assert!(
        refused.to_string().chars().all(|c| !c.is_ascii_digit()),
        "{refused}"
    );
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE site_signups SET created_at = created_at - interval '1 minute'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    auth.email_login_verify("slow@example.com", &code, &who)
        .await
        .expect("код не сгорает от отказа");
    let failures = number(
        services,
        "SELECT coalesce(sum(hits), 0)::bigint FROM auth_rate_limits \
         WHERE window_start = '2026-11-16 00:00:00+00'",
    )
    .await;
    assert_eq!(failures, 0, "верный код не остаётся в счёте неверных");
    env.db.cleanup().await;
}

#[tokio::test]
async fn адреса_cloudflare_и_внутренние_в_суточных_пределах_не_считаются() {
    let Some((env, ..)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    limits(
        services,
        serde_json::json!({ "site_address_day": 1, "site_network_day": 1 }),
    )
    .await;
    for (n, ip) in [
        "172.68.10.20",
        "172.68.10.20",
        "172.68.10.20",
        "2606:4700::1111",
        "2606:4700::1112",
    ]
    .into_iter()
    .enumerate()
    {
        register(services, &format!("edge{n}@example.com"), ip)
            .await
            .unwrap();
    }
    let hidden = {
        let mut conn = services.store().conn().await.unwrap();
        signups::hidden(&mut conn).await.unwrap().unwrap()
    };
    assert_eq!(hidden.kind, Hidden::Cloudflare);
    assert_eq!(hidden.address, "172.68.10.20");
    for n in 0..3 {
        register(services, &format!("inner{n}@example.com"), "172.18.0.5")
            .await
            .unwrap();
    }
    let hidden = {
        let mut conn = services.store().conn().await.unwrap();
        signups::hidden(&mut conn).await.unwrap().unwrap()
    };
    assert_eq!(hidden.kind, Hidden::Internal);
    register(services, "real@example.com", "198.51.100.9")
        .await
        .unwrap();
    assert_eq!(
        super::code(register(services, "real2@example.com", "198.51.100.9").await),
        signups::LIMIT_CODE
    );
    assert_eq!(
        number(
            services,
            "SELECT count(*) FROM site_signups WHERE address IS NULL AND network IS NULL"
        )
        .await,
        8
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn суточный_предел_отпускает_через_сутки_и_уборка_чистит_старое() {
    let Some((env, ..)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    limits(services, serde_json::json!({ "site_address_day": 2 })).await;
    let ip = "203.0.113.120";
    filled(services, ip, None, 2).await;
    for shift in ["0 hours", "23 hours 50 minutes"] {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(&format!(
            "UPDATE site_signups SET created_at = now() - interval '{shift}'"
        ))
        .execute(conn.conn())
        .await
        .unwrap();
        drop(conn);
        assert_eq!(
            super::code(register(services, "wait@example.com", ip).await),
            signups::LIMIT_CODE,
            "{shift}"
        );
    }
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE site_signups SET created_at = now() - interval '24 hours 1 minute'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    register(services, "wait@example.com", ip).await.unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE site_signups SET created_at = now() - interval '49 hours' WHERE client_id IS NULL")
            .execute(conn.conn())
            .await
            .unwrap();
        sqlx::query("UPDATE site_signup_refusals SET hour = hour - interval '8 days'")
            .execute(conn.conn())
            .await
            .unwrap();
        signups::sweep(&mut conn).await.unwrap();
    }
    assert_eq!(
        number(services, "SELECT count(*) FROM site_signups").await,
        1
    );
    assert_eq!(
        number(services, "SELECT count(*) FROM site_signup_refusals").await,
        0
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn бот_и_мини_приложение_пределами_не_задеты() {
    let Some((env, ..)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    limits(
        services,
        serde_json::json!({ "site_address_day": 1, "site_network_day": 1, "site_minute": 1 }),
    )
    .await;
    for id in 45_001..45_006 {
        let (_, created) = services
            .clients()
            .ensure_telegram(&profile(id), RegistrationSource::Bot, None)
            .await
            .unwrap();
        assert!(created);
    }
    for id in 45_101..45_106 {
        let (_, created) = services
            .clients()
            .ensure_telegram(&profile(id), RegistrationSource::Miniapp, None)
            .await
            .unwrap();
        assert!(created);
    }
    assert_eq!(
        number(services, "SELECT count(*) FROM site_signups").await,
        0
    );
    env.db.cleanup().await;
}
