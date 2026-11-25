use super::mail::{Letters, code_in, configure_with, fake_smtp, readable};
use super::site_limits::limits;
use super::*;
use crate::limits::RateRule;
use crate::login::EMAIL_UNCONFIRMED;
use crate::signup::{Arrival, Registration};
use crate::two_factor::CabinetLogin;
use sn_domain::client::NewIdentity;
use sn_modules::catalog::antibot::AntibotModule;
use sn_modules::catalog::antibot::signups;
use sn_modules::catalog::mail::MailModule;
use sn_store::{Store, StoreOptions};
use std::time::Duration as Wait;

fn registry() -> ModuleRegistry {
    let modules: Vec<Arc<dyn Module>> = vec![Arc::new(MailModule), Arc::new(AntibotModule)];
    ModuleRegistry::new(modules).unwrap()
}

async fn site() -> Option<(Env, AdminId, Letters, u16)> {
    let env = env_with(registry()).await?;
    let admin = owner(&env.services).await;
    let (port, letters) = fake_smtp().await;
    by_mail(&env.services, admin, port, "password", false).await;
    Some((env, admin, letters, port))
}

async fn by_mail(services: &Services, admin: AdminId, port: u16, login: &str, confirm: bool) {
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": login, "confirm_signup": confirm }),
    )
    .await;
}

