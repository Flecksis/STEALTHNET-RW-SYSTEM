use super::mail::{Letters, code_in, configure_with, fake_smtp, fake_smtp_rejecting, readable};
use super::*;
use crate::email_change::{
    APP_CODE_REQUIRED, CURRENT_CODE_REQUIRED, EMAIL_CHANGE_OFF, EMAIL_IN_USE, EMAIL_SAME,
    EmailProof, EmailProofInput, PASSWORD_INVALID, PASSWORD_REQUIRED,
};
use crate::two_factor::CabinetLogin;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_domain::client::NewIdentity;
use sn_modules::catalog::mail::MailModule;

pub(super) const NO_PROOF: EmailProofInput<'static> = EmailProofInput {
    password: None,
    code: None,
};

pub(super) fn principal(client_id: ClientId) -> ClientPrincipal {
    ClientPrincipal {
        client_id,
        channel: ClientChannel::Miniapp,
        full_access: true,
        session_id: None,
    }
}

async fn mail_env() -> Option<(Env, AdminId)> {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let env = env_with(registry).await?;
    let admin = owner(&env.services).await;
    Some((env, admin))
}

fn sent(letters: &Letters) -> Vec<String> {
    letters.lock().unwrap().clone()
}

fn head_and_text(raw: &str) -> (String, String) {
    let head = raw.split_once("\n\n").map_or(raw, |(h, _)| h).to_string();
    (head, readable(raw))
}

fn other_code(code: &str) -> &'static str {
    if code == "000000" { "111111" } else { "000000" }
}

async fn email_identity(services: &Services, client: ClientId) -> Option<(String, bool)> {
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .identities(client)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IdentityKind::Email)
        .map(|i| (i.value, i.verified))
}

