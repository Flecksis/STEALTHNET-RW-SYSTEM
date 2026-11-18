use super::client_letters::{Letters, decoded, fake_smtp, mail_on};
use super::*;
use crate::telegram::{Button, Keyboard};
use sn_domain::client::Client;
use sn_domain::settings::{ClientAccess, keys};
use sn_services::auth::RequestMeta;

const USER: i64 = 7301;

async fn site(services: &Services) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings()
        .set(
            keys::PUBLIC_URL,
            &serde_json::json!("https://vpn.example.com"),
            None,
        )
        .await
        .unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

async fn no_miniapp(services: &Services) {
    let (owner, _) = services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    services
        .settings_ops()
        .update_client_access(
            &ClientAccess {
                miniapp: false,
                ..ClientAccess::default()
            },
            owner,
        )
        .await
        .unwrap();
}

async fn web_apps(server: &MockServer) -> Vec<String> {
    sent(server)
        .await
        .last()
        .and_then(|b| b["reply_markup"]["inline_keyboard"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|row| row.as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["web_app"]["url"].as_str().map(str::to_string))
        .collect()
}

async fn deleted(server: &MockServer) -> Vec<i64> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/deleteMessage"))
        .filter_map(|r| serde_json::from_slice::<serde_json::Value>(&r.body).ok())
        .filter_map(|b| b["message_id"].as_i64())
        .collect()
}

fn code_of(letters: &Letters, index: usize) -> String {
    let letter = decoded(&letters.lock().unwrap()[index]);
    let at = letter.find("Код для смены почты").unwrap_or(0);
    letter[at..]
        .split(|c: char| !c.is_ascii_digit())
        .find(|part| part.len() == 6)
        .unwrap()
        .to_string()
}

async fn client(services: &Services) -> Client {
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .find_by_identity(IdentityKind::Telegram, &USER.to_string())
        .await
        .unwrap()
        .unwrap()
}

async fn email_of(services: &Services) -> Option<(String, bool)> {
    let id = client(services).await.id;
    let mut conn = services.store().conn().await.unwrap();
    conn.clients()
        .identities(id)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.kind == IdentityKind::Email)
        .map(|i| (i.value, i.verified))
}

async fn waiting(services: &Services) -> Option<Awaiting> {
    Session::load(services, USER).await.unwrap().awaiting
}

async fn link(ctx: &BotCtx, server: &MockServer, letters: &Letters, first: i64) {
    handle_update(ctx, callback_update(first, USER, "em:new")).await;
    handle_update(ctx, text_update(first + 1, USER, "Alex@Example.com")).await;
    let text = last_text(server).await;
    assert!(
        text.contains("Код отправлен на alex@example.com") && text.contains("15\u{a0}минут"),
        "{text}"
    );
    let last = letters.lock().unwrap().len() - 1;
    let code = code_of(letters, last);
    handle_update(ctx, text_update(first + 2, USER, &code)).await;
}