async fn one_connection(env: &Env) -> (Store, Services) {
    let store = Store::connect(
        &sn_store::testing::test_database_url().unwrap(),
        StoreOptions {
            max_connections: 1,
            acquire_timeout: Wait::from_secs(2),
            search_path: Some(env.db.schema().to_string()),
            application_name: env.db.schema().to_string(),
            ..StoreOptions::default()
        },
    )
    .await
    .unwrap();
    let services = Services::new(ServiceDeps {
        store: store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(registry()),
        secrets: env.services.secrets().cloned(),
        clock: env.clock.clone(),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    (store, services)
}

fn last_code(letters: &Letters) -> String {
    code_in(&readable(letters.lock().unwrap().last().unwrap()))
}

fn other(code: &str) -> &'static str {
    if code == "000000" { "111111" } else { "000000" }
}

async fn failures(services: &Services, rule: RateRule, identity: &str) -> u32 {
    services.limits().failures(rule, identity).await.unwrap()
}

async fn wrong_codes(services: &Services, email: &str) -> u32 {
    failures(services, rules::EMAIL_CODE_FAILURES_DAY, email).await
}

async fn client_of(services: &Services, email: &str) -> ClientId {
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .find_by_identity(IdentityKind::Email, email)
        .await
        .unwrap()
        .unwrap()
        .id
}

#[tokio::test]
async fn сброс_пароля_обходится_одним_соединением() {
    let Some((env, _, letters, _)) = site().await else {
        return;
    };
    let (store, squeezed) = one_connection(&env).await;
    for (n, services) in [(1u8, &env.services), (2, &squeezed)] {
        let auth = services.auth();
        let place = if n == 1 {
            "обычный пул"
        } else {
            "одно соединение"
        };
        let olga = format!("olga{n}@example.com");
        let who = meta(&format!("203.0.113.1{n}"));
        auth.email_register(&olga, "пароль-ольги-1", &who)
            .await
            .unwrap();
        auth.email_reset_start(&olga, &who).await.unwrap();
        let sent = last_code(&letters);
        assert_eq!(
            code(
                auth.email_reset_confirm(&olga, other(&sent), "новый-пароль-оли", &who)
                    .await
            ),
            "auth.code_invalid",
            "{place}"
        );
        let login = auth
            .email_reset_confirm(&olga, &sent, "новый-пароль-оли", &who)
            .await
            .unwrap_or_else(|err| panic!("{place}: {err}"));
        assert!(matches!(login, CabinetLogin::Session(_)), "{place}");
        assert_eq!(wrong_codes(services, &olga).await, 1, "{place}");
        auth.email_password_login(&olga, "новый-пароль-оли", &who)
            .await
            .unwrap();

        let boris = format!("boris{n}@example.com");
        let who = meta(&format!("203.0.113.2{n}"));
        auth.email_register(&boris, "пароль-бориса-1", &who)
            .await
            .unwrap();
        auth.email_reset_start(&boris, &who).await.unwrap();
        let sent = last_code(&letters);
        let blocked = client_of(services, &boris).await;
        services
            .clients()
            .block(blocked, Some("проверка"), Actor::System)
            .await
            .unwrap();
        assert_eq!(
            code(
                auth.email_reset_confirm(&boris, &sent, "новый-пароль-бориса", &who)
                    .await
            ),
            "auth.forbidden",
            "{place}"
        );
        assert_eq!(wrong_codes(services, &boris).await, 0, "{place}");
        services
            .clients()
            .unblock(blocked, Actor::System)
            .await
            .unwrap();
        auth.email_password_login(&boris, "пароль-бориса-1", &who)
            .await
            .unwrap();

        let anna = format!("anna{n}@example.com");
        let who = meta(&format!("203.0.113.3{n}"));
        auth.email_register(&anna, "пароль-анны-1", &who)
            .await
            .unwrap();
        auth.email_reset_start(&anna, &who).await.unwrap();
        let sent = last_code(&letters);
        let moved = client_of(services, &anna).await;
        {
            let mut conn = services.store().conn().await.unwrap();
            sqlx::query("DELETE FROM client_identities WHERE client_id = $1")
                .bind(moved.0)
                .execute(conn.conn())
                .await
                .unwrap();
        }
        assert_eq!(
            code(
                auth.email_reset_confirm(&anna, &sent, "новый-пароль-анны", &who)
                    .await
            ),
            "auth.code_invalid",
            "{place}"
        );
        assert_eq!(wrong_codes(services, &anna).await, 0, "{place}");

        let vera = format!("vera{n}@example.com");
        let who = meta(&format!("203.0.113.4{n}"));
        let linked = telegram_client(services, 5_600_000 + i64::from(n)).await;
        {
            let mut conn = services.store().conn().await.unwrap();
            conn.clients()
                .add_identity(linked, &NewIdentity::email_without_code(&vera).unwrap())
                .await
                .unwrap();
        }
        auth.email_reset_start(&vera, &who).await.unwrap();
        let sent = last_code(&letters);
        assert_eq!(
            code(
                auth.email_reset_confirm(&vera, &sent, "захват-аккаунта-1", &who)
                    .await
            ),
            EMAIL_UNCONFIRMED,
            "{place}"
        );
        assert_eq!(wrong_codes(services, &vera).await, 0, "{place}");
        assert!(!auth.has_password(linked).await.unwrap(), "{place}");
    }
    store.close().await;
    env.db.cleanup().await;
}

#[tokio::test]
async fn вход_админа_с_кодом_обходится_одним_соединением() {
    let Some((env, admin, _, _)) = site().await else {
        return;
    };
    let (store, services) = one_connection(&env).await;
    let auth = services.auth();
    let office = meta("203.0.113.51");
    let id = admin.to_string();
    let now = start();
    let setup = auth.admin_totp_setup(admin).await.unwrap();
    let at = |seconds| totp_code(&setup.secret, now + Duration::seconds(seconds));
    auth.admin_totp_enable(admin, &setup.secret, &at(0), PASSWORD)
        .await
        .unwrap();
    assert!(matches!(
        auth.admin_login("owner@example.com", "чужой-пароль-123", None, &office)
            .await,
        Err(ServiceError::Unauthorized)
    ));
    for (given, expected) in [(None, TOTP_REQUIRED), (Some(at(0)), TOTP_INVALID)] {
        assert_eq!(
            code(
                auth.admin_login("owner@example.com", PASSWORD, given.as_deref(), &office)
                    .await
            ),
            expected
        );
    }
    let password = failures(&services, rules::ADMIN_LOGIN_ACCOUNT, "owner@example.com").await;
    assert_eq!(password, 1, "верный пароль засчитан как неверный");
    assert_eq!(
        failures(&services, rules::ADMIN_TOTP_FAILURES, &id).await,
        1
    );
    env.clock.set(now + Duration::seconds(30));
    auth.admin_login("owner@example.com", PASSWORD, Some(&at(30)), &office)
        .await
        .unwrap();
    assert_eq!(
        failures(&services, rules::ADMIN_TOTP_FAILURES, &id).await,
        1
    );
    assert_eq!(
        code(auth.admin_totp_disable(admin, PASSWORD, &at(30)).await),
        "auth.totp_code"
    );
    let limits = services.limits();
    for _ in 0..rules::ADMIN_TOTP_FAILURES_DAY.limit {
        let _ = limits
            .strike(rules::ADMIN_TOTP_FAILURES_DAY, &id, "перебор")
            .await;
    }
    let before = failures(&services, rules::ADMIN_TOTP_FAILURES, &id).await;
    env.clock.set(now + Duration::seconds(60));
    assert_eq!(
        code(
            auth.admin_login("owner@example.com", PASSWORD, Some(&at(60)), &office)
                .await
        ),
        RATE_LIMITED
    );
    assert_eq!(
        failures(&services, rules::ADMIN_TOTP_FAILURES, &id).await,
        before
    );
    for rule in [rules::ADMIN_TOTP_FAILURES, rules::ADMIN_TOTP_FAILURES_DAY] {
        limits.forget(rule, &id).await.unwrap();
    }
    auth.admin_totp_disable(admin, PASSWORD, &at(60))
        .await
        .unwrap();
    for _ in 0..rules::ADMIN_LOGIN_ACCOUNT.limit {
        let _ = limits
            .strike(rules::ADMIN_LOGIN_ACCOUNT, "owner@example.com", "перебор")
            .await;
    }
    assert_eq!(
        code(
            auth.admin_login("owner@example.com", PASSWORD, None, &meta("198.51.100.77"))
                .await
        ),
        RATE_LIMITED
    );
    auth.admin_login("owner@example.com", PASSWORD, None, &office)
        .await
        .unwrap();
    store.close().await;
    env.db.cleanup().await;
}

async fn challenge(services: &Services, email: &str, who: &RequestMeta) -> String {
    let auth = services.auth();
    let (sent, _) = auth.issue_email_code(email, who).await.unwrap();
    match auth.email_login_verify(email, &sent, who).await.unwrap() {
        CabinetLogin::TotpRequired(asked) => asked.challenge,
        CabinetLogin::Session(_) => panic!("вход должен попросить код из приложения"),
    }
}

#[tokio::test]
async fn второй_фактор_клиента_обходится_одним_соединением() {
    let Some((env, _, _, _)) = site().await else {
        return;
    };
    let (store, services) = one_connection(&env).await;
    let auth = services.auth();
    let client = telegram_client(&services, 5_600_101).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .add_identity(
                client,
                &NewIdentity::new(IdentityKind::Email, "lida@example.com", true).unwrap(),
            )
            .await
            .unwrap();
    }
    let id = client.to_string();
    let now = start();
    let setup = auth.client_totp_setup(client).await.unwrap();
    let at = |seconds| totp_code(&setup.secret, now + Duration::seconds(seconds));
    auth.client_totp_enable(client, &setup.secret, &at(0))
        .await
        .unwrap();
    let home = meta("203.0.113.61");
    let mail = "lida@example.com";
    let first = challenge(&services, mail, &home).await;
    assert_eq!(
        code(auth.cabinet_totp_login(&first, &at(0), &home).await),
        "auth.totp_code"
    );
    assert_eq!(
        failures(&services, rules::CLIENT_TOTP_FAILURES, &id).await,
        1
    );
    env.clock.set(now + Duration::seconds(30));
    let code_now = at(30);
    let (one, two) = tokio::join!(
        auth.cabinet_totp_login(&first, &code_now, &home),
        auth.cabinet_totp_login(&first, &code_now, &home),
    );
    let refused: Vec<&str> = [one, two]
        .into_iter()
        .filter_map(|login| login.err())
        .map(|err| err.code())
        .collect();
    assert_eq!(refused, ["auth.login_expired"]);
    assert_eq!(
        failures(&services, rules::CLIENT_TOTP_FAILURES, &id).await,
        1
    );
    assert_eq!(
        code(auth.client_totp_disable(client, &at(30)).await),
        "auth.totp_code"
    );
    let wrong = failures(&services, rules::CLIENT_TOTP_FAILURES, &id).await;
    assert_eq!(wrong, 2);
    let second = challenge(&services, mail, &home).await;
    services
        .clients()
        .block(client, None, Actor::System)
        .await
        .unwrap();
    env.clock.set(now + Duration::seconds(60));
    assert_eq!(
        code(auth.cabinet_totp_login(&second, &at(60), &home).await),
        "auth.forbidden"
    );
    assert_eq!(
        failures(&services, rules::CLIENT_TOTP_FAILURES, &id).await,
        wrong
    );
    services
        .clients()
        .unblock(client, Actor::System)
        .await
        .unwrap();
    let off = auth.client_totp_disable(client, &at(60)).await.unwrap();
    assert!(!off.enabled);
    assert_eq!(
        failures(&services, rules::CLIENT_TOTP_FAILURES, &id).await,
        wrong
    );
    store.close().await;
    env.db.cleanup().await;
}

