use super::*;
use crate::{EnvFallbacks, FixedClock, ServiceDeps};
use chrono::{TimeZone, Utc};
use sn_domain::settings::{ClientAccess, keys};
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_store::SecretBox;
use sn_store::testing::{TestDb, test_store};
use std::sync::Arc;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "123456:BOTTOKEN";

struct Env {
    db: TestDb,
    services: Services,
    telegram: MockServer,
}

async fn env_with(token: Option<&str>) -> Option<Env> {
    let telegram = MockServer::start().await;
    let api = telegram.uri();
    env_at(token, telegram, &api).await
}

async fn env_at(token: Option<&str>, telegram: MockServer, api: &str) -> Option<Env> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::with_telegram_api(api)),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(FixedClock::new(
            Utc.with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap(),
        )),
        env: EnvFallbacks {
            bot_token: token.map(str::to_string),
            telegram_api: Some(api.to_string()),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    Some(Env {
        db,
        services,
        telegram,
    })
}

async fn env() -> Option<Env> {
    env_with(Some(TOKEN)).await
}

impl Env {
    async fn public_url(&self, url: &str) {
        let mut conn = self.services.store().conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &json!(url), None)
            .await
            .unwrap();
        drop(conn);
        self.services.reload().await.unwrap();
    }

    async fn access(&self, access: ClientAccess) -> AdminId {
        let admin = self.owner().await;
        self.services
            .settings_ops()
            .update_client_access(&access, admin)
            .await
            .unwrap();
        admin
    }

    async fn owner(&self) -> AdminId {
        self.services
            .auth()
            .upsert_owner("owner@example.com", "очень-надёжный-пароль")
            .await
            .unwrap()
            .0
    }

    async fn answer(&self, method_name: &str, result: Value) {
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/{method_name}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "result": result })),
            )
            .mount(&self.telegram)
            .await;
    }

    async fn refuse(&self, method_name: &str, code: u16, description: &str) {
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/{method_name}")))
            .respond_with(ResponseTemplate::new(code).set_body_json(
                json!({ "ok": false, "error_code": code, "description": description }),
            ))
            .mount(&self.telegram)
            .await;
    }

    async fn calls(&self, method_name: &str) -> Vec<Value> {
        self.telegram
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.url.path().ends_with(&format!("/{method_name}")))
            .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
            .collect()
    }

    async fn audit(&self) -> Vec<(String, Value)> {
        sqlx::query_as::<_, (String, Value)>(
            "SELECT action, details FROM audit_log WHERE action = $1 ORDER BY id",
        )
        .bind(MENU_ACTION)
        .fetch_all(self.services.store().pool())
        .await
        .unwrap()
    }
}

fn app(text: &str, url: &str) -> MenuButton {
    MenuButton::WebApp {
        text: text.into(),
        url: url.into(),
    }
}

#[test]
fn кнопка_читается_и_пишется_как_в_telegram() {
    for button in [
        MenuButton::Default,
        MenuButton::Commands,
        app("Приложение", "https://vpn.example.com/app"),
    ] {
        assert_eq!(MenuButton::parse(&button.json()), button);
    }
    assert_eq!(MenuButton::parse(&json!(null)), MenuButton::Default);
    assert_eq!(
        MenuButton::parse(&json!({ "type": "что-то новое" })),
        MenuButton::Default
    );
    assert_eq!(
        MenuButton::parse(&json!({ "type": "web_app", "text": "Открыть" })),
        app("Открыть", "")
    );
    assert_eq!(MenuButton::Default.key(), None);
    assert_eq!(MenuButton::Commands.key().as_deref(), Some("commands"));
    assert_eq!(
        app("Приложение", "https://vpn.example.com/app")
            .key()
            .as_deref(),
        Some("https://vpn.example.com/app\nПриложение")
    );
}

