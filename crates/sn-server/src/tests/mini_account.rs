use super::*;
use sn_domain::module::ModuleId;
use sn_modules::catalog::mail::MailModule;
use sn_services::auth::sign_init_data;
use sn_services::settings::ModuleConfigUpdate;
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const BOT: &str = "123456:TEST-token";

type Letters = Arc<Mutex<Vec<String>>>;

async fn local_smtp() -> (u16, Letters) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let letters: Letters = Arc::new(Mutex::new(Vec::new()));
    let store = letters.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let store = store.clone();
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                if write.write_all(b"220 local ESMTP\r\n").await.is_err() {
                    return;
                }
                let mut data = false;
                let mut body = String::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    if data {
                        if line == "." {
                            data = false;
                            store.lock().unwrap().push(std::mem::take(&mut body));
                            let _ = write.write_all(b"250 queued\r\n").await;
                        } else {
                            body.push_str(&line);
                            body.push('\n');
                        }
                        continue;
                    }
                    let upper = line.to_ascii_uppercase();
                    let reply: &[u8] = if upper.starts_with("EHLO") {
                        b"250-local\r\n250 OK\r\n"
                    } else if upper.starts_with("DATA") {
                        data = true;
                        b"354 go on\r\n"
                    } else if upper.starts_with("QUIT") {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        b"250 OK\r\n"
                    };
                    if write.write_all(reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, letters)
}

fn unbase64(text: &str) -> Vec<u8> {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for byte in text.bytes() {
        let Some(value) = ABC.iter().position(|c| *c == byte) else {
            continue;
        };
        acc = (acc << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

fn letter_code(raw: &str) -> String {
    let (head, body) = raw.split_once("\n\n").unwrap_or((raw, ""));
    let text = if head
        .to_ascii_lowercase()
        .contains("content-transfer-encoding: base64")
    {
        String::from_utf8(unbase64(body)).unwrap()
    } else {
        body.replace("=\n", "")
    };
    text.split(|c: char| !c.is_ascii_digit())
        .find(|part| part.len() == 6)
        .unwrap()
        .to_string()
}

async fn mini_http() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks {
            bot_token: Some(BOT.into()),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

async fn mail_on(h: &Http, port: u16) {
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

fn init_data(h: &Http) -> String {
    let user = r#"{"id":5093118422,"first_name":"Алексей","username":"morozov_a"}"#;
    let auth_date = h.services().now().timestamp().to_string();
    let hash = sign_init_data(&[("auth_date", &auth_date), ("user", user)], BOT);
    let encoded: String = user
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("auth_date={auth_date}&user={encoded}&hash={hash}")
}

#[tokio::test]
async fn мини_приложение_меняет_почту_пароль_и_вход_по_коду_не_закрывая_сессию() {
    let Some(h) = mini_http().await else { return };
    let (port, letters) = local_smtp().await;
    mail_on(&h, port).await;
    let (status, _, login) = h
        .send(
            Method::POST,
            "/api/client/auth/miniapp",
            None,
            Some(json!({ "init_data": init_data(&h) })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let mini = login["token"].as_str().unwrap().to_string();
    let client = ClientId(login["principal"]["client_id"].as_i64().unwrap());
    let (status, me) = h.get("/api/client/me", &mini).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["channel"], "miniapp");
    assert_eq!(me["email"], Value::Null);
    assert_eq!(me["email_change"], true);
    assert_eq!(me["has_password"], false);
    let (status, email) = h.get("/api/client/security/email", &mini).await;
    assert_eq!(status, StatusCode::OK, "{email}");
    assert_eq!(
        email,
        json!({ "email": null, "available": true, "proof": null, "pending": null })
    );

    let (status, started) = h
        .write(
            Method::POST,
            "/api/client/security/email/start",
            &mini,
            json!({ "email": "Alex@Example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["email"], "alex@example.com");
    let sent = letter_code(&letters.lock().unwrap()[0]);
    let wrong = if sent == "000000" { "111111" } else { "000000" };
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/email/confirm",
            &mini,
            json!({ "email": "alex@example.com", "code": wrong }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.code_invalid");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/email/confirm",
            &mini,
            json!({ "email": "alex@example.com", "code": sent }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["email"], "alex@example.com");

    let cabinet = h.client_token(client).await;
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/password",
            &mini,
            json!({ "password": "пароль-алексея-1" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, me) = h.get("/api/client/me", &mini).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], "alex@example.com");
    assert_eq!(me["has_password"], true);
    assert_eq!(
        h.get("/api/client/me", &cabinet).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "alex@example.com", "password": "пароль-алексея-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/password",
            &mini,
            json!({ "current": "чужой-пароль-12", "password": "пароль-алексея-2" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/password",
            &mini,
            json!({ "current": "пароль-алексея-1", "password": "пароль-алексея-2" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, email) = h.get("/api/client/security/email", &mini).await;
    assert_eq!(email["proof"], "password");

    let (status, setup) = h
        .write(
            Method::POST,
            "/api/client/security/totp/setup",
            &mini,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{setup}");
    let secret = setup["secret"].as_str().unwrap().to_string();
    assert!(
        setup["uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/")
    );
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/totp/enable",
            &mini,
            json!({ "secret": secret, "code": totp_code(&secret, start()) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, me) = h.get("/api/client/me", &mini).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["two_factor"]["enabled"], true);
    let (_, email) = h.get("/api/client/security/email", &mini).await;
    assert_eq!(email["proof"], "totp");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/email/start",
            &mini,
            json!({ "email": "bob@example.com", "password": "пароль-алексея-2" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.app_code_required");
    let later = start() + Duration::seconds(30);
    h.clock.set(later);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/email/start",
            &mini,
            json!({ "email": "bob@example.com", "code": totp_code(&secret, later) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, email) = h.get("/api/client/security/email", &mini).await;
    assert_eq!(email["pending"]["email"], "bob@example.com");
    let disable_at = later + Duration::seconds(30);
    h.clock.set(disable_at);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/totp/disable",
            &mini,
            json!({ "code": totp_code(&secret, disable_at) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);
    let (status, me) = h.get("/api/client/me", &mini).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["two_factor"]["enabled"], false);
    assert_eq!(letters.lock().unwrap().len(), 2);
    h.db.cleanup().await;
}

#[tokio::test]
async fn без_отправки_писем_смена_почты_закрыта() {
    let Some(h) = mini_http().await else { return };
    let client = h.client(5_093_118_422).await;
    let token = h.client_token(client).await;
    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email_change"], false);
    let (status, email) = h.get("/api/client/security/email", &token).await;
    assert_eq!(status, StatusCode::OK, "{email}");
    assert_eq!(email["available"], false);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/email/start",
            &token,
            json!({ "email": "alex@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.email_change_off");
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/languages",
            &owner,
            json!({ "english": true, "default": "ru" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/client/security/email/start")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-sn-lang", "en")
        .body(Body::from(
            json!({ "email": "alex@example.com" }).to_string(),
        ))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["message"],
        "Emails aren't being sent right now, so the email can't be changed."
    );
    h.db.cleanup().await;
}
