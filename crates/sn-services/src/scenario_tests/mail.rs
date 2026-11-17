use super::*;
use crate::settings::ModuleConfigUpdate;
use base64::Engine;
use sn_modules::catalog::mail::MailModule;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

type Letters = Arc<StdMutex<Vec<String>>>;

async fn fake_smtp() -> (u16, Letters) {
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

fn readable(letter: &str) -> String {
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

fn code_in(text: &str) -> String {
    text.split(|c: char| !c.is_ascii_digit())
        .find(|part| part.len() == crate::login::EMAIL_CODE_DIGITS)
        .unwrap()
        .to_string()
}

async fn configure(services: &Services, admin: AdminId, port: u16) {
    let mut config = serde_json::Map::new();
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
