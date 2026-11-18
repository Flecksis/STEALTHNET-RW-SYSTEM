use super::*;
use crate::settings::ModuleConfigUpdate;
use crate::two_factor::CabinetLogin;
use base64::Engine;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_modules::catalog::mail::MailModule;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

pub(super) type Letters = Arc<StdMutex<Vec<String>>>;

pub(super) async fn fake_smtp() -> (u16, Letters) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let letters: Letters = Arc::new(StdMutex::new(Vec::new()));
    let store = letters.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let store = store.clone();
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                if write.write_all(b"220 fake ESMTP\r\n").await.is_err() {
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
                        b"250-fake\r\n250 OK\r\n"
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

pub(super) fn readable(letter: &str) -> String {
    let (head, body) = letter.split_once("\n\n").unwrap_or((letter, ""));
    if head
        .to_ascii_lowercase()
        .contains("content-transfer-encoding: base64")
    {
        let joined: String = body.split_whitespace().collect();
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(joined)
                .unwrap(),
        )
        .unwrap()
    } else {
        body.replace("=\n", "")
    }
}

pub(super) fn code_in(text: &str) -> String {
    text.split(|c: char| !c.is_ascii_digit())
        .find(|part| part.len() == crate::login::EMAIL_CODE_DIGITS)
        .unwrap()
        .to_string()
}

async fn configure(services: &Services, admin: AdminId, port: u16) {
    configure_with(services, admin, port, serde_json::json!({})).await;
}

pub(super) async fn configure_with(
    services: &Services,
    admin: AdminId,
    port: u16,
    extra: serde_json::Value,
) {
    let mut config = serde_json::Map::new();
    if let serde_json::Value::Object(extra) = extra {
        config.extend(extra);
    }
    config.insert("transport".into(), serde_json::json!("smtp"));
    config.insert("smtp_host".into(), serde_json::json!("127.0.0.1"));
    config.insert("smtp_port".into(), serde_json::json!(port));
    config.insert("security".into(), serde_json::json!("none"));
    config.insert("from_email".into(), serde_json::json!("robot@example.com"));
    let ops = services.settings_ops();
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config,
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

#[tokio::test]
async fn код_входа_приходит_письмом_и_открывает_кабинет() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.9");
    assert!(!services.mail().available());
    assert_eq!(
        code(auth.email_login_start("alex@example.com", &who).await),
        "auth.email_password_mode"
    );
    let (port, letters) = fake_smtp().await;
    configure(services, admin, port).await;
    assert!(services.mail().available());
    services.mail().test("owner@example.com").await.unwrap();
    let start = auth
        .email_login_start("Alex@Example.com", &who)
        .await
        .unwrap();
    assert_eq!(start.email, "alex@example.com");
    let received = letters.lock().unwrap().clone();
    assert_eq!(received.len(), 2);
    assert!(readable(&received[0]).contains("проверочное письмо"));
    let letter = readable(&received[1]);
    assert!(letter.contains("Код для входа в кабинет"), "{letter}");
    let sent_code = code_in(&letter);
    auth.email_login_verify("alex@example.com", &sent_code, &who)
        .await
        .unwrap();
    let closed = {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    };
    configure(services, admin, closed).await;
    assert_eq!(
        code(services.mail().test("owner@example.com").await),
        "mail.failed"
    );
    services
        .settings_ops()
        .toggle_module(ModuleId::Mail, false, admin)
        .await
        .unwrap();
    assert!(!services.mail().available());
    env.db.cleanup().await;
}

