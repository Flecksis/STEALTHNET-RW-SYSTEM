use super::mini_account::{Letters, letter_code, local_smtp, mini_http};
use super::*;
use sn_domain::client::{IdentityKind, NewIdentity};
use sn_domain::module::ModuleId;
use sn_services::settings::ModuleConfigUpdate;

async fn mail_confirming(h: &Http, port: u16) {
    let admin = h
        .services()
        .auth()
        .upsert_owner("owner@example.com", PASSWORD)
        .await
        .unwrap()
        .0;
    let config = json!({
        "transport": "smtp",
        "smtp_host": "127.0.0.1",
        "smtp_port": port,
        "security": "none",
        "from_email": "robot@example.com",
        "login": "password",
        "confirm_signup": true,
    });
    let ops = h.services().settings_ops();
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config: config.as_object().unwrap().clone(),
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Mail, true, admin)
        .await
        .unwrap();
}

fn last(letters: &Letters) -> String {
    letters.lock().unwrap().last().unwrap().clone()
}

fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn регистрация_с_подтверждением_почты_через_api() {
    let Some(h) = mini_http().await else { return };
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["email_confirm"], false);
    let (port, letters) = local_smtp().await;
    mail_confirming(&h, port).await;
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["email_auth"], "password");
    assert_eq!(site["email_confirm"], true);
    assert_eq!(site["email_reset"], true);
    let (status, headers, fresh) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register",
            None,
            Some(json!({
                "email": "dina@example.com",
                "password": "пароль-дины-1",
                "utm": { "source": "vk" },
                "ref": "NOPE1234",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    assert_eq!(fresh["status"], "code_sent");
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert!(fresh.get("principal").is_none() && fresh.get("token").is_none());
    let code = letter_code(&last(&letters));
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "dina@example.com", "password": "пароль-дины-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.wrong_password");
    let owner = h
        .services()
        .clients()
        .ensure_telegram(
            &sn_domain::client::TelegramProfile {
                id: 5_300_001,
                username: None,
                first_name: Some("Олег".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            sn_domain::client::RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap()
        .0
        .id;
    {
        let mut tx = h.services().store().begin().await.unwrap();
        tx.clients()
            .add_identity(
                owner,
                &NewIdentity::new(IdentityKind::Email, "oleg@example.com", true).unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let sent_before = letters.lock().unwrap().len();
    let (status, headers, taken) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register",
            None,
            Some(json!({ "email": "oleg@example.com", "password": "пароль-чужой-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{taken}");
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert_eq!(keys(&taken), keys(&fresh));
    assert_eq!(taken["status"], "code_sent");
    assert_eq!(letters.lock().unwrap().len(), sent_before + 1);
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register/confirm",
            None,
            Some(json!({ "email": "dina@example.com", "signup_token": fresh["signup_token"], "code": wrong })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.code_invalid");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register/resend",
            None,
            Some(json!({ "email": "dina@example.com", "signup_token": fresh["signup_token"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["signup_token"], fresh["signup_token"]);
    let code = letter_code(&last(&letters));
    let (status, headers, login) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register/confirm",
            None,
            Some(json!({ "email": "dina@example.com", "signup_token": fresh["signup_token"], "code": code })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    assert!(headers.get(header::SET_COOKIE).is_some());
    assert_eq!(login["created"], true);
    let session = login["token"].as_str().unwrap().to_string();
    let (status, me) = h.get("/api/client/me", &session).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], "dina@example.com");
    assert_eq!(me["has_password"], true);
    let admin = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/modules/mail/config",
            &admin,
            json!({ "config": { "transport": "none" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}
