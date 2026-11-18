use super::*;
use crate::alerts::{Alert, AlertsUpdate, RouteInput, outage_text};
use crate::node_watch::WatchReport;
use crate::remna_sync::RemnaWebhookResult;
use sn_domain::alerts::AlertKind;
use sn_remnawave::Node;
use sn_remnawave::fake::{Fault, sample_node, webhook_body};
use sn_remnawave::webhook::{events, scopes, sign};

const SECRET: &str = "0123456789abcdef0123456789abcdef";

fn at(seconds: i64) -> DateTime<Utc> {
    start() + Duration::seconds(seconds)
}

fn node(id: u128, name: &str) -> Node {
    let mut node = sample_node(Uuid::from_u128(id), name, "DE");
    node.last_status_change = Some(start() - Duration::hours(5));
    node
}

fn germany() -> Node {
    node(0xde, "Germany")
}

fn down(mut node: Node, since: DateTime<Utc>) -> Node {
    node.is_connected = false;
    node.is_connecting = false;
    node.last_status_change = Some(since);
    node.last_status_message = Some("connect ETIMEDOUT".into());
    node
}

fn online(mut node: Node, since: DateTime<Utc>) -> Node {
    node.is_connected = true;
    node.is_connecting = false;
    node.is_disabled = false;
    node.last_status_change = Some(since);
    node.last_status_message = None;
    node
}

fn disabled(mut node: Node) -> Node {
    node.is_disabled = true;
    node.is_connected = false;
    node.is_connecting = false;
    node
}

async fn poll_at(env: &Env, now: DateTime<Utc>, nodes: Vec<Node>) -> WatchReport {
    env.clock.set(now);
    env.remna.set_nodes(nodes);
    env.services.node_watch().poll().await.unwrap()
}

async fn node_events(env: &Env) -> Vec<(String, String, DateTime<Utc>)> {
    let mut conn = env.services.store().conn().await.unwrap();
    let rows: Vec<(String, serde_json::Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT event, payload, occurred_at FROM remna_events WHERE scope = 'node' ORDER BY id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    rows.into_iter()
        .map(|(event, payload, occurred_at)| {
            let name = payload["data"]["name"].as_str().unwrap_or_default();
            (event, name.to_string(), occurred_at)
        })
        .collect()
}

fn kinds(list: &[(String, String, DateTime<Utc>)]) -> Vec<&str> {
    list.iter().map(|(event, _, _)| event.as_str()).collect()
}

async fn hook(env: &Env, event: &str, node: &Node, when: DateTime<Utc>) -> RemnaWebhookResult {
    env.clock.set(env.services.now().max(when));
    env.services.set_remna_webhook_secret(Some(SECRET.into()));
    let body = webhook_body(
        scopes::NODE,
        event,
        when,
        serde_json::to_value(node).unwrap(),
    );
    env.services
        .remna_sync()
        .accept_webhook(Some(&sign(SECRET, &body)), &body)
        .await
        .unwrap()
}

async fn node_alerts(env: &Env, now: DateTime<Utc>) -> Vec<Alert> {
    env.clock.set(now);
    env.services
        .alerts()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .filter(|alert| alert.kind == AlertKind::Nodes)
        .collect()
}

async fn feed_titles(env: &Env) -> Vec<String> {
    env.services
        .admin()
        .feed(
            &FeedQuery {
                kind: FeedKind::Nodes,
                remna_before: None,
                core_before: None,
                limit: Some(20),
            },
            true,
        )
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.title)
        .collect()
}

