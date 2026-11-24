use super::*;
use crate::{EnvFallbacks, ServiceDeps, SystemClock};
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_store::SecretBox;
use sn_store::testing::{TestDb, test_store};
use std::sync::Arc;

async fn setup() -> Option<(TestDb, Services, AdminId)> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    let admin = services
        .auth()
        .upsert_owner("owner@example.com", "пароль-владельца-раскладки")
        .await
        .unwrap()
        .0;
    Some((db, services, admin))
}

fn rows(raw: &[&[&str]]) -> Rows {
    raw.iter()
        .map(|row| row.iter().map(|k| k.to_string()).collect())
        .collect()
}

fn screen(view: &LayoutView, key: Screen) -> &ScreenView {
    view.screens.iter().find(|s| s.key == key).unwrap()
}

fn update(screen: &str, layout: Option<Rows>) -> LayoutUpdate {
    LayoutUpdate {
        layouts: BTreeMap::from([(screen.to_string(), layout)]),
        hidden: BTreeMap::new(),
        menu_mode: None,
    }
}

async fn last_audit(services: &Services) -> (String, serde_json::Value) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_as("SELECT action, details FROM audit_log ORDER BY id DESC LIMIT 1")
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

#[tokio::test]
async fn без_своей_раскладки_меню_идёт_стандартно_и_по_колонкам() {
    let Some((db, services, _)) = setup().await else {
        return;
    };
    let view = services.bot_layout().view().await.unwrap();
    let menu = screen(&view, Screen::Menu);
    assert!(!menu.custom);
    assert_eq!(menu.packed, Some(1));
    assert_eq!(menu.rows, Screen::Menu.standard(1));
    assert_eq!(menu.slots.len(), layout::MENU.len());
    let support = menu
        .slots
        .iter()
        .find(|s| s.key == "menu.btn.support")
        .unwrap();
    assert_eq!(support.text, "Поддержка");
    assert_eq!(support.text_en, "Support");
    assert_eq!(
        support.shown,
        vec![ClientState::New, ClientState::Active, ClientState::Ended]
    );
    let buy = menu.slots.iter().find(|s| s.key == "menu.btn.buy").unwrap();
    assert_eq!(buy.shown, vec![ClientState::New]);
    let extend = menu
        .slots
        .iter()
        .find(|s| s.key == "menu.btn.extend")
        .unwrap();
    assert_eq!(extend.shown, vec![ClientState::Active, ClientState::Ended]);
    let app = menu.slots.iter().find(|s| s.key == "menu.btn.app").unwrap();
    assert!(app.off.is_some());
    assert!(app.shown.is_empty());
    assert_eq!(app.kind, Kind::WebApp);
    let sub = screen(&view, Screen::Subscription);
    assert_eq!(sub.packed, None);
    assert_eq!(sub.states, vec![ClientState::Active, ClientState::Ended]);
    assert!(view.texts["menu.title"].contains("{brand}"));
    assert_eq!(
        view.texts_en["menu.no_subscription"],
        "No subscription yet."
    );
    db.cleanup().await;
}

#[tokio::test]
async fn своя_раскладка_сохраняется_и_пишется_в_журнал() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let wanted = rows(&[
        &["menu.btn.support", "menu.btn.buy"],
        &[
            "menu.btn.extend",
            "menu.btn.subscription",
            "menu.btn.balance",
        ],
    ]);
    let view = services
        .bot_layout()
        .update(&update("menu", Some(wanted.clone())), admin)
        .await
        .unwrap();
    let menu = screen(&view, Screen::Menu);
    assert!(menu.custom);
    assert_eq!(menu.packed, None);
    assert_eq!(menu.rows[..2], wanted[..]);
    assert_eq!(menu.rows.concat().len(), layout::MENU.len());
    assert!(menu.rows[2..].iter().all(|row| row.len() == 1));
    let saved = services.bot_content().content.layout(Screen::Menu);
    assert_eq!(saved, menu.rows);
    let (action, details) = last_audit(&services).await;
    assert_eq!(action, "settings.bot_layout");
    assert_eq!(details["screens"], serde_json::json!(["menu"]));
    assert!(crate::audit_journal::labels::label("settings.bot_layout").is_some());
    db.cleanup().await;
}

