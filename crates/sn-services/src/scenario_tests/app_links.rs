use super::*;
use crate::app_links::{AppLink, EXPIRED, NO_APP, NOT_READY, OpenedApp, TAKEN};
use crate::connect_apps::ConnectAppsUpdate;
use crate::limits::RATE_LIMITED;
use sn_domain::connect::{ConnectApp, ConnectApps, LinkButton, LinkSpot, Platform, Step, Text};
use sn_domain::ids::{RemnaUserId, SubscriptionId};
use sn_domain::subscription::{AutoRenewState, RemnaLink, SubscriptionKind};
use sn_remnawave::fake::{Fault, sample_user};
use sn_store::NewSubscription;

const FIRST: &str = "0123456789abcdef0123456789abcdef";
const SECOND: &str = "fedcba9876543210fedcba9876543210";

fn spot(app: &str, step: usize, button: Option<usize>) -> LinkSpot {
    LinkSpot {
        platform: Platform::Ios,
        app: app.to_string(),
        step,
        button,
    }
}

fn app(name: &str, link: &str, notes: &[&str]) -> ConnectApp {
    let button = |label: &str, url: &str| LinkButton {
        label: Text::new(label, label),
        url: url.to_string(),
    };
    let mut steps = vec![
        Step::Install {
            text: Text::default(),
            buttons: vec![button("App Store", "https://apps.apple.com/app/x")],
        },
        Step::Add {
            text: Text::default(),
            link: Some(link.to_string()),
        },
        Step::Connect {
            text: Text::default(),
        },
    ];
    if !notes.is_empty() {
        steps.push(Step::Note {
            title: Text::new("Правила", "Rules"),
            text: Text::default(),
            buttons: notes.iter().map(|url| button("Открыть", url)).collect(),
        });
    }
    ConnectApp {
        id: String::new(),
        name: name.to_string(),
        icon: None,
        enabled: true,
        steps,
    }
}

async fn save(env: &Env, apps: Vec<ConnectApp>) {
    let admin = owner(&env.services).await;
    let mut map = std::collections::BTreeMap::new();
    map.insert(Platform::Ios, apps);
    env.services
        .connect_apps()
        .update(
            &ConnectAppsUpdate {
                apps: Some(ConnectApps(map)),
            },
            admin,
        )
        .await
        .unwrap();
    env.services.set_remna(Some(env.remna.clone()));
}