#[tokio::test]
async fn письмо_через_resend_уходит_с_ключом_и_ошибка_понятна() {
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let mut config = serde_json::Map::new();
    config.insert("transport".into(), serde_json::json!("resend"));
    config.insert("resend_api_key".into(), serde_json::json!("re_test"));
    config.insert("from_email".into(), serde_json::json!("robot@example.com"));
    config.insert("from_name".into(), serde_json::json!("Сервис"));
    let ops = services.settings_ops();
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config,
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    assert!(services.mail().available());
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/emails"))
        .and(header("authorization", "Bearer re_test"))
        .and(body_partial_json(serde_json::json!({
            "from": "Сервис <robot@example.com>",
            "to": ["owner@example.com"]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "1" })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/emails"))
        .respond_with(ResponseTemplate::new(422).set_body_json(
            serde_json::json!({ "statusCode": 422, "message": "The example.com domain is not verified" }),
        ))
        .mount(&server)
        .await;
    services
        .mail()
        .with_resend_base(server.uri())
        .test("owner@example.com")
        .await
        .unwrap();
    let failed = services
        .mail()
        .with_resend_base(server.uri())
        .test("owner@example.com")
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "mail.failed");
    assert!(
        failed.to_string().contains("domain is not verified"),
        "{failed}"
    );
    env.db.cleanup().await;
}

async fn fake_smtp_rejecting(refused: &'static str) -> (u16, Letters) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let letters: Letters = Arc::new(StdMutex::new(Vec::new()));
    let store = letters.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let store = store.clone();
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                if write.write_all(b"220 fake ESMTP\r\n").await.is_err() {
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
                        b"250-fake\r\n250 OK\r\n"
                    } else if upper.starts_with("RCPT") && line.contains(refused) {
                        b"550 5.1.1 No such user here\r\n"
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

async fn site_ready(services: &Services) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings()
        .set(
            sn_domain::settings::keys::PUBLIC_URL,
            &serde_json::json!("https://vpn.example.com/"),
            None,
        )
        .await
        .unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

const UNSUBSCRIBE: &str = "https://vpn.example.com/api/public/m/broadcast/unsubscribe/abc";

fn rich(to: &str) -> sn_modules::MailOut<'_> {
    sn_modules::MailOut {
        to,
        subject: "Новые серверы",
        text: "Текст письма",
        html: "<p>Текст <b>письма</b></p>",
        unsubscribe: Some(UNSUBSCRIBE),
    }
}

#[tokio::test]
async fn письмо_рассылки_уходит_по_smtp_с_html_и_отпиской() {
    use sn_modules::{MailDelivery, MailSender};
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let bridge = crate::mail::MailBridge::new(services.clone());
    assert!(bridge.site().unwrap_err().contains("способ отправки"));
    let (port, letters) = fake_smtp_rejecting("nobody@example.com").await;
    configure(services, admin, port).await;
    assert!(bridge.site().unwrap_err().contains("адрес сайта"));
    site_ready(services).await;
    let site = bridge.site().unwrap();
    assert_eq!(site.base_url, "https://vpn.example.com");
    assert_eq!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Sent
    );
    let raw = letters.lock().unwrap()[0].clone();
    let unfolded = raw.replace("\n ", " ").replace("\n\t", " ");
    assert!(unfolded.contains("List-Unsubscribe:"), "{raw}");
    assert!(unfolded.contains(&format!("<{UNSUBSCRIBE}>")), "{raw}");
    assert!(unfolded.contains("List-Unsubscribe-Post: List-Unsubscribe=One-Click"));
    let lower = unfolded.to_ascii_lowercase();
    assert!(lower.contains("multipart/alternative"), "{raw}");
    assert!(lower.contains("text/plain") && lower.contains("text/html"));
    assert_eq!(
        bridge.send(&rich("nobody@example.com")).await,
        MailDelivery::Rejected("Почтовый сервер отказал, код 550: 5.1.1 No such user here".into())
    );
    assert!(matches!(
        bridge.send(&rich("не адрес")).await,
        MailDelivery::Rejected(_)
    ));
    let closed = {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    };
    configure(services, admin, closed).await;
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Off(_)
    ));
    services
        .settings_ops()
        .toggle_module(ModuleId::Mail, false, admin)
        .await
        .unwrap();
    assert!(bridge.site().unwrap_err().contains("выключен"));
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Off(reason) if reason.contains("выключен")
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn письмо_рассылки_через_resend_разбирает_ответы() {
    use sn_modules::{MailDelivery, MailSender};
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let mut config = serde_json::Map::new();
    config.insert("transport".into(), serde_json::json!("resend"));
    config.insert("resend_api_key".into(), serde_json::json!("re_test"));
    config.insert("from_email".into(), serde_json::json!("robot@example.com"));
    let ops = services.settings_ops();
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config,
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Mail, true, admin)
        .await
        .unwrap();
    site_ready(services).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/emails"))
        .and(body_partial_json(serde_json::json!({
            "to": ["alex@example.com"],
            "subject": "Новые серверы",
            "text": "Текст письма",
            "html": "<p>Текст <b>письма</b></p>",
            "headers": {
                "List-Unsubscribe": format!("<{UNSUBSCRIBE}>"),
                "List-Unsubscribe-Post": "List-Unsubscribe=One-Click"
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "1" })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let replies = [
        ResponseTemplate::new(429)
            .insert_header("retry-after", "3")
            .set_body_json(serde_json::json!({ "message": "Too many requests" })),
        ResponseTemplate::new(422).set_body_json(serde_json::json!({
            "message": "Invalid `to` field. The email address needs to follow the `email@example.com` format."
        })),
        ResponseTemplate::new(422).set_body_json(
            serde_json::json!({ "message": "The example.com domain is not verified" }),
        ),
        ResponseTemplate::new(500),
    ];
    for reply in replies {
        Mock::given(method("POST"))
            .and(path("/emails"))
            .respond_with(reply)
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    let bridge = crate::mail::MailBridge::new(services.clone()).with_resend_base(server.uri());
    assert_eq!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Sent
    );
    assert_eq!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Wait(std::time::Duration::from_secs(3))
    );
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Rejected(reason) if reason.contains("`to`")
    ));
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Off(reason) if reason.contains("not verified")
    ));
    assert!(matches!(
        bridge.send(&rich("alex@example.com")).await,
        MailDelivery::Failed { retry: true, .. }
    ));
    env.db.cleanup().await;
}