#[tokio::test]
async fn почта_привязывается_и_меняется_только_по_коду_с_нового_адреса() {
    let Some((env, admin)) = mail_env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.31");
    let client = telegram_client(services, 5_093_118_422).await;
    let me = principal(client);
    assert_eq!(
        code(
            auth.email_change_start(&me, "alex@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_CHANGE_OFF
    );
    let off = auth.email_state(client).await.unwrap();
    assert!(!off.available);
    assert_eq!((off.email, off.proof, off.pending), (None, None, None));

    let (port, letters) = fake_smtp().await;
    configure_with(services, admin, port, serde_json::json!({})).await;
    assert_eq!(
        code(
            auth.email_change_start(&me, "не почта", NO_PROOF, &who)
                .await
        ),
        "validation.invalid_value"
    );
    let pending = auth
        .email_change_start(&me, " Alex@Example.com ", NO_PROOF, &who)
        .await
        .unwrap();
    assert_eq!(pending.email, "alex@example.com");
    assert_eq!(pending.expires_at, start() + Duration::minutes(15));
    let first = sent(&letters);
    assert_eq!(first.len(), 1);
    let (head, text) = head_and_text(&first[0]);
    assert!(head.contains("alex@example.com"), "{head}");
    assert!(text.contains("Код для смены почты"), "{text}");
    let mail_code = code_in(&text);
    let state = auth.email_state(client).await.unwrap();
    assert!(state.available);
    assert_eq!(state.email, None);
    assert_eq!(
        state.pending.as_ref().map(|p| p.email.as_str()),
        Some("alex@example.com")
    );
    assert_eq!(
        code(
            auth.email_change_confirm(&me, "alex@example.com", other_code(&mail_code), &who)
                .await
        ),
        "auth.code_invalid"
    );
    assert_eq!(
        code(
            auth.email_change_confirm(&me, "bob@example.com", &mail_code, &who)
                .await
        ),
        "auth.code_invalid"
    );
    let stranger = telegram_client(services, 7_100_200).await;
    assert_eq!(
        code(
            auth.email_change_confirm(&principal(stranger), "alex@example.com", &mail_code, &who)
                .await
        ),
        "auth.code_invalid"
    );
    assert_eq!(email_identity(services, client).await, None);
    let changed = auth
        .email_change_confirm(&me, "ALEX@example.com", &mail_code, &who)
        .await
        .unwrap();
    assert_eq!(changed.email.as_deref(), Some("alex@example.com"));
    assert_eq!(changed.pending, None);
    assert_eq!(
        email_identity(services, client).await,
        Some(("alex@example.com".to_string(), true))
    );
    assert_eq!(sent(&letters).len(), 1);
    assert_eq!(
        code(
            auth.email_change_confirm(&me, "alex@example.com", &mail_code, &who)
                .await
        ),
        "auth.code_invalid"
    );
    assert_eq!(
        code(
            auth.email_change_start(&me, "alex@example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_SAME
    );

    env.clock.set(start() + Duration::minutes(16));
    assert_eq!(
        code(
            auth.email_change_start(&me, "bob@example.com", NO_PROOF, &who)
                .await
        ),
        CURRENT_CODE_REQUIRED
    );
    assert!(auth.email_state(client).await.unwrap().confirm_current);
    let proof = auth.email_proof_start(&me, &who).await.unwrap();
    assert_eq!(proof.email, "alex@example.com");
    let (head, text) = head_and_text(&sent(&letters)[1]);
    assert!(head.contains("alex@example.com"), "{head}");
    let current_code = code_in(&text);
    assert_eq!(
        code(
            auth.email_change_start(
                &me,
                "bob@example.com",
                EmailProofInput {
                    password: None,
                    code: Some(other_code(&current_code)),
                },
                &who
            )
            .await
        ),
        "auth.code_invalid"
    );
    auth.email_change_start(
        &me,
        "bob@example.com",
        EmailProofInput {
            password: None,
            code: Some(&current_code),
        },
        &who,
    )
    .await
    .unwrap();
    let (_, text) = head_and_text(&sent(&letters)[2]);
    let bob_code = code_in(&text);
    auth.email_change_confirm(&me, "bob@example.com", &bob_code, &who)
        .await
        .unwrap();
    assert_eq!(
        email_identity(services, client).await,
        Some(("bob@example.com".to_string(), true))
    );
    let all = sent(&letters);
    assert_eq!(all.len(), 4);
    let (head, text) = head_and_text(&all[3]);
    assert!(head.contains("alex@example.com"), "{head}");
    assert!(!head.contains("bob@example.com"), "{head}");
    assert!(text.contains("Почту в аккаунте"), "{text}");
    assert!(text.contains("b***@example.com"), "{text}");
    assert!(!text.contains("bob@example.com"), "{text}");
    let mut conn = services.store().conn().await.unwrap();
    assert!(
        conn.clients()
            .find_by_identity(IdentityKind::Email, "alex@example.com")
            .await
            .unwrap()
            .is_none()
    );
    let audit = conn
        .audit()
        .for_entity("client", client.0, 20)
        .await
        .unwrap();
    let changes: Vec<_> = audit
        .iter()
        .filter(|a| a.action == "client.email_changed")
        .collect();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].details["email"], "bob@example.com");
    assert_eq!(changes[0].details["previous"], "alex@example.com");
    assert_eq!(changes[0].details["channel"], "miniapp");
    assert_eq!(changes[1].details["previous"], serde_json::Value::Null);
    drop(conn);
    env.db.cleanup().await;
}

#[tokio::test]
async fn просроченный_и_перебранный_код_не_меняют_почту() {
    let Some((env, admin)) = mail_env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.32");
    let (port, letters) = fake_smtp().await;
    configure_with(services, admin, port, serde_json::json!({})).await;
    let client = telegram_client(services, 5_093_118_422).await;
    let me = principal(client);
    auth.email_change_start(&me, "alex@example.com", NO_PROOF, &who)
        .await
        .unwrap();
    let late = code_in(&readable(&sent(&letters)[0]));
    env.clock.set(start() + Duration::minutes(16));
    assert_eq!(auth.email_state(client).await.unwrap().pending, None);
    assert_eq!(
        code(
            auth.email_change_confirm(&me, "alex@example.com", &late, &who)
                .await
        ),
        "auth.code_invalid"
    );
    auth.email_change_start(&me, "alex@example.com", NO_PROOF, &who)
        .await
        .unwrap();
    let fresh = code_in(&readable(&sent(&letters)[1]));
    for _ in 0..crate::login::EMAIL_CODE_MAX_ATTEMPTS {
        assert_eq!(
            code(
                auth.email_change_confirm(&me, "alex@example.com", other_code(&fresh), &who)
                    .await
            ),
            "auth.code_invalid"
        );
    }
    assert_eq!(
        code(
            auth.email_change_confirm(&me, "alex@example.com", &fresh, &who)
                .await
        ),
        "auth.code_attempts"
    );
    assert_eq!(auth.email_state(client).await.unwrap().pending, None);
    assert_eq!(email_identity(services, client).await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn занятый_адрес_даёт_понятную_ошибку() {
    let Some((env, admin)) = mail_env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.33");
    let (port, letters) = fake_smtp().await;
    configure_with(services, admin, port, serde_json::json!({})).await;
    let client = telegram_client(services, 5_093_118_422).await;
    let other = telegram_client(services, 7_100_200).await;
    let me = principal(client);
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .add_identity(
            other,
            &NewIdentity::new(IdentityKind::Email, "taken@example.com", true).unwrap(),
        )
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        code(
            auth.email_change_start(&me, "Taken@Example.com", NO_PROOF, &who)
                .await
        ),
        EMAIL_IN_USE
    );
    assert!(sent(&letters).is_empty());
    auth.email_change_start(&me, "free@example.com", NO_PROOF, &who)
        .await
        .unwrap();
    let mail_code = code_in(&readable(&sent(&letters)[0]));
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .add_identity(
            other,
            &NewIdentity::new(IdentityKind::Email, "free@example.com", true).unwrap(),
        )
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        code(
            auth.email_change_confirm(&me, "free@example.com", &mail_code, &who)
                .await
        ),
        EMAIL_IN_USE
    );
    assert_eq!(
        code(
            auth.email_change_confirm(&me, "free@example.com", &mail_code, &who)
                .await
        ),
        "auth.code_invalid"
    );
    assert_eq!(email_identity(services, client).await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn для_смены_почты_нужен_пароль_сайта_или_код_из_приложения() {
    let Some((env, admin)) = mail_env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let who = meta("203.0.113.34");
    let (port, letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password" }),
    )
    .await;
    let CabinetLogin::Session(login) = auth
        .email_register("olga@example.com", "пароль-ольги-1", &who)
        .await
        .unwrap()
    else {
        panic!("без второго фактора регистрация сразу открывает кабинет");
    };
    let client = login.principal.client_id;
    let me = login.principal;
    assert_eq!(
        auth.email_proof(client).await.unwrap(),
        Some(EmailProof::Password)
    );
    assert_eq!(
        code(
            auth.email_change_start(&me, "olga.new@example.com", NO_PROOF, &who)
                .await
        ),
        PASSWORD_REQUIRED
    );
    let wrong = EmailProofInput {
        password: Some("чужой-пароль-1"),
        code: None,
    };
    assert_eq!(
        code(
            auth.email_change_start(&me, "olga.new@example.com", wrong, &who)
                .await
        ),
        PASSWORD_INVALID
    );
    assert!(sent(&letters).is_empty());
    let right = EmailProofInput {
        password: Some("пароль-ольги-1"),
        code: None,
    };
    auth.email_change_start(&me, "olga.new@example.com", right, &who)
        .await
        .unwrap();
    assert_eq!(sent(&letters).len(), 1);

    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "code" }),
    )
    .await;
    assert_eq!(auth.email_proof(client).await.unwrap(), None);
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password" }),
    )
    .await;

    let later = start() + Duration::minutes(20);
    env.clock.set(later);
    let setup = auth.client_totp_setup(client).await.unwrap();
    auth.client_totp_enable(client, &setup.secret, &totp_code(&setup.secret, later))
        .await
        .unwrap();
    assert_eq!(
        auth.email_proof(client).await.unwrap(),
        Some(EmailProof::Totp)
    );
    assert_eq!(
        code(
            auth.email_change_start(&me, "olga.new@example.com", right, &who)
                .await
        ),
        APP_CODE_REQUIRED
    );
    let step = later + Duration::seconds(30);
    env.clock.set(step);
    let current = totp_code(&setup.secret, step);
    let bad = EmailProofInput {
        password: None,
        code: Some(other_code(&current)),
    };
    assert_eq!(
        code(
            auth.email_change_start(&me, "olga.new@example.com", bad, &who)
                .await
        ),
        "auth.totp_code"
    );
    let good = EmailProofInput {
        password: None,
        code: Some(&current),
    };
    auth.email_change_start(&me, "olga.new@example.com", good, &who)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.email_change_start(&me, "olga.box@example.com", good, &who)
                .await
        ),
        "auth.totp_code"
    );
    assert_eq!(sent(&letters).len(), 2);
    let letter = readable(&sent(&letters)[1]);
    auth.email_change_confirm(&me, "olga.new@example.com", &code_in(&letter), &who)
        .await
        .unwrap();
    assert_eq!(
        email_identity(services, client).await,
        Some(("olga.new@example.com".to_string(), true))
    );
    let all = sent(&letters);
    let (head, text) = head_and_text(all.last().unwrap());
    assert!(head.contains("olga@example.com"), "{head}");
    assert!(text.contains("o***@example.com"), "{text}");
    auth.email_password_login("olga.new@example.com", "пароль-ольги-1", &who)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.email_password_login("olga@example.com", "пароль-ольги-1", &who)
                .await
        ),
        "auth.wrong_password"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn частые_запросы_смены_почты_упираются_в_лимит() {
    let Some((env, admin)) = mail_env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let (port, _letters) = fake_smtp().await;
    configure_with(services, admin, port, serde_json::json!({})).await;
    let client = telegram_client(services, 5_093_118_422).await;
    let me = principal(client);
    for n in 0..rules::EMAIL_CHANGE_CLIENT.limit {
        auth.email_change_start(
            &me,
            &format!("box{n}@example.com"),
            NO_PROOF,
            &meta(&format!("203.0.113.{}", 40 + n)),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        code(
            auth.email_change_start(&me, "box9@example.com", NO_PROOF, &meta("203.0.113.60"))
                .await
        ),
        RATE_LIMITED
    );
    let other = principal(telegram_client(services, 7_100_200).await);
    for n in 0..rules::EMAIL_START_ADDRESS.limit {
        auth.email_change_start(
            &other,
            "same@example.com",
            NO_PROOF,
            &meta(&format!("203.0.113.{}", 70 + n)),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        code(
            auth.email_change_start(&other, "same@example.com", NO_PROOF, &meta("203.0.113.80"))
                .await
        ),
        RATE_LIMITED
    );
    env.clock.set(start() + Duration::minutes(16));
    auth.email_change_start(&me, "box9@example.com", NO_PROOF, &meta("203.0.113.61"))
        .await
        .unwrap();
    env.db.cleanup().await;
}

#[tokio::test]
async fn неотправленный_код_не_ждёт_ввода() {
    let Some((env, admin)) = mail_env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let closed = {
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    };
    configure_with(services, admin, closed, serde_json::json!({})).await;
    let client = telegram_client(services, 5_093_118_422).await;
    let failed = auth
        .email_change_start(
            &principal(client),
            "alex@example.com",
            NO_PROOF,
            &meta("203.0.113.35"),
        )
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "mail.failed");
    assert_eq!(
        failed.to_string(),
        "Не получилось отправить письмо. Попробуйте позже."
    );
    let state = auth.email_state(client).await.unwrap();
    assert!(state.available);
    assert_eq!(state.pending, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn клиент_видит_общую_ошибку_почты_а_проверка_у_администратора_подробную() {
    let Some((env, admin)) = mail_env().await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    let (port, letters) = fake_smtp_rejecting("alex@example.com").await;
    configure_with(services, admin, port, serde_json::json!({})).await;
    let client = telegram_client(services, 5_093_118_501).await;
    let refused = auth
        .email_change_start(
            &principal(client),
            "alex@example.com",
            NO_PROOF,
            &meta("203.0.113.36"),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "mail.address");
    assert_eq!(
        refused.to_string(),
        "Не получилось отправить письмо на этот адрес. Проверьте адрес почты."
    );
    let login = auth
        .email_login_start("alex@example.com", &meta("203.0.113.37"))
        .await
        .unwrap_err();
    assert_eq!(login.code(), "mail.address");
    assert!(!login.to_string().contains("550"), "{login}");
    assert!(!login.to_string().contains("No such user"), "{login}");
    assert!(sent(&letters).is_empty());
    let check = services
        .mail()
        .test("alex@example.com")
        .await
        .unwrap_err()
        .to_string();
    assert!(check.contains("550"), "{check}");
    assert!(check.contains("No such user here"), "{check}");
    env.db.cleanup().await;
}