#[tokio::test]
async fn первый_опрос_сразу_показывает_лежащие_и_выключенные_ноды() {
    let Some(env) = env().await else { return };
    let nodes = || {
        vec![
            germany(),
            down(node(0xf1, "Finland"), start() - Duration::hours(3)),
            disabled(node(0x11, "Netherlands")),
        ]
    };
    let report = poll_at(&env, at(0), nodes()).await;
    assert_eq!(report.nodes, 3);
    assert_eq!(report.events, 2);
    assert_eq!(report.skipped, None);
    let list = node_events(&env).await;
    assert_eq!(
        kinds(&list),
        vec![events::NODE_CONNECTION_LOST, events::NODE_DISABLED]
    );
    assert_eq!(list[0].1, "Finland");
    assert_eq!(list[0].2, at(0));
    assert_eq!(list[1].1, "Netherlands");
    assert_eq!(list[1].2, at(0));
    let alerts = node_alerts(&env, at(5)).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0].html.contains("Нода Finland не отвечает"));

    let again = poll_at(&env, at(60), nodes()).await;
    assert_eq!(again.events, 0);
    assert_eq!(node_events(&env).await.len(), 2);
    assert!(node_alerts(&env, at(600)).await.is_empty());
    assert_eq!(
        feed_titles(&env).await,
        vec!["Нода Netherlands выключена", "Нода Finland не отвечает"]
    );

    let finland = down(node(0xf1, "Finland"), start() - Duration::hours(3));
    assert_eq!(
        hook(&env, events::NODE_CONNECTION_LOST, &finland, at(700)).await,
        RemnaWebhookResult::Duplicate
    );
    assert_eq!(node_events(&env).await.len(), 2);
    env.db.cleanup().await;
}