#[tokio::test]
async fn пароль_восстанавливается_кодом_из_письма_при_входе_по_паролю() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.21");
    auth.email_register("olga@example.com", "пароль-ольги-1", &who)
        .await
        .unwrap();
    assert_eq!(
        code(auth.email_reset_start("olga@example.com", &who).await),
        "auth.reset_unavailable"
    );
    let (port, letters) = fake_smtp().await;
    configure_with(
        services,
        admin,
        port,
        serde_json::json!({ "login": "password" }),
    )
    .await;
    assert!(services.mail().available());
    assert!(!services.mail().code_login());
    assert_eq!(
        code(auth.email_login_start("olga@example.com", &who).await),
        "auth.email_password_mode"
    );
    auth.email_password_login("olga@example.com", "пароль-ольги-1", &who)
        .await
        .unwrap();
    auth.email_reset_start("nobody@example.com", &who)
        .await
        .unwrap();
    assert!(letters.lock().unwrap().is_empty());
    auth.email_reset_start("Olga@Example.com", &who)
        .await
        .unwrap();
    let letter = readable(&letters.lock().unwrap()[0]);
    assert!(letter.contains("Код для смены пароля"), "{letter}");
    let sent_code = code_in(&letter);
    let wrong = if sent_code == "000000" {
        "111111"
    } else {
        "000000"
    };
    assert_eq!(
        code(
            auth.email_reset_confirm("olga@example.com", wrong, "новый-пароль-оли", &who)
                .await
        ),
        "auth.code_invalid"
    );
    let login = auth
        .email_reset_confirm("olga@example.com", &sent_code, "новый-пароль-оли", &who)
        .await
        .unwrap();
    assert!(matches!(login, CabinetLogin::Session(_)));
    assert_eq!(
        code(
            auth.email_password_login("olga@example.com", "пароль-ольги-1", &who)
                .await
        ),
        "auth.wrong_password"
    );
    auth.email_password_login("olga@example.com", "новый-пароль-оли", &who)
        .await
        .unwrap();
    assert_eq!(
        code(
            auth.email_reset_confirm("olga@example.com", &sent_code, "ещё-один-пароль", &who)
                .await
        ),
        "auth.code_invalid"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn пароль_меняется_в_кабинете_и_выдаётся_поддержкой() {
    let registry = ModuleRegistry::new(vec![Arc::new(MailModule)]).unwrap();
    let Some(env) = env_with(registry).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let who = meta("203.0.113.22");
    auth.email_register("ivan@example.com", "пароль-ивана-1", &who)
        .await
        .unwrap();
    let client = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(IdentityKind::Email, "ivan@example.com")
            .await
            .unwrap()
            .unwrap()
    };
    assert!(auth.has_password(client.id).await.unwrap());
    let principal = ClientPrincipal {
        client_id: client.id,
        channel: ClientChannel::Cabinet,
        full_access: true,
        session_id: None,
    };
    assert_eq!(
        code(
            auth.change_password(&principal, Some("чужой-пароль-1"), "новый-пароль-ивана")
                .await
        ),
        "auth.wrong_password"
    );
    auth.change_password(&principal, Some("пароль-ивана-1"), "новый-пароль-ивана")
        .await
        .unwrap();
    auth.email_password_login("ivan@example.com", "новый-пароль-ивана", &who)
        .await
        .unwrap();
    let issued = auth
        .issue_client_password(client.id, Actor::Admin(admin))
        .await
        .unwrap();
    assert_eq!(issued.chars().count(), 16);
    assert_eq!(
        code(
            auth.email_password_login("ivan@example.com", "новый-пароль-ивана", &who)
                .await
        ),
        "auth.wrong_password"
    );
    auth.email_password_login("ivan@example.com", &issued, &who)
        .await
        .unwrap();
    let telegram_only = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 7_100_200,
                username: None,
                first_name: Some("Пётр".into()),
                last_name: None,
                language_code: None,
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap()
        .0;
    assert_eq!(
        code(
            auth.issue_client_password(telegram_only.id, Actor::Admin(admin))
                .await
        ),
        "client.no_email"
    );
    env.db.cleanup().await;
}