#[test]
fn кнопки_сравниваются_по_адресу_а_не_по_подписи() {
    let ours = app("Приложение", "https://vpn.example.com/app");
    assert!(ours.same_place(&app("Открыть", "https://VPN.example.com/app/")));
    assert!(!ours.same_place(&app("Приложение", "https://vpn.example.com/cabinet")));
    assert!(!ours.same_place(&app("Приложение", "https://vpn.example.com")));
    assert!(!ours.same_place(&app("Приложение", "")));
    assert!(!ours.same_place(&MenuButton::Commands));
    assert!(!MenuButton::Commands.same_place(&ours));
    assert!(MenuButton::Commands.same_place(&MenuButton::Default));
    assert!(MenuButton::Default.same_place(&MenuButton::Commands));
    assert!(
        app("App", "https://впн.рф/app").same_place(&app("App", "https://xn--b1awf.xn--p1ai/app"))
    );
}

#[tokio::test]
async fn какая_кнопка_нужна_решают_адрес_и_переключатели() {
    let Some(env) = env().await else { return };
    let buttons = env.services.bot_buttons();
    assert_eq!(buttons.wanted(), MenuButton::Default);
    env.public_url("http://vpn.example.com").await;
    assert_eq!(buttons.wanted(), MenuButton::Default);
    env.public_url("https://vpn.example.com/").await;
    let MenuButton::WebApp { text, url } = buttons.wanted() else {
        panic!("ожидалась кнопка приложения");
    };
    assert_eq!(url, "https://vpn.example.com/app");
    assert!(!text.trim().is_empty());
    env.access(ClientAccess {
        menu_button: false,
        ..ClientAccess::default()
    })
    .await;
    assert_eq!(buttons.wanted(), MenuButton::Commands);
    env.access(ClientAccess {
        menu_button: false,
        miniapp: false,
        ..ClientAccess::default()
    })
    .await;
    assert_eq!(buttons.wanted(), MenuButton::Default);
    env.access(ClientAccess {
        miniapp: false,
        ..ClientAccess::default()
    })
    .await;
    assert_eq!(buttons.wanted(), MenuButton::Default);
    env.db.cleanup().await;
}

#[tokio::test]
async fn видно_что_в_telegram_стоит_та_же_кнопка() {
    let Some(env) = env().await else { return };
    env.public_url("https://vpn.example.com").await;
    let buttons = env.services.bot_buttons();
    let wanted = buttons.wanted();
    env.answer("getChatMenuButton", wanted.json()).await;
    env.answer(
        "getMe",
        json!({ "id": 123456, "is_bot": true, "username": "vpn_example_bot", "has_main_web_app": true }),
    )
    .await;
    let view = buttons.view().await.unwrap();
    assert_eq!(
        view,
        ButtonsView {
            username: Some("vpn_example_bot".into()),
            wanted: wanted.clone(),
            menu: wanted.clone(),
            same: true,
            applied: false,
            main_app: true,
        }
    );
    buttons.remember(&wanted).await.unwrap();
    assert!(buttons.view().await.unwrap().applied);
    assert!(env.calls("setChatMenuButton").await.is_empty());
    let asked = env.calls("getChatMenuButton").await;
    assert_eq!(asked, vec![json!({}), json!({})]);
    let shown = serde_json::to_value(buttons.view().await.unwrap()).unwrap();
    assert_eq!(shown["menu"]["type"], "web_app");
    assert_eq!(shown["menu"]["url"], "https://vpn.example.com/app");
    assert_eq!(shown["main_app"], true);
    assert!(!shown.to_string().contains(TOKEN));
    env.db.cleanup().await;
}