#[tokio::test]
async fn нода_запомненная_молча_прежней_версией_показывается_один_раз() {
    let Some(env) = env().await else { return };
    let finland = down(node(0xf1, "Finland"), start() - Duration::hours(2));
    {
        let mut conn = env.services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO remna_node_states \
             (node_uuid, name, state, since, event, message, seen_at, down_at, alerted_at) \
             VALUES ($1, 'Finland', 'down', $2, NULL, NULL, $3, NULL, NULL)",
        )
        .bind(finland.uuid)
        .bind(start() - Duration::hours(2))
        .bind(at(-60))
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let first = poll_at(&env, at(0), vec![finland.clone()]).await;
    assert_eq!(first.events, 1);
    let list = node_events(&env).await;
    assert_eq!(kinds(&list), vec![events::NODE_CONNECTION_LOST]);
    assert_eq!(list[0].2, at(0));
    let alerts = node_alerts(&env, at(5)).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");

    let second = poll_at(&env, at(60), vec![finland]).await;
    assert_eq!(second.events, 0);
    assert_eq!(node_events(&env).await.len(), 1);
    assert!(node_alerts(&env, at(120)).await.is_empty());

    poll_at(&env, at(300), vec![online(node(0xf1, "Finland"), at(280))]).await;
    assert_eq!(
        feed_titles(&env).await,
        vec!["Нода Finland снова в сети", "Нода Finland не отвечает"]
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn переходы_ноды_пишутся_в_ленту_по_одному_разу() {
    let Some(env) = env().await else { return };
    poll_at(&env, at(0), vec![germany()]).await;
    let lost = poll_at(&env, at(60), vec![down(germany(), at(40))]).await;
    assert_eq!(lost.events, 1);
    let still = poll_at(&env, at(120), vec![down(germany(), at(40))]).await;
    assert_eq!(still.events, 0);
    poll_at(&env, at(300), vec![online(germany(), at(250))]).await;
    poll_at(&env, at(360), vec![online(germany(), at(250))]).await;

    let list = node_events(&env).await;
    assert_eq!(
        kinds(&list),
        vec![
            events::NODE_CONNECTION_LOST,
            events::NODE_CONNECTION_RESTORED
        ]
    );
    assert_eq!(list[0].2, at(40));
    assert_eq!(list[1].2, at(250));
    assert_eq!(
        feed_titles(&env).await,
        vec!["Нода Germany снова в сети", "Нода Germany не отвечает"]
    );
    let page = env
        .services
        .admin()
        .feed(
            &FeedQuery {
                kind: FeedKind::Nodes,
                remna_before: None,
                core_before: None,
                limit: Some(20),
            },
            true,
        )
        .await
        .unwrap();
    let lost_item = page.items.last().unwrap();
    assert_eq!(lost_item.kind, events::NODE_CONNECTION_LOST);
    assert_eq!(
        lost_item.subject.as_deref(),
        Some(Uuid::from_u128(0xde).to_string().as_str())
    );
    assert_eq!(lost_item.data["lastStatusMessage"], "connect ETIMEDOUT");
    assert_eq!(lost_item.data["isConnected"], false);

    poll_at(&env, at(420), vec![disabled(germany())]).await;
    let mut starting = germany();
    starting.is_connected = false;
    starting.is_connecting = true;
    let enabled = poll_at(&env, at(480), vec![starting.clone()]).await;
    assert_eq!(enabled.events, 1);
    assert_eq!(poll_at(&env, at(510), vec![starting]).await.events, 0);
    poll_at(&env, at(540), vec![online(germany(), at(500))]).await;
    poll_at(&env, at(600), vec![down(germany(), at(590))]).await;
    poll_at(&env, at(660), vec![disabled(germany())]).await;
    let back = poll_at(&env, at(720), vec![online(germany(), at(700))]).await;
    assert_eq!(back.events, 2);
    assert_eq!(
        hook(
            &env,
            events::NODE_ENABLED,
            &online(germany(), at(690)),
            at(690)
        )
        .await,
        RemnaWebhookResult::Duplicate
    );
    let list = node_events(&env).await;
    assert_eq!(
        kinds(&list),
        vec![
            events::NODE_CONNECTION_LOST,
            events::NODE_CONNECTION_RESTORED,
            events::NODE_DISABLED,
            events::NODE_ENABLED,
            events::NODE_CONNECTION_RESTORED,
            events::NODE_CONNECTION_LOST,
            events::NODE_DISABLED,
            events::NODE_ENABLED,
            events::NODE_CONNECTION_RESTORED,
        ]
    );
    assert_eq!(list[4].2, at(500));
    assert_eq!(list[7].2, at(700));
    let titles = feed_titles(&env).await;
    assert_eq!(
        &titles[..3],
        [
            "Нода Germany снова в сети",
            "Нода Germany включена",
            "Нода Germany выключена"
        ]
    );
    assert!(node_alerts(&env, at(900)).await.is_empty());
    env.db.cleanup().await;
}

#[tokio::test]
async fn вебхук_после_опроса_не_даёт_второй_строки() {
    let Some(env) = env().await else { return };
    poll_at(&env, at(0), vec![germany()]).await;
    poll_at(&env, at(60), vec![down(germany(), at(50))]).await;
    assert_eq!(
        hook(
            &env,
            events::NODE_CONNECTION_LOST,
            &down(germany(), at(50)),
            at(50)
        )
        .await,
        RemnaWebhookResult::Duplicate
    );
    assert_eq!(
        kinds(&node_events(&env).await),
        vec![events::NODE_CONNECTION_LOST]
    );

    assert!(matches!(
        hook(
            &env,
            events::NODE_CONNECTION_RESTORED,
            &online(germany(), at(200)),
            at(200)
        )
        .await,
        RemnaWebhookResult::Accepted { .. }
    ));
    let report = poll_at(&env, at(240), vec![online(germany(), at(200))]).await;
    assert_eq!(report.events, 0);
    assert_eq!(
        hook(
            &env,
            events::NODE_CONNECTION_LOST,
            &down(germany(), at(51)),
            at(51)
        )
        .await,
        RemnaWebhookResult::Duplicate
    );
    assert_eq!(
        kinds(&node_events(&env).await),
        vec![
            events::NODE_CONNECTION_LOST,
            events::NODE_CONNECTION_RESTORED
        ]
    );
    assert!(matches!(
        hook(
            &env,
            events::NODE_TRAFFIC_NOTIFY,
            &online(germany(), at(200)),
            at(260)
        )
        .await,
        RemnaWebhookResult::Accepted { .. }
    ));
    assert_eq!(node_events(&env).await.len(), 3);
    env.db.cleanup().await;
}

#[tokio::test]
async fn опрос_после_вебхука_не_пишет_то_же_событие() {
    let Some(env) = env().await else { return };
    assert!(matches!(
        hook(
            &env,
            events::NODE_CONNECTION_LOST,
            &down(germany(), at(10)),
            at(10)
        )
        .await,
        RemnaWebhookResult::Accepted { .. }
    ));
    let first = poll_at(&env, at(60), vec![down(germany(), at(10))]).await;
    assert_eq!(first.events, 0);

    poll_at(&env, at(120), vec![online(germany(), at(100))]).await;
    assert!(matches!(
        hook(
            &env,
            events::NODE_CONNECTION_LOST,
            &down(germany(), at(130)),
            at(130)
        )
        .await,
        RemnaWebhookResult::Accepted { .. }
    ));
    let second = poll_at(&env, at(180), vec![down(germany(), at(130))]).await;
    assert_eq!(second.events, 0);
    assert_eq!(
        hook(
            &env,
            events::NODE_CONNECTION_RESTORED,
            &online(germany(), at(100)),
            at(100)
        )
        .await,
        RemnaWebhookResult::Duplicate
    );
    assert_eq!(
        kinds(&node_events(&env).await),
        vec![
            events::NODE_CONNECTION_LOST,
            events::NODE_CONNECTION_RESTORED,
            events::NODE_CONNECTION_LOST,
        ]
    );
    let mut conn = env.services.store().conn().await.unwrap();
    let unprocessed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM remna_events WHERE processed_at IS NULL")
            .fetch_one(conn.conn())
            .await
            .unwrap();
    assert_eq!(unprocessed, 2);
    drop(conn);
    env.db.cleanup().await;
}

#[tokio::test]
async fn уведомление_о_ноде_уходит_после_двух_минут() {
    let Some(env) = env().await else { return };
    poll_at(&env, at(0), vec![germany()]).await;
    poll_at(&env, at(60), vec![down(germany(), at(60))]).await;
    assert!(node_alerts(&env, at(61)).await.is_empty());
    poll_at(&env, at(120), vec![down(germany(), at(60))]).await;
    assert!(node_alerts(&env, at(150)).await.is_empty());
    poll_at(&env, at(180), vec![down(germany(), at(60))]).await;
    let alerts = node_alerts(&env, at(181)).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert_eq!(
        alerts[0].html,
        "<b>Нода Germany не отвечает</b>\nС 14:53\nПричина: connect ETIMEDOUT"
    );
    assert_eq!(alerts[0].path, "/admin/infra");
    assert!(node_alerts(&env, at(240)).await.is_empty());

    poll_at(&env, at(780), vec![online(germany(), at(750))]).await;
    let alerts = node_alerts(&env, at(781)).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert_eq!(
        alerts[0].html,
        "<b>Нода Germany снова в сети</b>\nЛежала 12 минут"
    );
    assert!(node_alerts(&env, at(900)).await.is_empty());
    env.db.cleanup().await;
}

#[tokio::test]
async fn короткое_мигание_ноды_без_уведомлений() {
    let Some(env) = env().await else { return };
    poll_at(&env, at(0), vec![germany()]).await;
    poll_at(&env, at(60), vec![down(germany(), at(55))]).await;
    assert!(node_alerts(&env, at(90)).await.is_empty());
    poll_at(&env, at(120), vec![online(germany(), at(110))]).await;
    assert!(node_alerts(&env, at(400)).await.is_empty());
    assert!(node_alerts(&env, at(900)).await.is_empty());

    assert!(matches!(
        hook(
            &env,
            events::NODE_CONNECTION_LOST,
            &down(germany(), at(1000)),
            at(1000)
        )
        .await,
        RemnaWebhookResult::Accepted { .. }
    ));
    assert!(node_alerts(&env, at(1060)).await.is_empty());
    assert!(matches!(
        hook(
            &env,
            events::NODE_CONNECTION_RESTORED,
            &online(germany(), at(1100)),
            at(1100)
        )
        .await,
        RemnaWebhookResult::Accepted { .. }
    ));
    assert!(node_alerts(&env, at(1300)).await.is_empty());
    assert_eq!(node_events(&env).await.len(), 4);
    env.db.cleanup().await;
}

#[tokio::test]
async fn вид_ноды_выключается_в_настройках() {
    let Some(env) = env().await else { return };
    let admin = owner(&env.services).await;
    let alerts = env.services.alerts();
    let view = alerts.view().await.unwrap();
    let nodes = view
        .kinds
        .iter()
        .find(|kind| kind.kind == AlertKind::Nodes)
        .unwrap();
    assert!(nodes.on);
    assert_eq!(nodes.title, "Ноды");
    assert_eq!(nodes.default_topic, "Ноды");
    let route = |on: bool| AlertsUpdate {
        routes: vec![RouteInput {
            kind: AlertKind::Nodes,
            on,
            topic: Some("Ноды".into()),
        }],
    };
    alerts.update(&route(false), admin).await.unwrap();
    poll_at(&env, at(0), vec![germany(), node(0xf1, "Finland")]).await;
    poll_at(
        &env,
        at(60),
        vec![down(germany(), at(60)), down(node(0xf1, "Finland"), at(60))],
    )
    .await;
    assert!(node_alerts(&env, at(300)).await.is_empty());
    alerts.update(&route(true), admin).await.unwrap();
    let alerts_now = node_alerts(&env, at(360)).await;
    assert_eq!(alerts_now.len(), 2, "{alerts_now:?}");
    assert!(alerts_now[0].html.contains("Нода Finland не отвечает"));
    assert!(alerts_now[1].html.contains("Нода Germany не отвечает"));

    alerts.update(&route(false), admin).await.unwrap();
    poll_at(
        &env,
        at(420),
        vec![
            online(germany(), at(400)),
            down(node(0xf1, "Finland"), at(60)),
        ],
    )
    .await;
    assert!(node_alerts(&env, at(430)).await.is_empty());
    alerts.update(&route(true), admin).await.unwrap();
    assert!(node_alerts(&env, at(440)).await.is_empty());

    let gone = poll_at(&env, at(480), vec![online(germany(), at(400))]).await;
    assert_eq!(gone.forgotten, 1);
    assert!(node_alerts(&env, at(900)).await.is_empty());
    env.db.cleanup().await;
}

#[tokio::test]
async fn много_нод_сразу_сводятся_в_строку_ещё() {
    let Some(env) = env().await else { return };
    let names = ["A-1", "A-2", "A-3", "A-4", "A-5"];
    let fleet = |state: fn(Node) -> Node| -> Vec<Node> {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| state(node(0x100 + i as u128, name)))
            .collect()
    };
    poll_at(&env, at(0), fleet(|n| n)).await;
    let report = poll_at(&env, at(60), fleet(|n| down(n, at(30)))).await;
    assert_eq!(report.events, 5);
    let alerts = node_alerts(&env, at(200)).await;
    let texts: Vec<&str> = alerts.iter().map(|a| a.html.as_str()).collect();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert_eq!(texts[3], "Ещё 2 ноды не отвечают");
    poll_at(&env, at(300), fleet(|n| online(n, at(290)))).await;
    let alerts = node_alerts(&env, at(301)).await;
    let texts: Vec<&str> = alerts.iter().map(|a| a.html.as_str()).collect();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert!(texts[0].ends_with("Лежала 4 минуты"), "{}", texts[0]);
    assert_eq!(texts[3], "Ещё 2 ноды снова в сети");
    env.db.cleanup().await;
}

#[tokio::test]
async fn без_remnawave_опрос_нод_молча_пропускается() {
    let Some(env) = env().await else { return };
    env.remna.set_nodes(vec![germany()]);
    env.remna.fail_next(Fault::Unavailable);
    let report = env.services.node_watch().poll().await.unwrap();
    assert_eq!(report.skipped, Some("unavailable"));
    env.services.set_remna(None);
    let report = env.services.node_watch().poll().await.unwrap();
    assert_eq!(report.skipped, Some("not_configured"));
    let mut conn = env.services.store().conn().await.unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM remna_node_states")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(rows, 0);
    drop(conn);
    assert!(node_events(&env).await.is_empty());
    env.db.cleanup().await;
}

#[test]
fn сколько_лежала_нода_пишется_по_русски() {
    let text = |seconds: i64| outage_text(Duration::seconds(seconds));
    assert_eq!(text(10), "1 минуту");
    assert_eq!(text(150), "3 минуты");
    assert_eq!(text(11 * 60), "11 минут");
    assert_eq!(text(21 * 60), "21 минуту");
    assert_eq!(text(60 * 60), "1 ч");
    assert_eq!(text(75 * 60), "1 ч 15 мин");
    assert_eq!(text(25 * 3600), "1 день 1 ч");
    assert_eq!(text(48 * 3600), "2 дня");
}