#[tokio::test]
async fn отказ_по_минутному_пределу_обходится_одним_соединением() {
    let Some((env, admin, letters, port)) = site().await else {
        return;
    };
    env.services
        .settings_ops()
        .toggle_module(ModuleId::Antibot, true, admin)
        .await
        .unwrap();
    limits(&env.services, serde_json::json!({ "site_minute": 1 })).await;
    {
        let mut conn = env.services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO site_signups (via, address, created_at) \
             VALUES ('password', '198.51.100.1', now()), ('password', '198.51.100.1', now() + interval '1 minute')",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    by_mail(&env.services, admin, port, "password", true).await;
    let (store, services) = one_connection(&env).await;
    let auth = services.auth();
    let who = meta("203.0.113.71");
    let Registration::CodeSent(pending) = auth
        .email_signup(
            "mila@example.com",
            "пароль-милы-1",
            &who,
            Arrival::default(),
        )
        .await
        .unwrap()
    else {
        panic!("ожидалось письмо с кодом");
    };
    let sent = last_code(&letters);
    assert_eq!(
        code(
            auth.email_signup_confirm("mila@example.com", &pending.signup_token, &sent, &who)
                .await
        ),
        signups::BUSY_CODE
    );
    assert_eq!(wrong_codes(&services, "mila@example.com").await, 0);
    by_mail(&env.services, admin, port, "code", false).await;
    services.reload().await.unwrap();
    let who = meta("203.0.113.72");
    auth.email_login_start("zoya@example.com", &who)
        .await
        .unwrap();
    let sent = last_code(&letters);
    assert_eq!(
        code(
            auth.email_login_verify("zoya@example.com", &sent, &who)
                .await
        ),
        signups::BUSY_CODE
    );
    assert_eq!(wrong_codes(&services, "zoya@example.com").await, 0);
    store.close().await;
    env.db.cleanup().await;
}