async fn empty_subscription(env: &Env, client: ClientId) -> SubscriptionId {
    let mut conn = env.services.store().conn().await.unwrap();
    conn.subscriptions()
        .create(&NewSubscription {
            client_id: client,
            tariff_id: None,
            kind: SubscriptionKind::Paid,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap()
        .id
}

async fn subscription(env: &Env, client: ClientId, remna: i64) -> SubscriptionId {
    let user = sample_user(remna, &format!("sn_{remna}"));
    env.remna.insert_user(user.clone());
    let id = empty_subscription(env, client).await;
    let mut conn = env.services.store().conn().await.unwrap();
    conn.subscriptions()
        .link_remna(
            id,
            &RemnaLink {
                user_id: RemnaUserId(remna),
                username: user.username,
                short_uuid: Some(user.short_uuid),
            },
        )
        .await
        .unwrap();
    id
}

async fn issue(env: &Env, id: SubscriptionId, client: ClientId, spot: &LinkSpot) -> AppLink {
    env.services
        .app_links()
        .issue(id, client, spot)
        .await
        .unwrap()
}

async fn open(env: &Env, link: &AppLink, holder: &str) -> ServiceResult<OpenedApp> {
    env.services.app_links().open(&link.token, holder).await
}

async fn rows(env: &Env) -> Vec<String> {
    let mut conn = env.services.store().conn().await.unwrap();
    sqlx::query_scalar("SELECT l::text FROM app_links l ORDER BY created_at")
        .fetch_all(conn.conn())
        .await
        .unwrap()
}

#[tokio::test]
async fn ссылка_на_приложение_достаётся_первому_браузеру_и_не_хранит_подписку() {
    let Some(env) = env().await else { return };
    let client = telegram_client(&env.services, 5_093_118_901).await;
    let id = subscription(&env, client, 901).await;
    let link = issue(&env, id, client, &spot("happ", 1, None)).await;
    assert_eq!(link.token.len(), 64);
    assert_eq!(link.expires_at, start() + Duration::minutes(10));
    assert!(!env.remna.calls().iter().any(|call| call == "get_user"));

    let stored = rows(&env).await;
    assert_eq!(stored.len(), 1);
    for secret in [link.token.as_str(), "sub.example.com", "short901"] {
        assert!(!stored[0].contains(secret), "{stored:?}");
    }

    let opened = open(&env, &link, FIRST).await.unwrap();
    assert_eq!(opened.name, "Happ");
    assert_eq!(opened.link, "happ://add/https://sub.example.com/short901");
    assert_eq!(
        opened.subscription_url.as_deref(),
        Some("https://sub.example.com/short901")
    );
    assert_eq!(
        opened.install[0].url,
        "https://apps.apple.com/app/happ-proxy-utility/id6504287215"
    );
    assert_eq!(opened.expires_in, 600);
    assert_eq!(open(&env, &link, FIRST).await.unwrap(), opened);
    assert_eq!(code(open(&env, &link, SECOND).await), TAKEN);
    assert_eq!(open(&env, &link, FIRST).await.unwrap(), opened);
    let stored = rows(&env).await;
    for secret in [link.token.as_str(), FIRST, "sub.example.com", "short901"] {
        assert!(!stored[0].contains(secret), "{stored:?}");
    }

    let links = env.services.app_links();
    for (token, holder) in [
        ("f".repeat(64), FIRST.to_string()),
        (link.token.to_uppercase(), FIRST.to_string()),
        (format!("{} ", link.token), FIRST.to_string()),
        (link.token[..63].to_string(), FIRST.to_string()),
        (String::new(), FIRST.to_string()),
        (link.token.clone(), String::new()),
        (link.token.clone(), "Z".repeat(32)),
        (link.token.clone(), FIRST[..31].to_string()),
    ] {
        assert_eq!(code(links.open(&token, &holder).await), EXPIRED, "{token}");
    }
    env.db.cleanup().await;
}

#[tokio::test]
async fn из_двух_браузеров_сразу_ссылка_открывается_в_одном() {
    let Some(env) = env().await else { return };
    let client = telegram_client(&env.services, 5_093_118_902).await;
    let id = subscription(&env, client, 902).await;
    let link = issue(&env, id, client, &spot("happ", 1, None)).await;
    let holders: Vec<String> = (0..8).map(|n| format!("{n:032x}")).collect();
    let tries =
        futures_util::future::join_all(holders.iter().map(|holder| open(&env, &link, holder)))
            .await;
    let opened = tries.iter().filter(|result| result.is_ok()).count();
    assert_eq!(opened, 1, "{tries:?}");
    for result in tries.into_iter().filter(Result::is_err) {
        assert_eq!(code(result), TAKEN);
    }
    env.db.cleanup().await;
}

#[tokio::test]
async fn ссылка_на_приложение_живёт_десять_минут_и_потом_удаляется() {
    let Some(env) = env().await else { return };
    let client = telegram_client(&env.services, 5_093_118_903).await;
    let id = subscription(&env, client, 903).await;
    let opened = issue(&env, id, client, &spot("happ", 1, None)).await;
    let waiting = issue(&env, id, client, &spot("happ", 1, None)).await;
    env.clock.set(start() + Duration::seconds(599));
    assert_eq!(open(&env, &opened, FIRST).await.unwrap().expires_in, 1);
    env.clock.set(start() + Duration::seconds(600));
    assert_eq!(code(open(&env, &opened, FIRST).await), EXPIRED);
    assert_eq!(code(open(&env, &opened, SECOND).await), EXPIRED);
    assert_eq!(code(open(&env, &waiting, FIRST).await), EXPIRED);
    assert_eq!(rows(&env).await.len(), 2);
    let fresh = issue(&env, id, client, &spot("happ", 1, None)).await;
    assert_eq!(rows(&env).await.len(), 1);
    assert!(open(&env, &fresh, FIRST).await.is_ok());
    env.db.cleanup().await;
}

#[tokio::test]
async fn схема_берётся_из_списка_приложений_а_не_из_запроса() {
    let Some(env) = env().await else { return };
    let client = telegram_client(&env.services, 5_093_118_904).await;
    let id = subscription(&env, client, 904).await;
    save(
        &env,
        vec![
            app(
                "Happ",
                "happ://add/{url}?name={name}",
                &["happ://routing/onadd/eyJ", "https://panel.example/help"],
            ),
            app("Web", "https://open.example/add?sub={url}", &[]),
        ],
    )
    .await;
    let links = env.services.app_links();
    for refused in [
        spot("web", 1, None),
        spot("happ", 0, None),
        spot("happ", 0, Some(0)),
        spot("happ", 3, None),
        spot("happ", 3, Some(1)),
        spot("happ", 3, Some(7)),
        spot("javascript:alert(1)", 1, None),
        LinkSpot {
            platform: Platform::Android,
            ..spot("happ", 1, None)
        },
    ] {
        assert_eq!(
            code(links.issue(id, client, &refused).await),
            NO_APP,
            "{refused:?}"
        );
    }
    assert!(rows(&env).await.is_empty());

    let add = issue(&env, id, client, &spot("happ", 1, None)).await;
    let note = issue(&env, id, client, &spot("happ", 3, Some(0))).await;
    assert_eq!(
        open(&env, &add, FIRST).await.unwrap().link,
        "happ://add/https://sub.example.com/short904?name=STEALTHNET"
    );
    let before = env.remna.calls_of("get_user").len();
    let opened = open(&env, &note, FIRST).await.unwrap();
    assert_eq!(opened.link, "happ://routing/onadd/eyJ");
    assert_eq!(opened.subscription_url, None);
    assert_eq!(env.remna.calls_of("get_user").len(), before);

    save(&env, vec![app("Happ", "v2raytun://import/{url}", &[])]).await;
    assert_eq!(
        open(&env, &add, FIRST).await.unwrap().link,
        "v2raytun://import/https://sub.example.com/short904"
    );
    assert_eq!(code(open(&env, &note, FIRST).await), NO_APP);
    let mut off = app("Happ", "happ://add/{url}", &[]);
    off.enabled = false;
    save(&env, vec![off]).await;
    assert_eq!(code(open(&env, &add, FIRST).await), NO_APP);
    assert_eq!(
        code(links.issue(id, client, &spot("happ", 1, None)).await),
        NO_APP
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn чужая_подписка_и_заблокированный_клиент_ссылку_не_получают() {
    let Some(env) = env().await else { return };
    let client = telegram_client(&env.services, 5_093_118_905).await;
    let other = telegram_client(&env.services, 5_093_118_906).await;
    let id = subscription(&env, client, 905).await;
    let links = env.services.app_links();
    let happ = spot("happ", 1, None);
    assert_eq!(code(links.issue(id, other, &happ).await), "not_found");
    assert_eq!(
        code(links.issue(SubscriptionId(id.0 + 999), client, &happ).await),
        "not_found"
    );
    assert!(rows(&env).await.is_empty());

    let moved = issue(&env, id, client, &happ).await;
    let blocked = issue(&env, id, client, &happ).await;
    {
        let mut conn = env.services.store().conn().await.unwrap();
        sqlx::query("UPDATE subscriptions SET client_id = $2 WHERE id = $1")
            .bind(id.0)
            .bind(other.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    assert_eq!(code(open(&env, &moved, FIRST).await), EXPIRED);
    {
        let mut conn = env.services.store().conn().await.unwrap();
        sqlx::query("UPDATE subscriptions SET client_id = $2 WHERE id = $1")
            .bind(id.0)
            .bind(client.0)
            .execute(conn.conn())
            .await
            .unwrap();
        conn.clients()
            .set_status(client, ClientStatus::Blocked, None)
            .await
            .unwrap();
    }
    assert_eq!(code(open(&env, &blocked, FIRST).await), EXPIRED);
    env.db.cleanup().await;
}

#[tokio::test]
async fn ссылка_подписки_берётся_свежей_и_сбой_remnawave_не_сжигает_переход() {
    let Some(env) = env().await else { return };
    let client = telegram_client(&env.services, 5_093_118_907).await;
    let id = subscription(&env, client, 907).await;
    let happ = spot("happ", 1, None);
    let link = issue(&env, id, client, &happ).await;
    env.remna.fail_on("get_user", Fault::Unavailable);
    let failed = open(&env, &link, FIRST).await.unwrap_err();
    assert!(matches!(failed, ServiceError::Remna(_)), "{failed:?}");
    assert_eq!(code(open(&env, &link, SECOND).await), TAKEN);
    assert_eq!(
        open(&env, &link, FIRST).await.unwrap().link,
        "happ://add/https://sub.example.com/short907"
    );

    let fresh = env
        .services
        .subscriptions()
        .reissue_link(id, Actor::Client(client))
        .await
        .unwrap();
    assert_ne!(fresh, "https://sub.example.com/short907");
    assert_eq!(
        open(&env, &link, FIRST).await.unwrap().link,
        format!("happ://add/{fresh}")
    );

    let empty = empty_subscription(&env, client).await;
    let early = issue(&env, empty, client, &happ).await;
    assert_eq!(code(open(&env, &early, FIRST).await), NOT_READY);
    env.db.cleanup().await;
}

#[tokio::test]
async fn ссылок_на_приложение_не_больше_тридцати_за_десять_минут() {
    let Some(env) = env().await else { return };
    let client = telegram_client(&env.services, 5_093_118_908).await;
    let other = telegram_client(&env.services, 5_093_118_909).await;
    let id = subscription(&env, client, 908).await;
    let links = env.services.app_links();
    let happ = spot("happ", 1, None);
    for _ in 0..rules::APP_LINK_CLIENT.limit {
        issue(&env, id, client, &happ).await;
    }
    assert_eq!(code(links.issue(id, client, &happ).await), RATE_LIMITED);
    assert_eq!(code(links.issue(id, other, &happ).await), "not_found");
    env.clock.set(start() + Duration::minutes(20));
    issue(&env, id, client, &happ).await;
    assert_eq!(rows(&env).await.len(), 1);
    env.db.cleanup().await;
}

#[tokio::test]
async fn вместе_с_подпиской_удаляются_и_её_ссылки() {
    let Some(env) = env().await else { return };
    let client = telegram_client(&env.services, 5_093_118_910).await;
    let id = subscription(&env, client, 910).await;
    let link = issue(&env, id, client, &spot("happ", 1, None)).await;
    {
        let mut conn = env.services.store().conn().await.unwrap();
        sqlx::query("DELETE FROM subscriptions WHERE id = $1")
            .bind(id.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    assert_eq!(code(open(&env, &link, FIRST).await), EXPIRED);
    assert!(rows(&env).await.is_empty());
    env.db.cleanup().await;
}