#[tokio::test]
async fn бот_привязывает_почту_кодом_и_сразу_предлагает_задать_пароль_в_приложении() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    handle_update(&ctx, text_update(1, USER, "/start")).await;
    assert!(!last_buttons(&server).await.contains(&"em".to_string()));
    let (port, letters) = fake_smtp().await;
    mail_on(&services, port, serde_json::json!({})).await;
    handle_update(&ctx, text_update(2, USER, "/start")).await;
    assert!(last_buttons(&server).await.contains(&"em".to_string()));
    assert!(
        last_labels(&server)
            .await
            .contains(&"Вход по почте".to_string())
    );

    handle_update(&ctx, callback_update(3, USER, "em")).await;
    let text = last_text(&server).await;
    assert!(text.contains("Почта не привязана"), "{text}");
    assert_eq!(last_buttons(&server).await, ["em:new", "m"]);

    handle_update(&ctx, callback_update(4, USER, "em:new")).await;
    assert!(last_text(&server).await.contains("Пришлите адрес почты"));
    assert_eq!(waiting(&services).await, Some(Awaiting::EmailAddress));
    handle_update(&ctx, text_update(5, USER, "не почта")).await;
    assert!(
        last_text(&server)
            .await
            .contains("В адресе ошибка. Пришлите адрес целиком")
    );
    assert_eq!(waiting(&services).await, Some(Awaiting::EmailAddress));
    assert!(letters.lock().unwrap().is_empty());

    handle_update(&ctx, text_update(6, USER, "Alex@Example.com")).await;
    let text = last_text(&server).await;
    assert!(
        text.contains("Код отправлен на alex@example.com") && text.contains("15\u{a0}минут"),
        "{text}"
    );
    assert_eq!(last_buttons(&server).await, ["em:again", "em:new", "em"]);
    assert_eq!(waiting(&services).await, Some(Awaiting::EmailCode));
    let code = code_of(&letters, 0);
    let wrong = if code == "000000" { "111111" } else { "000000" };
    handle_update(&ctx, text_update(7, USER, wrong)).await;
    let text = last_text(&server).await;
    assert!(text.contains("Код не подошёл"), "{text}");
    assert!(
        text.contains("Пришлите код из письма на alex@example.com"),
        "{text}"
    );
    assert_eq!(email_of(&services).await, None);

    handle_update(&ctx, text_update(8, USER, &code)).await;
    let text = last_text(&server).await;
    assert!(text.contains("Почта alex@example.com привязана."), "{text}");
    assert!(text.contains("Теперь задайте пароль для сайта"), "{text}");
    assert!(
        text.contains("откройте https://vpn.example.com/cabinet, выберите «Почта»"),
        "{text}"
    );
    assert_eq!(
        web_apps(&server).await,
        ["https://vpn.example.com/app/profile/password"]
    );
    assert_eq!(
        email_of(&services).await,
        Some(("alex@example.com".into(), true))
    );
    assert_eq!(waiting(&services).await, None);
    assert!(
        !services
            .auth()
            .has_password(client(&services).await.id)
            .await
            .unwrap()
    );
    assert!(deleted(&server).await.is_empty());

    handle_update(&ctx, callback_update(9, USER, "em")).await;
    let text = last_text(&server).await;
    assert!(text.contains("Почта: alex@example.com"), "{text}");
    assert!(text.contains("Пароль для сайта не задан."), "{text}");
    assert_eq!(
        web_apps(&server).await,
        ["https://vpn.example.com/app/profile/password"]
    );
    assert_eq!(last_buttons(&server).await, ["em:new", "m"]);
    db.cleanup().await;
}

