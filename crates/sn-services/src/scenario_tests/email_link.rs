use super::mail::{Letters, code_in, configure_with, fake_smtp, readable};
use super::*;
use crate::email_change::{
    APP_CODE_REQUIRED, EMAIL_CHANGE_OFF, EMAIL_CODE_NEEDED, EMAIL_IN_USE, EMAIL_LINKED_AUDIT,
    EMAIL_SAME, EmailProof, EmailProofInput, PASSWORD_REQUIRED,
};
use crate::login::EMAIL_UNCONFIRMED;
use crate::settings::ModuleConfigUpdate;
use crate::two_factor::CabinetLogin;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_domain::client::{NewIdentity, proven_identities};
use sn_modules::catalog::mail::MailModule;
use sn_modules::catalog::oauth::OauthModule;

const NO_PROOF: EmailProofInput<'static> = EmailProofInput {
    password: None,
    code: None,
};

fn principal(client_id: ClientId) -> ClientPrincipal {
    ClientPrincipal {
        client_id,
        channel: ClientChannel::Miniapp,
        full_access: true,
        session_id: None,
    }
}

async fn env() -> Option<(Env, AdminId)> {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule), Arc::new(OauthModule)]).unwrap();
    let env = env_with(registry).await?;
    let admin = owner(&env.services).await;
    Some((env, admin))
}

async fn mail(services: &Services, admin: AdminId, config: serde_json::Value) {
    let serde_json::Value::Object(config) = config else {
        panic!("нужен объект");
    };
    services
        .settings_ops()
        .update_module_config(
            ModuleId::Mail,
            &ModuleConfigUpdate {
                config,
                clear: Vec::new(),
            },
            admin,
        )
        .await
        .unwrap();
}

async fn allow(services: &Services, admin: AdminId) {
    mail(
        services,
        admin,
        serde_json::json!({ "transport": "none", "link_without_code": true }),
    )
    .await;
}

async fn email_of(services: &Services, client: ClientId) -> Option<(String, bool, bool)> {
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .identities(client)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IdentityKind::Email)
        .map(|i| {
            let no_code = i.linked_without_code();
            (i.value, i.verified, no_code)
        })
}

fn last_code(letters: &Letters) -> String {
    let all = letters.lock().unwrap();
    code_in(&readable(all.last().unwrap()))
}

