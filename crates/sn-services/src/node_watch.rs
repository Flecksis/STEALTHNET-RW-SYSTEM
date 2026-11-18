use crate::error::ServiceResult;
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_remnawave::webhook::{ParsedWebhook, RemnaEvent, body_digest, events, scopes};
use sn_remnawave::{Node, NodeState};
use sn_store::{Db, NewRemnaEvent, Tx};
use uuid::Uuid;

pub const DOWN_ALERT_AFTER_SECONDS: i64 = 120;

const COLUMNS: &str = "node_uuid, name, state, since, event, message, seen_at, down_at, alerted_at";
const INSERT: &str = "INSERT INTO remna_node_states \
     (node_uuid, name, state, since, event, message, seen_at, down_at, alerted_at) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT (node_uuid) DO NOTHING";
const UPDATE: &str = "UPDATE remna_node_states SET name = $2, state = $3, since = $4, event = $5, \
     message = $6, seen_at = $7, down_at = $8, alerted_at = $9 WHERE node_uuid = $1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Online,
    Down,
    Disabled,
}

impl Health {
    pub const fn as_str(self) -> &'static str {
        match self {
            Health::Online => "online",
            Health::Down => "down",
            Health::Disabled => "disabled",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "down" => Health::Down,
            "disabled" => Health::Disabled,
            _ => Health::Online,
        }
    }
}

pub fn observed(node: &Node, previous: Option<Health>) -> Option<Health> {
    match node.state() {
        NodeState::Online => Some(Health::Online),
        NodeState::Down => Some(Health::Down),
        NodeState::Disabled => Some(Health::Disabled),
        NodeState::Connecting => match previous {
            Some(Health::Disabled) => Some(Health::Down),
            other => other,
        },
    }
}

pub fn transition(from: Health, to: Health) -> &'static [(&'static str, Health)] {
    match (from, to) {
        _ if from == to => &[],
        (_, Health::Disabled) => &[(events::NODE_DISABLED, Health::Disabled)],
        (Health::Disabled, Health::Down) => &[(events::NODE_ENABLED, Health::Down)],
        (Health::Disabled, Health::Online) => &[
            (events::NODE_ENABLED, Health::Down),
            (events::NODE_CONNECTION_RESTORED, Health::Online),
        ],
        (_, Health::Down) => &[(events::NODE_CONNECTION_LOST, Health::Down)],
        (_, Health::Online) => &[(events::NODE_CONNECTION_RESTORED, Health::Online)],
    }
}

