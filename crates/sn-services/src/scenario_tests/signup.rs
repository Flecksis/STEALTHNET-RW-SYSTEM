use super::mail::{Letters, code_in, configure_with, fake_smtp, readable};
use super::*;
use crate::login::EMAIL_CODE_MAX_ATTEMPTS;
use crate::mail::SignupRule;
use crate::signup::{Arrival, Registration, SignupStart};
use crate::two_factor::CabinetLogin;
use sn_domain::client::{Languages, Locale, NewIdentity};
use sn_domain::lang;
use sn_modules::catalog::mail::MailModule;
use sn_modules::catalog::referral::{ReferralModule, ensure_code};
use sn_modules::catalog::start_param::Utm;
use sn_modules::catalog::utm::{UtmModule, for_client as origin_of};

async fn mail_env(extra: Vec<Arc<dyn Module>>, confirm: bool) -> Option<(Env, AdminId, Letters)> {
    let mut modules: Vec<Arc<dyn Module>> = vec![Arc::new(MailModule)];
    modules.extend(extra);
    let env = env_with(ModuleRegistry::new(modules).unwrap()).await?;
    let admin = owner(&env.services).await;
    let (port, letters) = fake_smtp().await;
    configure_with(
        &env.services,
        admin,
        port,
        serde_json::json!({ "login": "password", "confirm_signup": confirm }),
    )
    .await;
    Some((env, admin, letters))
}

fn sent(registration: Registration) -> SignupStart {
    match registration {
        Registration::CodeSent(start) => start,
        Registration::Login(_) => panic!("ожидалось письмо с кодом, а аккаунт создан сразу"),
    }
}

fn last_letter(letters: &Letters) -> String {
    readable(letters.lock().unwrap().last().expect("письмо не пришло"))
}

fn other(code: &str) -> &'static str {
    if code == "000000" { "111111" } else { "000000" }
}

async fn client_by_email(services: &Services, email: &str) -> Option<ClientId> {
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .find_by_identity(IdentityKind::Email, email)
        .await
        .unwrap()
        .map(|client| client.id)
}

async fn count(services: &Services, sql: &str) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(sql)
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