#[tokio::test]
async fn в_ряду_не_больше_трёх_кнопок_и_чужие_кнопки_не_принимаются() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let crowded = rows(&[&[
        "sub.btn.extend",
        "sub.btn.all",
        "common.btn.menu",
        "sub.btn.devices",
    ]]);
    let err = services
        .bot_layout()
        .update(&update("subscription", Some(crowded)), admin)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "bot_layout.invalid");
    assert!(err.to_string().contains("не больше 3"), "{err}");
    let foreign = rows(&[&["menu.btn.buy"]]);
    let err = services
        .bot_layout()
        .update(&update("subscription", Some(foreign)), admin)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("menu.btn.buy"), "{err}");
    let err = services
        .bot_layout()
        .update(&update("payment", Some(Vec::new())), admin)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "bot_layout.invalid");
    assert!(
        !services
            .bot_content()
            .content
            .has_own_layout(Screen::Subscription)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn вернуть_как_было_убирает_свою_раскладку() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let ops = services.bot_layout();
    ops.update(
        &update(
            "subscription",
            Some(rows(&[&["common.btn.menu", "sub.btn.extend"]])),
        ),
        admin,
    )
    .await
    .unwrap();
    assert!(
        services
            .bot_content()
            .content
            .has_own_layout(Screen::Subscription)
    );
    let view = ops
        .update(&update("subscription", None), admin)
        .await
        .unwrap();
    let sub = screen(&view, Screen::Subscription);
    assert!(!sub.custom);
    assert_eq!(sub.rows, Screen::Subscription.standard(1));
    let (_, details) = last_audit(&services).await;
    assert_eq!(details["reset"], serde_json::json!(["subscription"]));
    db.cleanup().await;
}

#[tokio::test]
async fn кнопку_можно_скрыть_и_вернуть_из_раскладки() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let ops = services.bot_layout();
    let hide = LayoutUpdate {
        layouts: BTreeMap::new(),
        hidden: BTreeMap::from([("menu.btn.support".to_string(), true)]),
        menu_mode: None,
    };
    let view = ops.update(&hide, admin).await.unwrap();
    let support = screen(&view, Screen::Menu)
        .slots
        .iter()
        .find(|s| s.key == "menu.btn.support")
        .unwrap();
    assert!(support.hidden);
    assert_eq!(support.text, "Поддержка");
    assert!(
        services
            .bot_content()
            .content
            .button("menu.btn.support", &[])
            .is_none()
    );
    let show = LayoutUpdate {
        layouts: BTreeMap::new(),
        hidden: BTreeMap::from([("menu.btn.support".to_string(), false)]),
        menu_mode: None,
    };
    ops.update(&show, admin).await.unwrap();
    assert!(
        !services
            .bot_content()
            .content
            .config()
            .buttons
            .contains_key("menu.btn.support")
    );
    let fixed = LayoutUpdate {
        layouts: BTreeMap::new(),
        hidden: BTreeMap::from([("sub.btn.extend".to_string(), true)]),
        menu_mode: None,
    };
    assert_eq!(
        ops.update(&fixed, admin).await.unwrap_err().code(),
        "bot_layout.invalid"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn купить_и_продлить_скрываются_для_режима_только_кабинет() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let ops = services.bot_layout();
    let before = ops.view().await.unwrap();
    for key in ["menu.btn.buy", "menu.btn.extend"] {
        let slot = screen(&before, Screen::Menu)
            .slots
            .iter()
            .find(|s| s.key == key)
            .unwrap();
        assert!(slot.hideable, "{key}");
        assert!(!slot.hidden, "{key}");
    }
    let hide = LayoutUpdate {
        hidden: BTreeMap::from([
            ("menu.btn.buy".to_string(), true),
            ("menu.btn.extend".to_string(), true),
        ]),
        ..LayoutUpdate::default()
    };
    let view = ops.update(&hide, admin).await.unwrap();
    let content = &services.bot_content().content;
    for key in ["menu.btn.buy", "menu.btn.extend"] {
        let slot = screen(&view, Screen::Menu)
            .slots
            .iter()
            .find(|s| s.key == key)
            .unwrap();
        assert!(slot.hidden, "{key}");
        assert!(!slot.text.is_empty(), "{key}");
        assert!(content.button(key, &[]).is_none(), "{key}");
    }
    assert!(content.button("sub.btn.extend", &[]).is_some());
    assert!(content.button("sub.btn.buy", &[]).is_some());
    let (action, details) = last_audit(&services).await;
    assert_eq!(action, "settings.bot_layout");
    assert_eq!(
        details["hidden"],
        serde_json::json!(["menu.btn.buy", "menu.btn.extend"])
    );
    let show = LayoutUpdate {
        hidden: BTreeMap::from([("menu.btn.buy".to_string(), false)]),
        ..LayoutUpdate::default()
    };
    ops.update(&show, admin).await.unwrap();
    let content = &services.bot_content().content;
    assert!(content.button("menu.btn.buy", &[]).is_some());
    assert!(content.button("menu.btn.extend", &[]).is_none());
    db.cleanup().await;
}

