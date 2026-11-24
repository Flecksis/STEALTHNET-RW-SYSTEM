use super::mini_account::mini_http;
use super::*;
use sn_domain::module::ModuleId;
use sn_services::settings::ModuleConfigUpdate;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const REFUSAL: &str = "550 5.1.1 <alex@example.com>: Recipient address rejected: unknown user at relay mx7.internal.example";

async fn refusing_smtp() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                if write.write_all(b"220 relay ESMTP\r\n").await.is_err() {
                    return;
                }
                while let Ok(Some(line)) = lines.next_line().await {
                    let upper = line.to_ascii_uppercase();
                    let reply = if upper.starts_with("EHLO") {
                        "250-relay\r\n250 OK\r\n".to_string()
                    } else if upper.starts_with("RCPT") {
                        format!("{REFUSAL}\r\n")
                    } else if upper.starts_with("QUIT") {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        "250 OK\r\n".to_string()
                    };
                    if write.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

async fn mail_via(h: &Http, port: u16) {
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
        "login": "code",
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

async fn ask_code(h: &Http, email: &str, lang: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/client/auth/email")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(lang) = lang {
        request = request.header(crate::lang::HEADER, lang);
    }
    let response = router(h.state.clone())
        .oneshot(
            request
                .body(Body::from(json!({ "email": email }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

fn assert_no_details(body: &Value) {
    let text = body.to_string();
    for detail in [
        "550",
        "5.1.1",
        "Recipient",
        "mx7",
        "127.0.0.1",
        "почтовым сервером",
    ] {
        assert!(!text.contains(detail), "{detail}: {text}");
    }
}

#[tokio::test]
async fn клиент_получает_общую_ошибку_почты_на_русском_и_английском_а_админ_подробности() {
    let Some(h) = mini_http().await else { return };
    mail_via(&h, refusing_smtp().await).await;
    let owner = h.login("owner@example.com").await;
    let (status, body) = ask_code(&h, "alex@example.com", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "mail.address");
    assert_eq!(
        body["error"]["message"],
        "Не получилось отправить письмо на этот адрес. Проверьте адрес почты."
    );
    assert_no_details(&body);
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/languages",
            &owner,
            json!({ "english": true, "default": "ru" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = ask_code(&h, "kate@example.com", Some("en")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["message"],
        "We couldn't send the email to this address. Please check it."
    );
    assert_no_details(&body);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/mail/test",
            &owner,
            json!({ "to": "alex@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let shown = body["error"]["message"].as_str().unwrap();
    assert!(shown.contains("550"), "{shown}");
    assert!(shown.contains("mx7.internal.example"), "{shown}");
    let closed = {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    };
    mail_via(&h, closed).await;
    let (status, body) = ask_code(&h, "lena@example.com", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "mail.failed");
    assert_eq!(
        body["error"]["message"],
        "Не получилось отправить письмо. Попробуйте позже."
    );
    assert_no_details(&body);
    let (_, body) = ask_code(&h, "nick@example.com", Some("en")).await;
    assert_eq!(
        body["error"]["message"],
        "We couldn't send the email. Please try again later."
    );
    h.db.cleanup().await;
}