#[tokio::test]
async fn без_галочки_почта_без_писем_не_привязывается() {
    let Some((env, admin)) = env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.51");
    let client = telegram_client(services, 5_500_001).await;
    let me = principal(client);
    let state = auth.email_state(client).await.unwrap();
    assert!(!state.available && !state.without_code && !state.unconfirmed);
    assert_eq!(
        code(
            auth.email_link(&me, "alex@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_CHANGE_OFF
    );
    assert!(!services.mail().links_email());

    allow(services, admin).await;
    assert!(auth.email_state(client).await.unwrap().without_code);
    assert!(services.mail().links_email());
    services
        .settings_ops()
        .toggle_module(ModuleId::Mail, false, admin)
        .await
        .unwrap();
    assert!(auth.email_state(client).await.unwrap().without_code);

    let mut access = services.settings().client_access;
    access.site_login = false;
    services
        .settings_ops()
        .update_client_access(&access, admin)
        .await
        .unwrap();
    assert!(!auth.email_state(client).await.unwrap().without_code);
    assert_eq!(
        code(
            auth.email_link(&me, "alex@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_CHANGE_OFF
    );
    assert_eq!(email_of(services, client).await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn с_галочкой_клиент_из_telegram_привязывает_почту_и_входит_с_паролем() {
    let Some((env, admin)) = env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.52");
    let client = telegram_client(services, 5_500_011).await;
    let me = principal(client);
    allow(services, admin).await;
    assert_eq!(
        code(auth.email_link(&me, "не почта", NO_PROOF, &who).await),
        "validation.invalid_value"
    );
    let state = auth
        .email_link(&me, " Alex@Example.com ", NO_PROOF, &who)
        .await
        .unwrap();
    assert_eq!(state.email.as_deref(), Some("alex@example.com"));
    assert!(state.unconfirmed && state.without_code && !state.available);
    assert!(!state.confirm_current);
    assert_eq!(
        email_of(services, client).await,
        Some(("alex@example.com".into(), false, true))
    );
    assert_eq!(
        code(
            auth.email_link(&me, "ALEX@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_SAME
    );
    auth.change_password(&me, None, "пароль-алекса-1")
        .await
        .unwrap();
    let CabinetLogin::Session(login) = auth
        .email_password_login("alex@example.com", "пароль-алекса-1", &who)
        .await
        .unwrap()
    else {
        panic!("ожидался вход без второго фактора");
    };
    assert_eq!(login.principal.client_id, client);

    assert_eq!(
        code(
            auth.email_link(&me, "alex.new@example.com", NO_PROOF, &who)
                .await
        ),
        PASSWORD_REQUIRED
    );
    let right = EmailProofInput {
        password: Some("пароль-алекса-1"),
        code: None,
    };
    auth.email_link(&me, "alex.new@example.com", right, &who)
        .await
        .unwrap();
    assert_eq!(
        email_of(services, client).await,
        Some(("alex.new@example.com".into(), false, true))
    );
    auth.email_password_login("alex.new@example.com", "пароль-алекса-1", &who)
        .await
        .unwrap();

    let later = start() + Duration::minutes(20);
    env.clock.set(later);
    let setup = auth.client_totp_setup(client).await.unwrap();
    auth.client_totp_enable(client, &setup.secret, &totp_code(&setup.secret, later))
        .await
        .unwrap();
    assert_eq!(
        auth.email_state(client).await.unwrap().proof,
        Some(EmailProof::Totp)
    );
    assert_eq!(
        code(
            auth.email_link(&me, "alex.third@example.com", right, &who)
                .await
        ),
        APP_CODE_REQUIRED
    );
    let step = later + Duration::seconds(30);
    env.clock.set(step);
    let current = totp_code(&setup.secret, step);
    auth.email_link(
        &me,
        "alex.third@example.com",
        EmailProofInput {
            password: None,
            code: Some(&current),
        },
        &who,
    )
    .await
    .unwrap();

    let mut conn = services.store().conn().await.unwrap();
    let notes: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT details->>'email', details->>'previous' FROM audit_log \
         WHERE action = $1 AND entity_id = $2 ORDER BY id",
    )
    .bind(EMAIL_LINKED_AUDIT)
    .bind(client.0)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(
        notes,
        [
            (Some("alex@example.com".into()), None),
            (
                Some("alex.new@example.com".into()),
                Some("alex@example.com".into())
            ),
            (
                Some("alex.third@example.com".into()),
                Some("alex.new@example.com".into())
            ),
        ]
    );
    drop(conn);
    env.db.cleanup().await;
}

#[tokio::test]
async fn чужую_и_подтверждённую_почту_без_кода_не_тронуть() {
    let Some((env, admin)) = env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.53");
    allow(services, admin).await;
    auth.email_register("olga@example.com", "пароль-ольги-1", &who)
        .await
        .unwrap();
    let olga = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(IdentityKind::Email, "olga@example.com")
            .await
            .unwrap()
            .unwrap()
            .id
    };
    assert!(
        !auth.email_state(olga).await.unwrap().without_code,
        "клиенту, который входит только по почте, без кода её не сменить"
    );
    let right = EmailProofInput {
        password: Some("пароль-ольги-1"),
        code: None,
    };
    assert_eq!(
        code(
            auth.email_link(&principal(olga), "olga.new@example.com", right, &who)
                .await
        ),
        EMAIL_CHANGE_OFF
    );
    assert!(matches!(
        auth.email_register("olga@example.com", "другой-пароль-1", &who)
            .await,
        Err(ServiceError::Rejected {
            code: "auth.email_taken",
            ..
        })
    ));

    let client = telegram_client(services, 5_500_021).await;
    let me = principal(client);
    assert_eq!(
        code(
            auth.email_link(&me, "olga@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_IN_USE
    );
    services
        .clients()
        .set_email(client, "boss@example.com", Actor::Admin(admin))
        .await
        .unwrap();
    assert!(!auth.email_state(client).await.unwrap().without_code);
    assert_eq!(
        code(
            auth.email_link(&me, "other@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_CHANGE_OFF
    );
    assert_eq!(
        email_of(services, client).await,
        Some(("boss@example.com".into(), true, false))
    );

    let who = meta("203.0.113.57");
    let twin = telegram_client(services, 5_500_022).await;
    auth.email_link(&principal(twin), "twin@example.com", NO_PROOF, &who)
        .await
        .unwrap();
    let third = telegram_client(services, 5_500_023).await;
    assert_eq!(
        code(
            auth.email_link(&principal(third), "twin@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_IN_USE
    );
    let blocked = telegram_client(services, 5_500_024).await;
    services
        .clients()
        .block(blocked, Some("проверка"), Actor::Admin(admin))
        .await
        .unwrap();
    assert!(matches!(
        auth.email_link(&principal(blocked), "blocked@example.com", NO_PROOF, &who)
            .await,
        Err(ServiceError::Forbidden)
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn когда_письма_включили_неподтверждённая_почта_не_открывает_аккаунт() {
    let Some((env, admin)) = env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.54");
    let client = telegram_client(services, 5_500_031).await;
    let me = principal(client);
    allow(services, admin).await;
    auth.email_link(&me, "alex@example.com", NO_PROOF, &who)
        .await
        .unwrap();
    auth.change_password(&me, None, "пароль-алекса-1")
        .await
        .unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .add_identity(
                client,
                &NewIdentity::new(IdentityKind::Google, "8801", true).unwrap(),
            )
            .await
            .unwrap();
    }

    let (port, letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password" }),
    )
    .await;
    let state = auth.email_state(client).await.unwrap();
    assert!(state.available && state.unconfirmed && !state.without_code);
    assert_eq!(
        code(
            auth.email_link(&me, "other@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_CODE_NEEDED
    );
    auth.email_password_login("alex@example.com", "пароль-алекса-1", &who)
        .await
        .unwrap();

    auth.email_reset_start("alex@example.com", &who)
        .await
        .unwrap();
    let sent = last_code(&letters);
    assert_eq!(
        code(
            auth.email_reset_confirm("alex@example.com", &sent, "захват-аккаунта-1", &who)
                .await
        ),
        EMAIL_UNCONFIRMED
    );
    assert_eq!(
        code(
            auth.email_reset_confirm("alex@example.com", &sent, "захват-аккаунта-1", &who)
                .await
        ),
        "auth.code_invalid",
        "код после отказа больше не действует"
    );
    assert_eq!(
        code(
            auth.email_password_login("alex@example.com", "захват-аккаунта-1", &who)
                .await
        ),
        "auth.wrong_password"
    );
    auth.email_password_login("alex@example.com", "пароль-алекса-1", &who)
        .await
        .unwrap();
    assert_eq!(
        email_of(services, client).await,
        Some(("alex@example.com".into(), false, true))
    );

    env.clock.set(start() + Duration::minutes(30));
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "code" }),
    )
    .await;
    auth.email_login_start("alex@example.com", &who)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.email_login_verify("alex@example.com", &last_code(&letters), &who)
                .await
        ),
        EMAIL_UNCONFIRMED
    );

    env.clock.set(start() + Duration::minutes(60));
    let before = letters.lock().unwrap().len();
    auth.email_change_start(&me, "alex@example.com", NO_PROOF, &who)
        .await
        .unwrap();
    assert_eq!(letters.lock().unwrap().len(), before + 1);
    let state = auth
        .email_change_confirm(&me, "alex@example.com", &last_code(&letters), &who)
        .await
        .unwrap();
    assert!(!state.unconfirmed);
    assert_eq!(
        email_of(services, client).await,
        Some(("alex@example.com".into(), true, false))
    );
    {
        let mut conn = services.store().conn().await.unwrap();
        assert!(
            conn.clients()
                .find_by_identity(IdentityKind::Google, "8801")
                .await
                .unwrap()
                .is_some(),
            "вход через Google остаётся: аккаунт подтверждает сам клиент"
        );
    }
    assert_eq!(
        code(
            auth.email_change_start(&me, "alex@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_SAME
    );

    env.clock.set(start() + Duration::minutes(90));
    auth.email_login_start("alex@example.com", &who)
        .await
        .unwrap();
    let CabinetLogin::Session(login) = auth
        .email_login_verify("alex@example.com", &last_code(&letters), &who)
        .await
        .unwrap()
    else {
        panic!("ожидался вход без второго фактора");
    };
    assert_eq!(login.principal.client_id, client);
    env.db.cleanup().await;
}

#[tokio::test]
async fn опечатку_в_неподтверждённой_почте_исправляют_без_письма_на_неё() {
    let Some((env, admin)) = env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.55");
    let client = telegram_client(services, 5_500_041).await;
    let me = principal(client);
    allow(services, admin).await;
    auth.email_link(&me, "alxe@example.com", NO_PROOF, &who)
        .await
        .unwrap();
    let (port, letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password" }),
    )
    .await;
    assert_eq!(
        code(auth.email_proof_start(&me, &who).await),
        "auth.current_email_code_not_needed"
    );
    auth.email_change_start(&me, "alex@example.com", NO_PROOF, &who)
        .await
        .unwrap();
    auth.email_change_confirm(&me, "alex@example.com", &last_code(&letters), &who)
        .await
        .unwrap();
    assert_eq!(
        email_of(services, client).await,
        Some(("alex@example.com".into(), true, false))
    );
    let all = letters.lock().unwrap().clone();
    assert_eq!(all.len(), 1, "на адрес с опечаткой ничего не уходит");
    assert!(!all[0].contains("alxe@example.com"));
    env.db.cleanup().await;
}

#[tokio::test]
async fn администратор_подтверждает_и_меняет_почту_привязанную_без_кода() {
    let Some((env, admin)) = env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.56");
    let client = telegram_client(services, 5_500_051).await;
    allow(services, admin).await;
    auth.email_link(&principal(client), "alex@example.com", NO_PROOF, &who)
        .await
        .unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .add_identity(
                client,
                &NewIdentity::new(IdentityKind::Google, "8802", true).unwrap(),
            )
            .await
            .unwrap();
        let identities = conn.clients().identities(client).await.unwrap();
        let proven: Vec<IdentityKind> = proven_identities(identities)
            .into_iter()
            .map(|(kind, _)| kind)
            .collect();
        assert_eq!(proven, [IdentityKind::Telegram, IdentityKind::Google]);
    }
    let (port, letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password" }),
    )
    .await;
    auth.issue_client_password(client, Actor::Admin(admin))
        .await
        .unwrap();
    services
        .clients()
        .set_email(client, "boss@example.com", Actor::Admin(admin))
        .await
        .unwrap();
    assert!(
        letters.lock().unwrap().is_empty(),
        "на неподтверждённый адрес письма не уходят"
    );
    assert_eq!(
        email_of(services, client).await,
        Some(("boss@example.com".into(), true, false))
    );
    let mut conn = services.store().conn().await.unwrap();
    assert!(
        conn.clients()
            .find_by_identity(IdentityKind::Google, "8802")
            .await
            .unwrap()
            .is_some()
    );
    drop(conn);
    env.db.cleanup().await;
}

#[tokio::test]
async fn с_подтверждением_регистрации_без_кода_почта_не_привязывается() {
    let Some((env, admin)) = env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let client = telegram_client(services, 5_500_061).await;
    let (port, _letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password", "confirm_signup": true, "link_without_code": true }),
    )
    .await;
    assert!(!auth.email_state(client).await.unwrap().without_code);
    services
        .settings_ops()
        .toggle_module(ModuleId::Mail, false, admin)
        .await
        .unwrap();
    assert!(!services.mail().available());
    assert!(
        !auth.email_state(client).await.unwrap().without_code,
        "регистрация без подтверждения закрыта, и привязка без кода тоже"
    );
    env.db.cleanup().await;
}