async fn with_email(services: &Services, client: ClientId, email: &str, password: Option<&str>) {
    let mut tx = services.store().begin().await.unwrap();
    tx.clients()
        .add_identity(
            client,
            &NewIdentity::new(IdentityKind::Email, email, true).unwrap(),
        )
        .await
        .unwrap();
    if let Some(password) = password {
        sqlx::query("INSERT INTO client_passwords (client_id, password_hash) VALUES ($1, $2)")
            .bind(client.0)
            .bind(crate::auth::hash_password(password).unwrap())
            .execute(tx.conn())
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn с_подтверждением_аккаунт_появляется_только_после_кода() {
    let Some((env, _, letters)) = mail_env(Vec::new(), true).await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.61");
    assert_eq!(services.mail().signup_rule(), SignupRule::Confirm);
    let clients = count(services, "SELECT count(*) FROM clients").await;
    assert_eq!(
        code(
            auth.email_register("nika@example.com", "пароль-ники-1", &who)
                .await
        ),
        "auth.email_confirm"
    );
    let first = sent(
        auth.email_signup(
            " Nika@Example.com ",
            "пароль-ники-1",
            &who,
            Arrival::default(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(first.email, "nika@example.com");
    assert_eq!(first.signup_token.len(), 64);
    assert_eq!(
        count(services, "SELECT count(*) FROM clients").await,
        clients
    );
    assert!(
        client_by_email(services, "nika@example.com")
            .await
            .is_none()
    );
    let (hash, taken): (Option<String>, bool) = {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query_as("SELECT password_hash, taken FROM email_signups WHERE email = $1")
            .bind("nika@example.com")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    let hash = hash.unwrap();
    assert!(hash.starts_with("$argon2id$") && !hash.contains("пароль-ники-1"));
    assert!(!taken);
    let letter = last_letter(&letters);
    assert!(letter.contains("Код для регистрации"), "{letter}");
    let code_sent = code_in(&letter);
    assert_eq!(
        code(
            auth.email_password_login("nika@example.com", "пароль-ники-1", &who)
                .await
        ),
        "auth.wrong_password"
    );
    assert_eq!(
        code(
            auth.email_signup_confirm(
                "nika@example.com",
                &first.signup_token,
                other(&code_sent),
                &who
            )
            .await
        ),
        "auth.code_invalid"
    );
    assert_eq!(
        code(
            auth.email_signup_confirm("nika@example.com", &"f".repeat(64), &code_sent, &who)
                .await
        ),
        "auth.signup_expired"
    );
    assert_eq!(
        code(
            auth.email_signup_confirm("other@example.com", &first.signup_token, &code_sent, &who)
                .await
        ),
        "auth.signup_expired"
    );
    assert_eq!(
        count(services, "SELECT count(*) FROM clients").await,
        clients
    );
    let CabinetLogin::Session(session) = auth
        .email_signup_confirm("nika@example.com", &first.signup_token, &code_sent, &who)
        .await
        .unwrap()
    else {
        panic!("ожидалась сессия");
    };
    assert!(session.created);
    let client = client_by_email(services, "nika@example.com").await.unwrap();
    assert_eq!(session.principal.client_id, client);
    assert_eq!(
        auth.client_session(&session.token).await.unwrap().client_id,
        client
    );
    let identities = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients().identities(client).await.unwrap()
    };
    assert!(
        identities
            .iter()
            .any(|i| i.kind == IdentityKind::Email && i.value == "nika@example.com" && i.verified)
    );
    auth.email_password_login("nika@example.com", "пароль-ники-1", &who)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.email_signup_confirm("nika@example.com", &first.signup_token, &code_sent, &who)
                .await
        ),
        "auth.signup_expired"
    );
    assert_eq!(
        count(services, "SELECT count(*) FROM email_signups").await,
        0
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn неверный_и_просроченный_код_не_создают_аккаунт() {
    let Some((env, _, letters)) = mail_env(Vec::new(), true).await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.62");
    let first = sent(
        auth.email_signup("lev@example.com", "пароль-льва-1", &who, Arrival::default())
            .await
            .unwrap(),
    );
    let first_code = code_in(&last_letter(&letters));
    for _ in 0..EMAIL_CODE_MAX_ATTEMPTS {
        assert_eq!(
            code(
                auth.email_signup_confirm(
                    "lev@example.com",
                    &first.signup_token,
                    other(&first_code),
                    &who
                )
                .await
            ),
            "auth.code_invalid"
        );
    }
    assert_eq!(
        code(
            auth.email_signup_confirm("lev@example.com", &first.signup_token, &first_code, &who)
                .await
        ),
        "auth.code_attempts"
    );
    let again = sent(
        auth.email_signup("lev@example.com", "пароль-льва-2", &who, Arrival::default())
            .await
            .unwrap(),
    );
    let again_code = code_in(&last_letter(&letters));
    assert_eq!(
        code(
            auth.email_signup_confirm("lev@example.com", &first.signup_token, &again_code, &who)
                .await
        ),
        "auth.signup_expired"
    );
    env.clock.set(start() + Duration::minutes(16));
    assert_eq!(
        code(
            auth.email_signup_confirm("lev@example.com", &again.signup_token, &again_code, &who)
                .await
        ),
        "auth.code_invalid"
    );
    env.clock.set(start() + Duration::minutes(31));
    assert_eq!(
        code(
            auth.email_signup_confirm("lev@example.com", &again.signup_token, &again_code, &who)
                .await
        ),
        "auth.signup_expired"
    );
    assert_eq!(
        code(
            auth.email_signup_resend("lev@example.com", &again.signup_token, &who)
                .await
        ),
        "auth.signup_expired"
    );
    assert!(client_by_email(services, "lev@example.com").await.is_none());
    env.db.cleanup().await;
}

#[tokio::test]
async fn код_регистрации_отправляется_ещё_раз() {
    let Some((env, _, letters)) = mail_env(Vec::new(), true).await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.63");
    let first = sent(
        auth.email_signup(
            "mila@example.com",
            "пароль-милы-1",
            &who,
            Arrival::default(),
        )
        .await
        .unwrap(),
    );
    let first_code = code_in(&last_letter(&letters));
    for _ in 0..2 {
        assert_eq!(
            code(
                auth.email_signup_confirm(
                    "mila@example.com",
                    &first.signup_token,
                    other(&first_code),
                    &who
                )
                .await
            ),
            "auth.code_invalid"
        );
    }
    assert_eq!(
        code(
            auth.email_signup_resend("mila@example.com", &"f".repeat(64), &who)
                .await
        ),
        "auth.signup_expired"
    );
    assert_eq!(letters.lock().unwrap().len(), 1);
    env.clock.set(start() + Duration::minutes(20));
    let again = auth
        .email_signup_resend("Mila@Example.com", &first.signup_token, &who)
        .await
        .unwrap();
    assert_eq!(again.signup_token, first.signup_token);
    assert_eq!(again.email, "mila@example.com");
    assert_eq!(again.expires_at, start() + Duration::minutes(35));
    assert_eq!(letters.lock().unwrap().len(), 2);
    let letter = last_letter(&letters);
    assert!(letter.contains("Код для регистрации"), "{letter}");
    let again_code = code_in(&letter);
    if again_code != first_code {
        assert_eq!(
            code(
                auth.email_signup_confirm(
                    "mila@example.com",
                    &first.signup_token,
                    &first_code,
                    &who
                )
                .await
            ),
            "auth.code_invalid"
        );
    }
    auth.email_signup_resend("mila@example.com", &first.signup_token, &who)
        .await
        .unwrap();
    auth.email_signup_resend("mila@example.com", &first.signup_token, &who)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.email_signup_resend("mila@example.com", &first.signup_token, &who)
                .await
        ),
        RATE_LIMITED
    );
    assert_eq!(letters.lock().unwrap().len(), 4);
    let last_code = code_in(&last_letter(&letters));
    for _ in 0..4 {
        assert_eq!(
            code(
                auth.email_signup_confirm(
                    "mila@example.com",
                    &first.signup_token,
                    other(&last_code),
                    &who
                )
                .await
            ),
            "auth.code_invalid"
        );
    }
    let login = auth
        .email_signup_confirm("mila@example.com", &first.signup_token, &last_code, &who)
        .await
        .unwrap();
    assert!(matches!(login, CabinetLogin::Session(ref s) if s.created));
    assert!(
        client_by_email(services, "mila@example.com")
            .await
            .is_some()
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn метка_реферал_и_язык_переходят_в_клиента_после_подтверждения() {
    let Some((env, admin, letters)) =
        mail_env(vec![Arc::new(UtmModule), Arc::new(ReferralModule)], true).await
    else {
        return;
    };
    let services = &env.services;
    let ops = services.settings_ops();
    for module in [ModuleId::Utm, ModuleId::Referral] {
        ops.toggle_module(module, true, admin).await.unwrap();
    }
    ops.update_languages(
        &Languages {
            english: true,
            default: Locale::Ru,
        },
        admin,
    )
    .await
    .unwrap();
    let referrer = telegram_client(services, 5_200_001).await;
    let invite = {
        let mut conn = services.store().conn().await.unwrap();
        ensure_code(&mut conn, referrer).await.unwrap()
    };
    let auth = services.auth();
    let who = meta("203.0.113.64");
    let tiktok = Utm::from_parts(Some("tiktok"), None, Some("osen"), None, None).unwrap();
    let lowered = invite.to_lowercase();
    let first = sent(
        lang::scope(
            Locale::En,
            auth.email_signup(
                "ada@example.com",
                "пароль-ады-1",
                &who,
                Arrival::site(Some(&tiktok), Some(&lowered)),
            ),
        )
        .await
        .unwrap(),
    );
    let letter = last_letter(&letters);
    assert!(letter.contains("finish signing up"), "{letter}");
    services.events().dispatch_pending(50).await.unwrap();
    assert_eq!(
        count(services, "SELECT count(*) FROM referral_links").await,
        0
    );
    assert_eq!(
        count(
            services,
            "SELECT count(*) FROM clients WHERE registration_source = 'web'"
        )
        .await,
        0
    );
    let CabinetLogin::Session(session) = auth
        .email_signup_confirm(
            "ada@example.com",
            &first.signup_token,
            &code_in(&letter),
            &who,
        )
        .await
        .unwrap()
    else {
        panic!("ожидалась сессия");
    };
    services.events().dispatch_pending(50).await.unwrap();
    let client = session.principal.client_id;
    let mut conn = services.store().conn().await.unwrap();
    let origin = origin_of(&mut conn, client).await.unwrap().unwrap();
    assert_eq!(origin.utm.source.as_deref(), Some("tiktok"));
    assert_eq!(origin.utm.campaign.as_deref(), Some("osen"));
    assert_eq!(origin.registration_source, RegistrationSource::Web);
    let linked: Option<i64> =
        sqlx::query_scalar("SELECT referrer_id FROM referral_links WHERE client_id = $1")
            .bind(client.0)
            .fetch_optional(conn.conn())
            .await
            .unwrap();
    assert_eq!(linked, Some(referrer.0));
    assert_eq!(conn.clients().get(client).await.unwrap().locale, Locale::En);
    drop(conn);
    env.db.cleanup().await;
}

#[tokio::test]
async fn занятая_почта_получает_подсказку_а_ответ_не_отличается() {
    let Some((env, _, letters)) = mail_env(Vec::new(), true).await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.65");
    let olga = telegram_client(services, 5_200_002).await;
    with_email(
        services,
        olga,
        "olga@example.com",
        Some("старый-пароль-оли"),
    )
    .await;
    let fresh = sent(
        auth.email_signup(
            "fresh@example.com",
            "пароль-новичка-1",
            &who,
            Arrival::default(),
        )
        .await
        .unwrap(),
    );
    let taken = sent(
        auth.email_signup(
            "Olga@Example.com",
            "пароль-захватчика-1",
            &meta("203.0.113.66"),
            Arrival::default(),
        )
        .await
        .unwrap(),
    );
    let keys = |start: &SignupStart| {
        let value = serde_json::to_value(start).unwrap();
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&fresh), keys(&taken));
    assert_eq!(serde_json::to_value(&taken).unwrap()["status"], "code_sent");
    assert_eq!(taken.email, "olga@example.com");
    assert_eq!(fresh.signup_token.len(), taken.signup_token.len());
    assert_eq!(fresh.expires_at, taken.expires_at);
    let letter = last_letter(&letters);
    assert!(letter.contains("с этой почтой уже есть"), "{letter}");
    assert!(letter.contains("Забыли пароль?"), "{letter}");
    assert!(!letter.contains("Код для регистрации"), "{letter}");
    let (flag, hash): (bool, Option<String>) = {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query_as("SELECT taken, password_hash FROM email_signups WHERE email = $1")
            .bind("olga@example.com")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    assert!(flag && hash.is_none());
    for guess in ["000000", "482913"] {
        assert_eq!(
            code(
                auth.email_signup_confirm("olga@example.com", &taken.signup_token, guess, &who)
                    .await
            ),
            "auth.code_invalid"
        );
    }
    let resent = auth
        .email_signup_resend("olga@example.com", &taken.signup_token, &who)
        .await
        .unwrap();
    assert_eq!(keys(&resent), keys(&fresh));
    assert!(last_letter(&letters).contains("с этой почтой уже есть"));
    auth.email_password_login("olga@example.com", "старый-пароль-оли", &who)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.email_password_login("olga@example.com", "пароль-захватчика-1", &who)
                .await
        ),
        "auth.wrong_password"
    );
    let late = sent(
        auth.email_signup(
            "late@example.com",
            "пароль-поздний-1",
            &who,
            Arrival::default(),
        )
        .await
        .unwrap(),
    );
    let late_code = code_in(&last_letter(&letters));
    let other_client = telegram_client(services, 5_200_003).await;
    with_email(services, other_client, "late@example.com", None).await;
    assert_eq!(
        code(
            auth.email_signup_confirm("late@example.com", &late.signup_token, &late_code, &who)
                .await
        ),
        "auth.email_taken"
    );
    assert_eq!(
        client_by_email(services, "late@example.com").await,
        Some(other_client)
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn без_подтверждения_регистрация_по_паролю_как_раньше() {
    let Some((env, admin, letters)) = mail_env(vec![Arc::new(ReferralModule)], false).await else {
        return;
    };
    let services = &env.services;
    services
        .settings_ops()
        .toggle_module(ModuleId::Referral, true, admin)
        .await
        .unwrap();
    assert_eq!(services.mail().signup_rule(), SignupRule::Open);
    let auth = services.auth();
    let who = meta("203.0.113.67");
    let referrer = telegram_client(services, 5_200_004).await;
    let invite = {
        let mut conn = services.store().conn().await.unwrap();
        ensure_code(&mut conn, referrer).await.unwrap()
    };
    let Registration::Login(CabinetLogin::Session(session)) = auth
        .email_signup(
            "ivan@example.com",
            "пароль-ивана-1",
            &who,
            Arrival::site(None, Some(&invite)),
        )
        .await
        .unwrap()
    else {
        panic!("без подтверждения аккаунт создаётся сразу");
    };
    assert!(session.created);
    assert!(letters.lock().unwrap().is_empty());
    let client = client_by_email(services, "ivan@example.com").await.unwrap();
    assert_eq!(session.principal.client_id, client);
    let identities = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients().identities(client).await.unwrap()
    };
    assert!(
        identities
            .iter()
            .any(|i| i.value == "ivan@example.com" && !i.verified)
    );
    auth.email_register("petr@example.com", "пароль-петра-1", &who)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.email_register("ivan@example.com", "другой-пароль-1", &who)
                .await
        ),
        "auth.email_taken"
    );
    services.events().dispatch_pending(50).await.unwrap();
    let linked = count(
        services,
        &format!(
            "SELECT count(*) FROM referral_links WHERE client_id = {} AND referrer_id = {}",
            client.0, referrer.0
        ),
    )
    .await;
    assert_eq!(linked, 1);
    assert_eq!(
        count(services, "SELECT count(*) FROM email_signups").await,
        0
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn регистрация_закрыта_пока_почта_выключена_при_включённом_подтверждении() {
    let Some((env, admin, _)) = mail_env(Vec::new(), true).await else {
        return;
    };
    let services = &env.services;
    let ops = services.settings_ops();
    let auth = services.auth();
    let who = meta("203.0.113.68");
    ops.toggle_module(ModuleId::Mail, false, admin)
        .await
        .unwrap();
    assert_eq!(services.mail().signup_rule(), SignupRule::Closed);
    assert_eq!(
        code(
            auth.email_signup("zoya@example.com", "пароль-зои-1", &who, Arrival::default())
                .await
        ),
        "auth.signup_closed"
    );
    assert_eq!(
        code(
            auth.email_register("zoya@example.com", "пароль-зои-1", &who)
                .await
        ),
        "auth.signup_closed"
    );
    assert!(
        client_by_email(services, "zoya@example.com")
            .await
            .is_none()
    );
    ops.toggle_module(ModuleId::Mail, true, admin)
        .await
        .unwrap();
    assert_eq!(services.mail().signup_rule(), SignupRule::Confirm);
    let mut refused = serde_json::Map::new();
    refused.insert("transport".into(), serde_json::json!("none"));
    let err = ops
        .update_module_config(
            ModuleId::Mail,
            &ModuleConfigUpdate {
                config: refused,
                clear: Vec::new(),
            },
            admin,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("SMTP или Resend"), "{err}");
    let mut by_code = serde_json::Map::new();
    by_code.insert("login".into(), serde_json::json!("code"));
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config: by_code,
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    assert_eq!(services.mail().signup_rule(), SignupRule::Open);
    assert_eq!(
        code(
            auth.email_signup("zoya@example.com", "пароль-зои-1", &who, Arrival::default())
                .await
        ),
        "auth.email_code_mode"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn письма_регистрации_считаются_в_общем_потолке_писем_с_кодами() {
    let Some((env, admin, letters)) = mail_env(Vec::new(), true).await else {
        return;
    };
    let services = &env.services;
    let mut ceiling = serde_json::Map::new();
    ceiling.insert("codes_per_hour".into(), serde_json::json!(10));
    services
        .settings_ops()
        .update_module_config(
            ModuleId::Mail,
            &ModuleConfigUpdate {
                config: ceiling,
                clear: Vec::new(),
            },
            admin,
        )
        .await
        .unwrap();
    let auth = services.auth();
    let mut first = None;
    for n in 0..10 {
        let start = sent(
            auth.email_signup(
                &format!("guest{n}@example.com"),
                "пароль-гостя-1",
                &meta(&format!("203.0.113.{}", 100 + n)),
                Arrival::default(),
            )
            .await
            .unwrap(),
        );
        first.get_or_insert(start);
    }
    assert_eq!(
        code(
            auth.email_signup(
                "guest10@example.com",
                "пароль-гостя-1",
                &meta("198.51.100.90"),
                Arrival::default(),
            )
            .await
        ),
        RATE_LIMITED
    );
    let first = first.unwrap();
    assert_eq!(
        code(
            auth.email_signup_resend(&first.email, &first.signup_token, &meta("198.51.100.91"))
                .await
        ),
        RATE_LIMITED
    );
    assert_eq!(letters.lock().unwrap().len(), 10);
    assert_eq!(
        count(services, "SELECT count(*) FROM email_signups").await,
        10
    );
    env.db.cleanup().await;
}

async fn captcha_env(login: &str, confirm: bool) -> Option<(Env, AdminId, Letters)> {
    let modules: Vec<Arc<dyn Module>> = vec![
        Arc::new(MailModule),
        Arc::new(sn_modules::catalog::antibot::AntibotModule),
    ];
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
    let ops = env.services.settings_ops();
    let mut config = serde_json::Map::new();
    config.insert("captcha".into(), serde_json::json!("turnstile"));
    config.insert(
        "captcha_site_key".into(),
        serde_json::json!("0x4AAAAAAAsite"),
    );
    config.insert(
        "captcha_secret".into(),
        serde_json::json!("0x4AAAAAAAsecret-key"),
    );
    config.insert("burst_count".into(), serde_json::json!(0));
    ops.update_module_config(
        ModuleId::Antibot,
        &ModuleConfigUpdate {
            config,
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Antibot, true, admin)
        .await
        .unwrap();
    Some((env, admin, letters))
}

#[tokio::test]
async fn регистрация_по_паролю_просит_капчу_и_проверяет_её_на_сервере() {
    let Some((env, admin, _letters)) = captcha_env("password", false).await else {
        return;
    };
    let services = &env.services;
    let stub = super::siteverify::siteverify(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.81");
    let signup = |captcha: Option<&'static str>, email: &'static str| {
        let auth = services.auth();
        let who = who.clone();
        async move {
            auth.email_signup_with(email, "пароль-гостя-1", &who, Arrival::default(), captcha)
                .await
        }
    };
    assert_eq!(
        signup(None, "guest@example.com").await.unwrap_err().code(),
        sn_modules::catalog::antibot::CAPTCHA_REQUIRED
    );
    assert_eq!(
        signup(Some("bad-1"), "guest@example.com")
            .await
            .unwrap_err()
            .code(),
        sn_modules::catalog::antibot::FAILED
    );
    assert_eq!(
        signup(Some("ok:trial:"), "guest@example.com")
            .await
            .unwrap_err()
            .code(),
        sn_modules::catalog::antibot::FAILED
    );
    assert_eq!(
        signup(Some("down-1"), "guest@example.com")
            .await
            .unwrap_err()
            .code(),
        sn_modules::catalog::antibot::UNAVAILABLE
    );
    assert!(
        client_by_email(services, "guest@example.com")
            .await
            .is_none()
    );
    assert_eq!(
        auth.email_register("old-api@example.com", "пароль-гостя-1", &who)
            .await
            .unwrap_err()
            .code(),
        sn_modules::catalog::antibot::CAPTCHA_REQUIRED
    );
    let registered = signup(Some("ok:signup:"), "guest@example.com")
        .await
        .unwrap();
    assert!(matches!(registered, Registration::Login(_)));
    assert!(
        client_by_email(services, "guest@example.com")
            .await
            .is_some()
    );
    let sent = stub.last();
    assert_eq!(sent["secret"], "0x4AAAAAAAsecret-key");
    assert_eq!(sent["remoteip"], "203.0.113.81");
    assert_eq!(
        signup(Some("ok:signup:"), "second@example.com")
            .await
            .unwrap_err()
            .code(),
        sn_modules::catalog::antibot::FAILED
    );
    let ops = services.settings_ops();
    let mut off = serde_json::Map::new();
    off.insert("captcha_signup".into(), serde_json::json!(false));
    ops.update_module_config(
        ModuleId::Antibot,
        &ModuleConfigUpdate {
            config: off,
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    signup(None, "third@example.com").await.unwrap();
    env.db.cleanup().await;
}

#[tokio::test]
async fn вход_кодом_просит_капчу_перед_письмом() {
    let Some((env, _, letters)) = captcha_env("code", false).await else {
        return;
    };
    let services = &env.services;
    let _stub = super::siteverify::siteverify(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.82");
    assert_eq!(
        auth.email_login_start("coder@example.com", &who)
            .await
            .unwrap_err()
            .code(),
        sn_modules::catalog::antibot::CAPTCHA_REQUIRED
    );
    assert!(letters.lock().unwrap().is_empty());
    auth.email_login_start_with("coder@example.com", &who, Some("ok:login:"))
        .await
        .unwrap();
    assert_eq!(letters.lock().unwrap().len(), 1);
    env.db.cleanup().await;
}

#[tokio::test]
async fn секрет_капчи_хранится_зашифрованным_и_при_смене_сервиса_просится_заново() {
    let Some((env, admin, _)) = captcha_env("password", false).await else {
        return;
    };
    let services = &env.services;
    let stored: String = count_text(
        services,
        "SELECT config::text FROM module_states WHERE module = 'antibot'",
    )
    .await;
    assert!(!stored.contains("0x4AAAAAAAsecret-key"), "{stored}");
    let sealed: String = count_text(
        services,
        "SELECT encode(ciphertext, 'escape') FROM secrets WHERE key LIKE '%captcha_secret%'",
    )
    .await;
    assert!(!sealed.is_empty());
    assert!(!sealed.contains("0x4AAAAAAAsecret-key"));
    assert_eq!(
        services
            .modules()
            .config(ModuleId::Antibot)
            .text("captcha_secret"),
        Some("0x4AAAAAAAsecret-key")
    );
    let view = services.settings_ops().module_config(ModuleId::Antibot);
    assert_eq!(
        view.config["captcha_secret"],
        serde_json::json!({ "filled": true })
    );
    let ops = services.settings_ops();
    let mut switch = serde_json::Map::new();
    switch.insert("captcha".into(), serde_json::json!("yandex"));
    switch.insert("captcha_site_key".into(), serde_json::json!("ysc1_site"));
    let err = ops
        .update_module_config(
            ModuleId::Antibot,
            &ModuleConfigUpdate {
                config: switch.clone(),
                clear: Vec::new(),
            },
            admin,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), crate::settings::SECRETS_AGAIN);
    switch.insert("captcha_secret".into(), serde_json::json!("ysc2_secret"));
    ops.update_module_config(
        ModuleId::Antibot,
        &ModuleConfigUpdate {
            config: switch,
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    let mut no_secret = serde_json::Map::new();
    no_secret.insert("captcha".into(), serde_json::json!("turnstile"));
    no_secret.insert(
        "captcha_site_key".into(),
        serde_json::json!("0x4AAAAAAAsite"),
    );
    let err = ops
        .update_module_config(
            ModuleId::Antibot,
            &ModuleConfigUpdate {
                config: no_secret,
                clear: vec!["captcha_secret".into()],
            },
            admin,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "modules.config");
    env.db.cleanup().await;
}

async fn count_text(services: &Services, sql: &str) -> String {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(sql)
        .fetch_optional(conn.conn())
        .await
        .unwrap()
        .unwrap_or_default()
}

#[tokio::test]
async fn регистрация_с_подтверждением_почты_просит_капчу_только_на_первом_шаге() {
    let Some((env, _, letters)) = captcha_env("password", true).await else {
        return;
    };
    let services = &env.services;
    let _stub = super::siteverify::siteverify(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.83");
    assert_eq!(
        auth.email_signup(
            "mila@example.com",
            "пароль-милы-1",
            &who,
            Arrival::default()
        )
        .await
        .unwrap_err()
        .code(),
        sn_modules::catalog::antibot::CAPTCHA_REQUIRED
    );
    assert!(letters.lock().unwrap().is_empty());
    let start = sent(
        auth.email_signup_with(
            "mila@example.com",
            "пароль-милы-1",
            &who,
            Arrival::default(),
            Some("ok:signup:"),
        )
        .await
        .unwrap(),
    );
    assert!(
        client_by_email(services, "mila@example.com")
            .await
            .is_none()
    );
    let code = code_in(&last_letter(&letters));
    let login = auth
        .email_signup_confirm("mila@example.com", &start.signup_token, &code, &who)
        .await
        .unwrap();
    assert!(matches!(login, CabinetLogin::Session(_)));
    assert!(
        client_by_email(services, "mila@example.com")
            .await
            .is_some()
    );
    env.db.cleanup().await;
}