pub fn target(event: &str) -> Option<Health> {
    match event {
        events::NODE_CONNECTION_LOST | events::NODE_ENABLED => Some(Health::Down),
        events::NODE_CONNECTION_RESTORED => Some(Health::Online),
        events::NODE_DISABLED => Some(Health::Disabled),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeNotice {
    Down {
        name: String,
        since: DateTime<Utc>,
        message: Option<String>,
    },
    Back {
        name: String,
        down_at: DateTime<Utc>,
        back_at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WatchReport {
    pub nodes: usize,
    pub events: usize,
    pub forgotten: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<&'static str>,
}

impl WatchReport {
    fn skipped(reason: &'static str) -> Self {
        WatchReport {
            skipped: Some(reason),
            ..WatchReport::default()
        }
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    node_uuid: Uuid,
    name: String,
    state: String,
    since: DateTime<Utc>,
    event: Option<String>,
    message: Option<String>,
    seen_at: DateTime<Utc>,
    down_at: Option<DateTime<Utc>>,
    alerted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
struct Track {
    uuid: Uuid,
    name: String,
    state: Health,
    since: DateTime<Utc>,
    event: Option<String>,
    message: Option<String>,
    seen_at: DateTime<Utc>,
    down_at: Option<DateTime<Utc>>,
    alerted_at: Option<DateTime<Utc>>,
}

impl From<Row> for Track {
    fn from(row: Row) -> Self {
        Track {
            uuid: row.node_uuid,
            name: row.name,
            state: Health::parse(&row.state),
            since: row.since,
            event: row.event,
            message: row.message,
            seen_at: row.seen_at,
            down_at: row.down_at,
            alerted_at: row.alerted_at,
        }
    }
}

impl Track {
    fn enter(&mut self, to: Health, at: DateTime<Utc>, event: &str) {
        self.state = to;
        self.since = at;
        self.event = Some(event.to_string());
        if to == Health::Down {
            self.down_at.get_or_insert(at);
        } else if self.alerted_at.is_none() {
            self.down_at = None;
        }
    }
}

fn status_message(node: &Node) -> Option<String> {
    node.last_status_message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
}

async fn lock(tx: &mut Tx, uuid: Uuid) -> ServiceResult<Option<Track>> {
    let row: Option<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM remna_node_states WHERE node_uuid = $1 FOR UPDATE"
    ))
    .bind(uuid)
    .fetch_optional(tx.conn())
    .await?;
    Ok(row.map(Track::from))
}

async fn write(tx: &mut Tx, sql: &str, track: &Track) -> ServiceResult<bool> {
    let done = sqlx::query(sql)
        .bind(track.uuid)
        .bind(&track.name)
        .bind(track.state.as_str())
        .bind(track.since)
        .bind(&track.event)
        .bind(&track.message)
        .bind(track.seen_at)
        .bind(track.down_at)
        .bind(track.alerted_at)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() == 1)
}

async fn record(
    tx: &mut Tx,
    uuid: Uuid,
    event: &str,
    at: DateTime<Utc>,
    data: serde_json::Value,
) -> ServiceResult<()> {
    let payload = serde_json::json!({
        "scope": scopes::NODE,
        "event": event,
        "timestamp": at,
        "data": data,
    });
    let digest = body_digest(format!("node_watch {uuid} {payload}").as_bytes());
    let inserted = tx
        .remna_events()
        .insert(&NewRemnaEvent {
            body_sha256: digest.to_vec(),
            scope: scopes::NODE.to_string(),
            event: event.to_string(),
            occurred_at: at,
            subject_kind: Some("node".to_string()),
            subject_id: Some(uuid.to_string()),
            payload,
        })
        .await?;
    if let Some(id) = inserted {
        tx.remna_events().mark_processed(id).await?;
    }
    Ok(())
}

async fn announce(
    tx: &mut Tx,
    track: &mut Track,
    node: &Node,
    now: DateTime<Utc>,
) -> ServiceResult<usize> {
    let event = match track.state {
        Health::Online => return Ok(0),
        Health::Down => events::NODE_CONNECTION_LOST,
        Health::Disabled => events::NODE_DISABLED,
    };
    track.event = Some(event.to_string());
    if track.state == Health::Down {
        track.down_at.get_or_insert(track.since);
    }
    let data = serde_json::to_value(node).unwrap_or_default();
    record(tx, track.uuid, event, now, data).await?;
    tracing::info!(node = %node.name, event, "нода недоступна с первого опроса");
    Ok(1)
}

pub(crate) async fn accept_webhook(
    tx: &mut Tx,
    parsed: &ParsedWebhook,
    now: DateTime<Utc>,
) -> ServiceResult<bool> {
    if parsed.scope != scopes::NODE {
        return Ok(true);
    }
    let Some(target) = target(&parsed.event) else {
        return Ok(true);
    };
    let Some(uuid) = parsed
        .subject()
        .filter(|(kind, _)| *kind == "node")
        .and_then(|(_, id)| Uuid::parse_str(&id).ok())
    else {
        return Ok(true);
    };
    let node = match &parsed.payload {
        RemnaEvent::Node(node) => Some(node.as_ref()),
        _ => None,
    };
    let at = parsed.occurred_at.min(now);
    for _ in 0..2 {
        let Some(mut track) = lock(tx, uuid).await? else {
            let fresh = Track {
                uuid,
                name: node.map_or_else(|| uuid.to_string(), |n| n.name.clone()),
                state: target,
                since: at,
                event: Some(parsed.event.clone()),
                message: node.and_then(status_message),
                seen_at: at,
                down_at: (target == Health::Down).then_some(at),
                alerted_at: None,
            };
            if write(tx, INSERT, &fresh).await? {
                return Ok(true);
            }
            continue;
        };
        if let Some(node) = node {
            track.name = node.name.clone();
            track.message = status_message(node);
        }
        let enabled = parsed.event == events::NODE_ENABLED;
        if track.state == target {
            if track.event.is_some() {
                return Ok(false);
            }
            track.event = Some(parsed.event.clone());
            if target == Health::Down {
                track.down_at.get_or_insert(at);
            }
        } else if at < track.since || (enabled && track.state != Health::Disabled) {
            return Ok(false);
        } else {
            track.enter(target, at, &parsed.event);
            track.seen_at = at;
        }
        write(tx, UPDATE, &track).await?;
        return Ok(true);
    }
    Ok(true)
}

pub struct NodeWatch<'a> {
    services: &'a Services,
}

impl<'a> NodeWatch<'a> {
    pub fn new(services: &'a Services) -> Self {
        NodeWatch { services }
    }

    pub async fn poll(&self) -> ServiceResult<WatchReport> {
        let Ok(remna) = self.services.remna() else {
            return Ok(WatchReport::skipped("not_configured"));
        };
        let nodes = match remna.nodes().await {
            Ok(nodes) => nodes,
            Err(err) => {
                tracing::debug!(error = %err, "ноды Remnawave не опрошены");
                return Ok(WatchReport::skipped("unavailable"));
            }
        };
        let now = self.services.now();
        let mut report = WatchReport {
            nodes: nodes.len(),
            ..WatchReport::default()
        };
        for node in &nodes {
            report.events += self.observe(node, now).await?;
        }
        let present: Vec<Uuid> = nodes.iter().map(|node| node.uuid).collect();
        let mut conn = self.services.store().conn().await?;
        report.forgotten =
            sqlx::query("DELETE FROM remna_node_states WHERE NOT (node_uuid = ANY($1))")
                .bind(&present)
                .execute(conn.conn())
                .await?
                .rows_affected();
        Ok(report)
    }

    async fn observe(&self, node: &Node, now: DateTime<Utc>) -> ServiceResult<usize> {
        let mut tx = self.services.store().begin().await?;
        let current = lock(&mut tx, node.uuid).await?;
        let Some(health) = observed(node, current.as_ref().map(|track| track.state)) else {
            tx.rollback().await?;
            return Ok(0);
        };
        let Some(mut track) = current else {
            let mut first = Track {
                uuid: node.uuid,
                name: node.name.clone(),
                state: health,
                since: node
                    .last_status_change
                    .filter(|at| *at <= now)
                    .unwrap_or(now),
                event: None,
                message: status_message(node),
                seen_at: now,
                down_at: None,
                alerted_at: None,
            };
            let announced = announce(&mut tx, &mut first, node, now).await?;
            if !write(&mut tx, INSERT, &first).await? {
                tx.rollback().await?;
                return Ok(0);
            }
            tx.commit().await?;
            return Ok(announced);
        };
        let steps = transition(track.state, health);
        let mut written = steps.len();
        if steps.is_empty() {
            if node.state() != NodeState::Connecting {
                track.seen_at = now;
            }
            if track.event.is_none() {
                written += announce(&mut tx, &mut track, node, now).await?;
            }
        } else {
            let at = node
                .last_status_change
                .filter(|at| *at > track.seen_at && *at <= now)
                .unwrap_or(now);
            let data = serde_json::to_value(node).unwrap_or_default();
            for (event, state) in steps {
                track.enter(*state, at, event);
                record(&mut tx, node.uuid, event, at, data.clone()).await?;
                tracing::info!(node = %node.name, event, "состояние ноды изменилось");
            }
            track.seen_at = now;
        }
        track.name = node.name.clone();
        track.message = status_message(node);
        write(&mut tx, UPDATE, &track).await?;
        tx.commit().await?;
        Ok(written)
    }

    pub async fn notices(&self, send: bool) -> ServiceResult<Vec<NodeNotice>> {
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM remna_node_states WHERE down_at IS NOT NULL \
             ORDER BY down_at, name FOR UPDATE SKIP LOCKED"
        ))
        .fetch_all(tx.conn())
        .await?;
        let mut out = Vec::new();
        for mut track in rows.into_iter().map(Track::from) {
            let Some(down_at) = track.down_at else {
                continue;
            };
            match (track.state, track.alerted_at) {
                (Health::Down, None) => {
                    if !send || now - down_at < Duration::seconds(DOWN_ALERT_AFTER_SECONDS) {
                        continue;
                    }
                    out.push(NodeNotice::Down {
                        name: track.name.clone(),
                        since: down_at,
                        message: track.message.clone(),
                    });
                    track.alerted_at = Some(now);
                }
                (Health::Online, Some(_)) => {
                    if send {
                        out.push(NodeNotice::Back {
                            name: track.name.clone(),
                            down_at,
                            back_at: track.since,
                        });
                    }
                    track.down_at = None;
                    track.alerted_at = None;
                }
                (Health::Down | Health::Disabled, Some(_)) => continue,
                (Health::Online | Health::Disabled, None) => track.down_at = None,
            }
            write(&mut tx, UPDATE, &track).await?;
        }
        tx.commit().await?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_remnawave::fake::sample_node;

    fn node(connected: bool, connecting: bool, disabled: bool) -> Node {
        let mut node = sample_node(Uuid::from_u128(1), "Germany", "DE");
        node.is_connected = connected;
        node.is_connecting = connecting;
        node.is_disabled = disabled;
        node
    }

    #[test]
    fn переходы_дают_события_как_у_вебхука() {
        use Health::*;
        let events_of = |from, to| -> Vec<&str> {
            transition(from, to)
                .iter()
                .map(|(event, _)| *event)
                .collect()
        };
        assert!(events_of(Online, Online).is_empty());
        assert_eq!(events_of(Online, Down), [events::NODE_CONNECTION_LOST]);
        assert_eq!(events_of(Down, Online), [events::NODE_CONNECTION_RESTORED]);
        assert_eq!(events_of(Online, Disabled), [events::NODE_DISABLED]);
        assert_eq!(events_of(Down, Disabled), [events::NODE_DISABLED]);
        assert_eq!(events_of(Disabled, Down), [events::NODE_ENABLED]);
        assert_eq!(
            events_of(Disabled, Online),
            [events::NODE_ENABLED, events::NODE_CONNECTION_RESTORED]
        );
        for from in [Online, Down, Disabled] {
            for to in [Online, Down, Disabled] {
                let steps = transition(from, to);
                for (event, state) in steps {
                    assert_eq!(target(event), Some(*state));
                }
                assert_eq!(steps.last().map_or(from, |(_, state)| *state), to);
            }
        }
        assert_eq!(target(events::NODE_TRAFFIC_NOTIFY), None);
    }

    #[test]
    fn подключение_после_выключения_ещё_не_в_сети() {
        let connecting = node(false, true, false);
        assert_eq!(observed(&connecting, None), None);
        assert_eq!(
            observed(&connecting, Some(Health::Online)),
            Some(Health::Online)
        );
        assert_eq!(
            observed(&connecting, Some(Health::Down)),
            Some(Health::Down)
        );
        assert_eq!(
            observed(&connecting, Some(Health::Disabled)),
            Some(Health::Down)
        );
        assert_eq!(
            observed(&node(true, false, true), None),
            Some(Health::Disabled)
        );
        assert_eq!(
            observed(&node(false, false, false), Some(Health::Online)),
            Some(Health::Down)
        );
    }
}