#[tokio::test]
async fn чужая_кнопка_видна_и_ставится_заново() {
    let Some(env) = env().await else { return };
    env.public_url("https://vpn.example.com").await;
    let buttons = env.services.bot_buttons();
    let wanted = buttons.wanted();
    buttons.remember(&wanted).await.unwrap();
    let foreign = app("Кабинет", "https://vpn.example.com/cabinet");
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/getChatMenuButton")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "ok": true, "result": foreign.json() })),
        )
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&env.telegram)
        .await;
    env.answer("getChatMenuButton", wanted.json()).await;
    env.answer(
        "getMe",
        json!({ "id": 123456, "is_bot": true, "username": "vpn_example_bot" }),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{TOKEN}/setChatMenuButton")))
        .and(body_json(json!({ "menu_button": wanted.json() })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "result": true })),
        )
        .expect(1)
        .mount(&env.telegram)
        .await;
    let view = buttons.view().await.unwrap();
    assert_eq!(view.menu, foreign);
    assert!(!view.same && view.applied && !view.main_app);
    let admin = env.owner().await;
    let view = buttons.put_menu(admin).await.unwrap();
    assert!(view.same && view.applied);
    assert_eq!(view.menu, wanted);
    assert_eq!(
        env.audit().await,
        vec![(
            MENU_ACTION.to_string(),
            json!({ "from": "https://vpn.example.com/cabinet", "to": "https://vpn.example.com/app" })
        )]
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn выключенная_кнопка_меняется_на_команды_и_запоминается() {
    let Some(env) = env().await else { return };
    env.public_url("https://vpn.example.com").await;
    let admin = env
        .access(ClientAccess {
            menu_button: false,
            ..ClientAccess::default()
        })
        .await;
    let buttons = env.services.bot_buttons();
    env.answer("getChatMenuButton", json!({ "type": "commands" }))
        .await;
    env.answer("getMe", json!({ "id": 123456, "is_bot": true }))
        .await;
    env.answer("setChatMenuButton", json!(true)).await;
    let view = buttons.put_menu(admin).await.unwrap();
    assert_eq!(view.wanted, MenuButton::Commands);
    assert!(view.same && view.applied);
    assert_eq!(view.username, None);
    assert_eq!(
        env.calls("setChatMenuButton").await,
        vec![json!({ "menu_button": { "type": "commands" } })]
    );
    assert_eq!(buttons.stored().await.unwrap().as_deref(), Some("commands"));
    env.db.cleanup().await;
}

#[tokio::test]
async fn отказ_telegram_ничего_не_меняет_и_понятен_оператору() {
    let Some(env) = env().await else { return };
    env.public_url("https://vpn.example.com").await;
    let buttons = env.services.bot_buttons();
    env.answer("getChatMenuButton", json!({ "type": "commands" }))
        .await;
    env.answer("getMe", json!({ "id": 123456, "is_bot": true }))
        .await;
    env.refuse("setChatMenuButton", 400, "Bad Request: BUTTON_URL_INVALID")
        .await;
    let admin = env.owner().await;
    let err = buttons.put_menu(admin).await.unwrap_err();
    assert_eq!(err.code(), TELEGRAM_FAILED);
    assert_eq!(
        err.to_string(),
        "Telegram ответил: Bad Request: BUTTON_URL_INVALID"
    );
    assert_eq!(buttons.stored().await.unwrap(), None);
    assert!(env.audit().await.is_empty());
    env.db.cleanup().await;
}

#[tokio::test]
async fn без_бота_с_плохим_токеном_и_без_связи_причина_понятна() {
    let Some(env) = env_with(None).await else {
        return;
    };
    let err = env.services.bot_buttons().view().await.unwrap_err();
    assert_eq!(err.code(), NO_BOT);
    let owner = env.owner().await;
    let err = env
        .services
        .bot_buttons()
        .put_menu(owner)
        .await
        .unwrap_err();
    assert_eq!(err.code(), NO_BOT);
    assert!(
        env.telegram
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    env.db.cleanup().await;

    let Some(env) = env_with(Some(TOKEN)).await else {
        return;
    };
    env.refuse("getChatMenuButton", 401, "Unauthorized").await;
    env.refuse("getMe", 401, "Unauthorized").await;
    let err = env.services.bot_buttons().view().await.unwrap_err();
    assert_eq!(err.code(), TELEGRAM_FAILED);
    assert_eq!(err.to_string(), "Telegram не принял токен бота");
    env.db.cleanup().await;

    let telegram = MockServer::start().await;
    let Some(env) = env_at(Some(TOKEN), telegram, "http://127.0.0.1:9").await else {
        return;
    };
    let err = env.services.bot_buttons().view().await.unwrap_err();
    assert_eq!(err.code(), TELEGRAM_FAILED);
    assert_eq!(err.to_string(), "нет связи с Telegram");
    assert!(!format!("{err:?}").contains(TOKEN));
    env.db.cleanup().await;
}