async fn public_url(services: &Services, url: &str) {
    let mut conn = services.store().conn().await.unwrap();
    conn.settings()
        .set(
            sn_domain::settings::keys::PUBLIC_URL,
            &serde_json::json!(url),
            None,
        )
        .await
        .unwrap();
    drop(conn);
    services.reload().await.unwrap();
}

fn mode(menu_mode: MenuMode) -> LayoutUpdate {
    LayoutUpdate {
        menu_mode: Some(menu_mode),
        ..LayoutUpdate::default()
    }
}

#[tokio::test]
async fn веб_режим_нужен_работающему_мини_приложению() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let ops = services.bot_layout();
    let view = ops.view().await.unwrap();
    assert_eq!(view.menu_mode, MenuMode::Classic);
    assert!(view.web_off.is_some());
    let refused = ops.update(&mode(MenuMode::Web), admin).await.unwrap_err();
    assert_eq!(refused.code(), "bot_layout.invalid");
    assert!(
        refused.to_string().contains("Веб-режим недоступен"),
        "{refused}"
    );
    public_url(&services, "http://vpn.example.com").await;
    let refused = ops.update(&mode(MenuMode::Web), admin).await.unwrap_err();
    assert!(refused.to_string().contains("https"), "{refused}");
    public_url(&services, "https://vpn.example.com").await;
    let view = ops.view().await.unwrap();
    assert_eq!(view.web_off, None);
    let saved = ops.update(&mode(MenuMode::Web), admin).await.unwrap();
    assert_eq!(saved.menu_mode, MenuMode::Web);
    assert_eq!(
        services.bot_content().content.config().menu_mode,
        MenuMode::Web
    );
    let (action, details) = last_audit(&services).await;
    assert_eq!(action, "settings.bot_layout");
    assert_eq!(details["menu_mode"], "web");
    let menu = screen(&saved, Screen::Menu);
    let web = |key: &str| menu.slots.iter().find(|s| s.key == key).unwrap();
    assert_eq!(web("menu.btn.buy").web, Some("Тарифы"));
    assert_eq!(web("menu.btn.extend").web, Some("Тарифы"));
    assert_eq!(web("menu.btn.subscription").web, Some("Главная"));
    assert_eq!(web("menu.btn.language").web, Some("Язык"));
    assert_eq!(
        web("menu.btn.balance").web,
        Some(if saved.balance {
            "Пополнение"
        } else {
            "Профиль"
        })
    );
    assert_eq!(web("menu.btn.buy").kind, Kind::Callback);
    assert_eq!(web("menu.btn.documents").web, None);
    assert!(web("menu.btn.documents").web_note.is_some());
    assert_eq!(web("menu.btn.support").web, None);
    assert!(
        web("menu.btn.support")
            .web_note
            .as_deref()
            .unwrap()
            .contains("ИИ-чат")
    );
    assert!(
        screen(&saved, Screen::Subscription)
            .slots
            .iter()
            .all(|s| s.web.is_none() && s.web_note.is_none())
    );
    let mut access = services.settings().client_access;
    access.miniapp = false;
    services
        .settings_ops()
        .update_client_access(&access, admin)
        .await
        .unwrap();
    let view = ops.view().await.unwrap();
    assert_eq!(view.menu_mode, MenuMode::Web);
    assert_eq!(view.web_off, Some("Мини-приложение выключено"));
    let again = ops.update(&mode(MenuMode::Web), admin).await.unwrap();
    assert_eq!(again.menu_mode, MenuMode::Web);
    let back = ops.update(&mode(MenuMode::Classic), admin).await.unwrap();
    assert_eq!(back.menu_mode, MenuMode::Classic);
    let (_, details) = last_audit(&services).await;
    assert_eq!(details["menu_mode"], "classic");
    let stored: serde_json::Value = {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings().get(CONTENT_KEY).await.unwrap().unwrap()
    };
    assert!(stored.get("menu_mode").is_none(), "{stored}");
    db.cleanup().await;
}