#[tokio::test]
async fn без_мини_приложения_бот_принимает_пароль_и_сразу_удаляет_сообщение() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    no_miniapp(&services).await;
    let (port, letters) = fake_smtp().await;
    mail_on(&services, port, serde_json::json!({})).await;
    Mock::given(method("POST"))
        .and(path_regex(r"/deleteMessage$"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "ok": false, "error_code": 400, "description": "Bad Request: message can't be deleted"
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/deleteMessage$"))
        .respond_with(ok(serde_json::json!(true)))
        .with_priority(2)
        .mount(&server)
        .await;
    handle_update(&ctx, text_update(1, USER, "/start")).await;
    link(&ctx, &server, &letters, 2).await;
    let text = last_text(&server).await;
    assert!(text.contains("Почта alex@example.com привязана."), "{text}");
    assert!(text.contains("пришлите его следующим сообщением"), "{text}");
    assert!(
        text.contains("может остаться в уведомлениях на телефоне"),
        "{text}"
    );
    assert!(text.contains("https://vpn.example.com/cabinet"), "{text}");
    assert!(web_apps(&server).await.is_empty());
    assert_eq!(waiting(&services).await, Some(Awaiting::SitePassword));
    assert!(deleted(&server).await.is_empty());

    handle_update(&ctx, text_update(10, USER, "короткий")).await;
    assert_eq!(deleted(&server).await, [10]);
    let text = last_text(&server).await;
    assert!(
        text.contains("Не получилось удалить ваше сообщение"),
        "{text}"
    );
    assert!(text.contains("не короче 10"), "{text}");
    assert_eq!(waiting(&services).await, Some(Awaiting::SitePassword));

    handle_update(&ctx, text_update(11, USER, "пароль-клиента-1")).await;
    assert_eq!(deleted(&server).await, [10, 11]);
    let text = last_text(&server).await;
    assert!(!text.contains("Не получилось удалить"), "{text}");
    assert!(text.contains("Пароль для сайта задан."), "{text}");
    assert!(
        text.contains("войдите с адресом alex@example.com и паролем"),
        "{text}"
    );
    assert_eq!(waiting(&services).await, None);
    let owner = client(&services).await.id;
    let login = services
        .auth()
        .email_password_login(
            "alex@example.com",
            "пароль-клиента-1",
            &RequestMeta::default(),
        )
        .await
        .unwrap();
    match login {
        sn_services::two_factor::CabinetLogin::Session(session) => {
            assert_eq!(session.principal.client_id, owner);
        }
        other => panic!("вход по почте не дал сессию: {other:?}"),
    }

    handle_update(&ctx, text_update(12, USER, "ещё-один-пароль")).await;
    assert_eq!(deleted(&server).await, [10, 11]);
    db.cleanup().await;
}

#[tokio::test]
async fn при_входе_по_коду_бот_не_просит_пароль() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    let (port, letters) = fake_smtp().await;
    mail_on(&services, port, serde_json::json!({ "login": "code" })).await;
    handle_update(&ctx, text_update(1, USER, "/start")).await;
    link(&ctx, &server, &letters, 2).await;
    let text = last_text(&server).await;
    assert!(text.contains("Почта alex@example.com привязана."), "{text}");
    assert!(text.contains("нажмите «Прислать код»"), "{text}");
    assert!(!text.contains("пароль"), "{text}");
    assert!(web_apps(&server).await.is_empty());
    assert_eq!(waiting(&services).await, None);
    handle_update(&ctx, callback_update(5, USER, "em")).await;
    let text = last_text(&server).await;
    assert!(!text.contains("Пароль для сайта"), "{text}");
    assert_eq!(last_buttons(&server).await, ["em:new", "m"]);
    db.cleanup().await;
}

#[tokio::test]
async fn без_входа_на_сайт_пункт_почты_скрыт() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    site(&services).await;
    let (port, _letters) = fake_smtp().await;
    mail_on(&services, port, serde_json::json!({})).await;
    let (owner, _) = services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap();
    services
        .settings_ops()
        .update_client_access(
            &ClientAccess {
                site_login: false,
                ..ClientAccess::default()
            },
            owner,
        )
        .await
        .unwrap();
    handle_update(&ctx, text_update(1, USER, "/start")).await;
    assert!(!last_buttons(&server).await.contains(&"em".to_string()));
    handle_update(&ctx, callback_update(2, USER, "em:new")).await;
    assert!(
        last_text(&server)
            .await
            .contains("Привязать почту сейчас нельзя")
    );
    assert_eq!(waiting(&services).await, None);
    db.cleanup().await;
}

#[test]
fn уведомления_о_почте_от_администратора() {
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let set = notify::render(
        &sn_domain::bot_content::Content::default(),
        notify::templates::EMAIL_SET,
        &serde_json::json!({ "email": "nina@example.com" }),
        utc,
    )
    .unwrap();
    assert_eq!(
        set.text,
        "Через поддержку в ваш аккаунт добавлена почта nina@example.com. Если вы об этом не просили, сразу напишите в поддержку."
    );
    assert_eq!(
        set.keyboard,
        Keyboard::new().row(vec![Button::callback("Вход по почте", "em")])
    );
    let removed = notify::render(
        &sn_domain::bot_content::Content::default(),
        notify::templates::EMAIL_REMOVED,
        &serde_json::json!({ "email": "nina@example.com" }),
        utc,
    )
    .unwrap();
    assert!(
        removed
            .text
            .contains("от вашего аккаунта отвязана почта nina@example.com")
    );
    assert_eq!(
        removed.keyboard,
        Keyboard::new().row(vec![Button::callback("Поддержка", "sup")])
    );
    assert!(
        notify::render(
            &sn_domain::bot_content::Content::default(),
            notify::templates::EMAIL_SET,
            &serde_json::json!({}),
            utc,
        )
        .is_none()
    );
}
