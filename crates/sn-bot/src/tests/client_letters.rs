use super::*;
use sn_domain::client::Locale;
use sn_domain::mail_letters::Group;
use sn_domain::module::ModuleId;
use sn_domain::settings::keys;
use sn_services::auth::RequestMeta;
use sn_services::mail_letters::{DraftTexts, LangDrafts, LangSigns, LettersUpdate};
use sn_services::settings::ModuleConfigUpdate;
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

pub(super) fn decoded(letter: &str) -> String {
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
    let joined = letter.replace("=\n", "");
    let bytes = joined.as_bytes();
    let mut plain = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'='
            && at + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&joined[at + 1..at + 3], 16)
        {
            plain.push(byte);
            at += 3;
            continue;
        }
        plain.push(bytes[at]);
        at += 1;
    }
    out.push('\n');
    out.push_str(&String::from_utf8_lossy(&plain));
    out
}

pub(super) async fn mail_on(services: &Services, port: u16, extra: serde_json::Value) {
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
            notify::templates::TOPUP_DONE,
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

fn subject_of(raw: &str) -> String {
    use base64::Engine;
    let mut header = String::new();
    let mut lines = raw.lines();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("Subject:") {
            header.push_str(rest.trim());
            for next in lines.by_ref() {
                if next.starts_with(' ') || next.starts_with('\t') {
                    header.push(' ');
                    header.push_str(next.trim());
                } else {
                    break;
                }
            }
            break;
        }
    }
    let mut out = String::new();
    let mut encoded = false;
    for word in header.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        if lower.starts_with("=?utf-8?b?") && word.ends_with("?=") {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&word[10..word.len() - 2])
                .unwrap();
            out.push_str(&String::from_utf8_lossy(&bytes));
            encoded = true;
        } else {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
            encoded = false;
        }
    }
    let _ = encoded;
    out
}

async fn site_on(services: &Services) {
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
}

async fn email_client(services: &Services, email: &str) -> ClientId {
    services
        .auth()
        .email_register(email, "пароль-клиента-1", &RequestMeta::default())
        .await
        .unwrap();
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .find_by_identity(sn_domain::client::IdentityKind::Email, email)
        .await
        .unwrap()
        .unwrap()
        .id
}

async fn owner(services: &Services) -> sn_domain::ids::AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

fn handler_for(server: &MockServer) -> NotifyHandler {
    NotifyHandler::new(&BotConfig {
        api_base: server.uri(),
        ..BotConfig::default()
    })
}