#[test]
fn в_веб_режиме_кнопки_ведут_в_свои_разделы_мини_приложения() {
    let app = "https://vpn.example.com/app";
    let install = Install {
        miniapp: Some(app.to_string()),
        web: true,
        balance: true,
        support_app: true,
        ..Install::default()
    };
    let url = |key: &str| install.app_url(key);
    assert_eq!(url("menu.btn.app").as_deref(), Some(app));
    assert_eq!(url("menu.btn.subscription").as_deref(), Some(app));
    for (key, path) in [
        ("menu.btn.trial", "/trial"),
        ("menu.btn.buy", "/plans"),
        ("menu.btn.extend", "/plans"),
        ("menu.btn.balance", "/topup"),
        ("menu.btn.promo", "/promo"),
        ("menu.btn.options", "/addons"),
        ("menu.btn.gifts", "/gifts"),
        ("menu.btn.referral", "/profile"),
        ("menu.btn.roulette", "/roulette"),
        ("menu.btn.support", "/support"),
        ("menu.btn.email", "/profile/email"),
        ("menu.btn.language", "/profile/language"),
    ] {
        assert_eq!(url(key), Some(format!("{app}{path}")), "{key}");
    }
    for key in [
        "menu.btn.contests",
        "menu.btn.documents",
        "sub.btn.devices",
        "sub.btn.extend",
        "common.btn.menu",
    ] {
        assert_eq!(url(key), None, "{key}");
    }
    let no_topup = Install {
        balance: false,
        ..install.clone()
    };
    assert_eq!(
        no_topup.app_url("menu.btn.balance"),
        Some(format!("{app}/profile"))
    );
    let friends_screen = Install {
        friends_screen: true,
        ..install.clone()
    };
    assert_eq!(
        friends_screen.app_url("menu.btn.referral"),
        Some(format!("{app}/friends"))
    );
    let bot_tickets = Install {
        support_app: false,
        ..install.clone()
    };
    assert_eq!(bot_tickets.app_url("menu.btn.support"), None);
    let own_link = Install {
        support: Some("https://t.me/help".into()),
        ..install.clone()
    };
    assert_eq!(own_link.app_url("menu.btn.support"), None);
    assert_eq!(own_link.kind("menu.btn.support"), Kind::Link);
    let classic = Install {
        web: false,
        ..install.clone()
    };
    assert_eq!(classic.app_url("menu.btn.app").as_deref(), Some(app));
    assert_eq!(
        classic.app_url("menu.btn.roulette"),
        Some(format!("{app}/roulette"))
    );
    assert_eq!(classic.app_url("menu.btn.buy"), None);
    let without_app = Install {
        miniapp: None,
        ..install
    };
    for key in ["menu.btn.app", "menu.btn.buy", "menu.btn.support"] {
        assert_eq!(without_app.app_url(key), None, "{key}");
    }
}

#[tokio::test]
async fn тексты_и_кнопки_не_стирают_раскладку() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let wanted = rows(&[&["menu.btn.language", "menu.btn.support"]]);
    services
        .bot_layout()
        .update(&update("menu", Some(wanted.clone())), admin)
        .await
        .unwrap();
    let texts = crate::bot_content::ContentUpdate {
        texts: BTreeMap::from([(
            "menu.no_subscription".to_string(),
            Some("Пока пусто.".to_string()),
        )]),
        ..Default::default()
    };
    services
        .bot_content_ops()
        .update_content(&texts, admin)
        .await
        .unwrap();
    assert_eq!(
        services.bot_content().content.layout(Screen::Menu)[0],
        wanted[0]
    );
    db.cleanup().await;
}
