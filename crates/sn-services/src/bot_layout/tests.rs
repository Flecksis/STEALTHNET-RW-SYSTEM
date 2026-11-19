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
        hidden: BTreeMap::from([("menu.btn.buy".to_string(), true)]),
    };
    assert_eq!(
        ops.update(&fixed, admin).await.unwrap_err().code(),
        "bot_layout.invalid"
    );
    db.cleanup().await;
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