#[tokio::test]
async fn о_пополнении_баланса_бот_пишет_и_приходит_письмо() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site_on(&services).await;
    handle_update(&ctx, text_update(1, 611, "/start")).await;
    let telegram = client_of(&services, 611).await.unwrap();
    let mailed = email_client(&services, "topup@example.com").await;
    let (port, letters) = fake_smtp().await;
    mail_on(&services, port, serde_json::json!({})).await;
    let handler = handler_for(&server);
    let amount = serde_json::json!({"amount": {"minor": 1000, "currency": "USD"}});
    assert_eq!(
        notify(
            &services,
            &handler,
            telegram.id,
            notify::templates::TOPUP_DONE,
            amount.clone()
        )
        .await,
        JobResult::Done
    );
    let message = sent(&server).await.last().cloned().unwrap();
    assert_eq!(message["text"], "Баланс пополнен на $10.00.");
    assert_eq!(
        message["reply_markup"]["inline_keyboard"][0][0]["text"],
        "Баланс"
    );
    assert_eq!(
        notify(
            &services,
            &handler,
            mailed,
            notify::templates::TOPUP_DONE,
            amount
        )
        .await,
        JobResult::Done
    );
    let sent_mail = letters.lock().unwrap().clone();
    assert_eq!(sent_mail.len(), 1);
    assert_eq!(subject_of(&sent_mail[0]), "Баланс пополнен");
    let letter = decoded(&sent_mail[0]);
    assert!(letter.contains("Баланс пополнен на $10.00."), "{letter}");
    assert!(
        letter.contains("https://vpn.example.com/cabinet/payments"),
        "{letter}"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn повторная_оплата_по_оплаченному_счёту_приходит_письмом() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    site_on(&services).await;
    let client = email_client(&services, "vera@example.com").await;
    let (port, letters) = fake_smtp().await;
    mail_on(&services, port, serde_json::json!({})).await;
    let handler = handler_for(&server);
    let credited = serde_json::json!({
        "text": "Повторная оплата 499 ₽ по уже оплаченному счёту зачислена на баланс.",
        "text_en": "A repeat payment of ₽499 for an already paid invoice is on your balance.",
        "amount": {"minor": 49900, "currency": "RUB"},
    });
    let review = serde_json::json!({
        "text": "Повторная оплата 499 ₽ по уже оплаченному счёту получена. Проверим её вручную и свяжемся с вами.",
        "amount": {"minor": 49900, "currency": "RUB"},
    });
    for (template, params) in [
        (notify::templates::REPEAT_CREDITED, credited),
        (notify::templates::REPEAT_REVIEW, review),
    ] {
        assert_eq!(
            notify(&services, &handler, client, template, params).await,
            JobResult::Done
        );
    }
    let sent_mail = letters.lock().unwrap().clone();
    assert_eq!(sent_mail.len(), 2, "оба письма о повторной оплате ушли");
    assert_eq!(subject_of(&sent_mail[0]), "Деньги зачислены на баланс");
    let first = decoded(&sent_mail[0]);
    assert!(
        first.contains("Повторная оплата 499 ₽ по уже оплаченному счёту зачислена на баланс."),
        "{first}"
    );
    assert_eq!(subject_of(&sent_mail[1]), "Оплату проверим вручную");
    assert!(decoded(&sent_mail[1]).contains("Проверим её вручную"));
    assert!(sent_to_telegram(&server).await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn напоминание_до_автопродления_приходит_со_своей_темой() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    site_on(&services).await;
    let client = email_client(&services, "pavel@example.com").await;
    let (port, letters) = fake_smtp().await;
    mail_on(&services, port, serde_json::json!({})).await;
    let handler = handler_for(&server);
    let price = serde_json::json!({"minor": 69900, "currency": "RUB"});
    let short = serde_json::json!({"minor": 20000, "currency": "RUB"});
    let cases = [
        (
            notify::templates::AUTORENEW_REMINDER,
            serde_json::json!({"expire_at": "2027-02-14T10:00:00Z", "amount": price, "missing": short, "balance_off": false}),
        ),
        (
            notify::templates::AUTORENEW_CHARGE_FAILED,
            serde_json::json!({"expire_at": "2027-02-14T10:00:00Z", "amount": price, "balance_off": false}),
        ),
        (
            notify::templates::AUTORENEW_REMINDER,
            serde_json::json!({"expire_at": "2027-02-14T10:00:00Z", "amount": price, "missing": short, "balance_off": true}),
        ),
    ];
    for (template, params) in cases {
        assert_eq!(
            notify(&services, &handler, client, template, params).await,
            JobResult::Done
        );
    }
    let sent_mail = letters.lock().unwrap().clone();
    assert_eq!(sent_mail.len(), 3);
    assert_eq!(
        subject_of(&sent_mail[0]),
        "Пополните баланс для автопродления"
    );
    let reminder = decoded(&sent_mail[0]);
    assert!(
        reminder.contains("на балансе не хватает 200\u{a0}₽"),
        "{reminder}"
    );
    assert!(
        reminder.contains("https://vpn.example.com/cabinet/topup"),
        "{reminder}"
    );
    assert_eq!(subject_of(&sent_mail[1]), "Подписку не удалось продлить");
    assert_eq!(subject_of(&sent_mail[2]), "Продлите подписку вручную");
    db.cleanup().await;
}

#[tokio::test]
async fn письмо_уходит_по_шаблону_оператора_на_языке_клиента() {
    let Some((db, services, server, _ctx)) = setup().await else {
        return;
    };
    site_on(&services).await;
    let owner = owner(&services).await;
    services
        .settings_ops()
        .update_languages(
            &sn_domain::client::Languages {
                english: true,
                default: Locale::Ru,
            },
            owner,
        )
        .await
        .unwrap();
    let russian = email_client(&services, "olga@example.com").await;
    let english = email_client(&services, "john@example.com").await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.clients()
            .set_locale(english, Locale::En)
            .await
            .unwrap();
    }
    let (port, letters) = fake_smtp().await;
    mail_on(&services, port, serde_json::json!({})).await;
    let mut update = LettersUpdate {
        sign: LangSigns {
            ru: Some("С заботой, {brand}".into()),
            en: None,
        },
        ..LettersUpdate::default()
    };
    update.letters.insert(
        "support_reply".into(),
        LangDrafts {
            ru: Some(DraftTexts {
                subject: "Ответили на обращение №{ticket}".into(),
                text: "Здравствуйте!\n\n{reply}".into(),
                button: "Читать переписку".into(),
            }),
            en: Some(DraftTexts {
                subject: "We answered request #{ticket}".into(),
                text: "Hello!\n\n{reply}".into(),
                button: String::new(),
            }),
        },
    );
    update.groups.insert(Group::Payments, false);
    services.letters().update(&update, owner).await.unwrap();
    let handler = handler_for(&server);
    let reply = serde_json::json!({"body": "Продлили <b>подписку</b> на 3 дня", "ticket_id": 41});
    for client in [russian, english] {
        assert_eq!(
            notify(
                &services,
                &handler,
                client,
                notify::templates::SUPPORT_REPLY,
                reply.clone()
            )
            .await,
            JobResult::Done
        );
    }
    assert_eq!(
        notify(
            &services,
            &handler,
            russian,
            notify::templates::TOPUP_DONE,
            serde_json::json!({"amount": {"minor": 1000, "currency": "USD"}})
        )
        .await,
        JobResult::Done
    );
    let sent_mail = letters.lock().unwrap().clone();
    assert_eq!(sent_mail.len(), 2, "письма об оплате выключены");
    assert_eq!(subject_of(&sent_mail[0]), "Ответили на обращение №41");
    let ru = decoded(&sent_mail[0]);
    assert!(ru.contains("Здравствуйте!"), "{ru}");
    assert!(
        ru.contains("Продлили &lt;b&gt;подписку&lt;/b&gt; на 3 дня"),
        "{ru}"
    );
    assert!(ru.contains("Продлили <b>подписку</b> на 3 дня"), "{ru}");
    assert!(ru.contains("Читать переписку"), "{ru}");
    assert!(
        ru.contains("https://vpn.example.com/cabinet/help/41"),
        "{ru}"
    );
    assert!(ru.contains("С заботой, STEALTHNET"), "{ru}");
    assert_eq!(subject_of(&sent_mail[1]), "We answered request #41");
    let en = decoded(&sent_mail[1]);
    assert!(en.contains("Hello!"), "{en}");
    assert!(en.contains("Open the conversation"), "{en}");
    assert!(en.contains("The STEALTHNET team"), "{en}");
    db.cleanup().await;
}
