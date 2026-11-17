use super::*;
use sn_domain::module::ModuleId;
use sn_domain::settings::keys;
use sn_services::auth::RequestMeta;
use sn_services::settings::ModuleConfigUpdate;
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

fn decoded(letter: &str) -> String {
    use base64::Engine;
    let mut out = letter.to_string();
    let mut block = String::new();
    let flush = |block: &mut String, out: &mut String| {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(block.as_bytes()) {
            out.push('\n');
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
        block.clear();
    };
    for line in letter.lines().map(str::trim) {
        let encoded = line.len() >= 16
            && line
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=');
        if encoded {
            block.push_str(line);
        } else if !block.is_empty() {
            flush(&mut block, &mut out);
        }
    }
    if !block.is_empty() {
        flush(&mut block, &mut out);
    }
    out
}

async fn mail_on(services: &Services, port: u16, extra: serde_json::Value) {
    let (owner, _) = services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    let mut config = serde_json::Map::new();
    config.insert("transport".into(), serde_json::json!("smtp"));
    config.insert("smtp_host".into(), serde_json::json!("127.0.0.1"));
    config.insert("smtp_port".into(), serde_json::json!(port));
    config.insert("security".into(), serde_json::json!("none"));
    config.insert("from_email".into(), serde_json::json!("robot@example.com"));
    config.insert("login".into(), serde_json::json!("password"));
    if let serde_json::Value::Object(extra) = extra {
        config.extend(extra);
    }
    let ops = services.settings_ops();
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config,
            clear: Vec::new(),
        },
        owner,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Mail, true, owner)
        .await
        .unwrap();
}

#[tokio::test]
async fn клиенту_без_телеграма_уведомление_приходит_письмом() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::PUBLIC_URL,
                &serde_json::json!("https://vpn.example.com"),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    services
        .auth()
        .email_register("nina@example.com", "пароль-нины-1", &RequestMeta::default())
        .await
        .unwrap();
    let client = {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(sn_domain::client::IdentityKind::Email, "nina@example.com")
            .await
            .unwrap()
            .unwrap()
    };
    let handler = NotifyHandler::new(&BotConfig {
        api_base: server.uri(),
        ..BotConfig::default()
    });
    let reply =
        serde_json::json!({"body": "Продлили подписку, обновите её в приложении", "ticket_id": 41});
    assert_eq!(
        notify(
            &services,
            &handler,
            client.id,
            notify::templates::SUPPORT_REPLY,
            reply.clone()
        )
        .await,
        JobResult::Done
    );
    let (port, letters) = fake_smtp().await;
    mail_on(
        &services,
        port,
        serde_json::json!({ "letter_payments": false }),
    )
    .await;
    assert_eq!(
        notify(
            &services,
            &handler,
            client.id,
            notify::templates::SUPPORT_REPLY,
            reply
        )
        .await,
        JobResult::Done
    );
    assert_eq!(
        notify(
            &services,
            &handler,
            client.id,
            notify::templates::PAYMENT_PAID,
            serde_json::json!({"amount": {"minor": 49900, "currency": "RUB"}})
        )
        .await,
        JobResult::Done
    );
    let sent = letters.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "письмо об оплате выключено");
    let letter = decoded(&sent[0]);
    assert!(
        letter.contains("Продлили подписку, обновите её в приложении"),
        "{letter}"
    );
    assert!(
        letter.contains("https://vpn.example.com/cabinet/help/41"),
        "{letter}"
    );
    assert!(!letter.contains("Отписаться"), "{letter}");
    assert!(sent[0].contains("nina@example.com"));
    assert!(sent.iter().all(|l| !l.contains("api.telegram")));
    assert!(sent_to_telegram(&server).await.is_empty());
    db.cleanup().await;
}

async fn sent_to_telegram(server: &MockServer) -> Vec<serde_json::Value> {
    sent(server).await
}
