use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::remna_sync::same_user;
use crate::remna_transfer::{TariffMatch, match_tariff, snapshot, tariffs};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sn_domain::ids::{ClientId, RemnaUserId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::subscription::{
    RemnaLink, Subscription, SubscriptionKind, SubscriptionStatus, TRIAL_TAG,
};
use sn_domain::traffic::TrafficStrategy;
use sn_remnawave::{RemnaError, RemnaUser, RemnawaveApi, UsersStreamQuery};
use sn_store::{ActorKind, AuditEntry, ClientPerson, Db, LockSpace, Tx};
use std::collections::HashMap;
use uuid::Uuid;

#[cfg(test)]
mod tests;

pub const REMNA_LINKED: &str = "subscription.remna_linked";
pub const REMNA_ADDED: &str = "subscription.remna_added";
pub const TAKEN: &str = "remna_link.taken";
pub const WORKING: &str = "remna_link.working";
pub const ENDED: &str = "remna_link.ended";
pub const BLOCKED: &str = "remna_link.blocked";
pub const USER_GONE: &str = "remna_link.user_gone";
pub const USER_CHANGED: &str = "remna_link.user_changed";
pub const CHANGED: &str = "remna_link.changed";
pub const QUERY: &str = "remna_link.query";
pub const SINGLE: &str = "remna_link.single";

const FOUND_LIMIT: u32 = 20;
const QUERY_MAX: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    Unlinked,
    Missing,
    Linked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FoundBy {
    Username,
    ShortLink,
    Telegram,
    Email,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Terms {
    pub status: Option<SubscriptionStatus>,
    pub expire_at: Option<DateTime<Utc>>,
    pub traffic_limit_bytes: Option<u64>,
    pub traffic_used_bytes: Option<u64>,
    pub traffic_strategy: Option<TrafficStrategy>,
    pub hwid_limit: Option<u32>,
    pub squads: Vec<String>,
    pub synced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Holder {
    pub subscription_id: SubscriptionId,
    pub client: ClientPerson,
    pub same_client: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TariffPick {
    One {
        id: i64,
        name: String,
        extra_devices: u32,
    },
    Many {
        names: Vec<String>,
    },
    None,
}

impl From<TariffMatch> for TariffPick {
    fn from(value: TariffMatch) -> Self {
        match value {
            TariffMatch::One { id, name, extra } => TariffPick::One {
                id,
                name,
                extra_devices: extra,
            },
            TariffMatch::Many(names) => TariffPick::Many { names },
            TariffMatch::None => TariffPick::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub id: RemnaUserId,
    pub username: String,
    pub short_uuid: String,
    pub telegram_id: Option<i64>,
    pub email: Option<String>,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub found_by: Vec<FoundBy>,
    pub terms: Terms,
    pub holder: Option<Holder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tariff: Option<TariffPick>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkSearch {
    pub state: LinkState,
    pub current: Option<RemnaLink>,
    pub missing_since: Option<DateTime<Utc>>,
    pub panel: Terms,
    pub users: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddSearch {
    pub users: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LinkRequest {
    pub user_id: RemnaUserId,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Linked {
    pub user_id: RemnaUserId,
    pub username: String,
    pub refreshed: bool,
    pub released: Vec<SubscriptionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Added {
    pub subscription_id: SubscriptionId,
    pub user_id: RemnaUserId,
    pub username: String,
    pub tariff: Option<String>,
    pub refreshed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Lookup {
    Username(String),
    Short(String),
    Telegram(i64),
    Email(String),
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct HolderRow {
    id: i64,
    client_id: i64,
    remna_user_id: Option<i64>,
    remna_username: Option<String>,
    short_uuid: Option<String>,
    ended_at: Option<DateTime<Utc>>,
}

impl HolderRow {
    fn holds(&self, user: &RemnaUser) -> bool {
        self.remna_username.as_deref() == Some(user.username.as_str())
            || (!user.short_uuid.is_empty()
                && self.short_uuid.as_deref() == Some(user.short_uuid.as_str()))
    }

    fn noted(&self) -> serde_json::Value {
        json!({
            "subscription_id": self.id,
            "remna_user_id": self.remna_user_id,
            "remna_username": self.remna_username,
            "short_uuid": self.short_uuid,
            "ended": self.ended_at.is_some(),
        })
    }
}

fn nobody(client_id: ClientId) -> ClientPerson {
    ClientPerson {
        client_id,
        name: None,
        telegram_username: None,
        telegram_id: None,
        email: None,
    }
}

fn short_of(user: &RemnaUser) -> Option<String> {
    Some(user.short_uuid.clone()).filter(|s| !s.is_empty())
}

fn query_lookups(query: &str) -> ServiceResult<Vec<Lookup>> {
    let wanted = lookups(&query.chars().take(QUERY_MAX).collect::<String>());
    if wanted.is_empty() {
        return Err(ServiceError::rejected(
            QUERY,
            "Введите имя пользователя, ссылку подписки, Telegram ID или почту",
        ));
    }
    Ok(wanted)
}

fn candidate(
    user: RemnaUser,
    found_by: Vec<FoundBy>,
    holder: Option<Holder>,
    tariff: Option<TariffPick>,
) -> Candidate {
    Candidate {
        holder,
        tariff,
        terms: user_terms(&user),
        id: user.id,
        username: user.username,
        short_uuid: user.short_uuid,
        telegram_id: user.telegram_id,
        email: user.email,
        description: user.description,
        created_at: user.created_at,
        found_by,
    }
}

fn taken_race(user: &RemnaUser) -> ServiceError {
    ServiceError::rejected(
        TAKEN,
        format!(
            "Пользователя Remnawave {} только что привязали к другой подписке. Найдите пользователя заново",
            user.username
        ),
    )
}

async fn release(
    tx: &mut Tx,
    exclude: Option<SubscriptionId>,
    client_id: ClientId,
    user: &RemnaUser,
) -> ServiceResult<Result<Vec<HolderRow>, Holder>> {
    let short = short_of(user);
    let rows: Vec<HolderRow> = sqlx::query_as(
        "SELECT id, client_id, remna_user_id, remna_username, short_uuid, ended_at FROM subscriptions \
         WHERE ($1::bigint IS NULL OR id <> $1) \
         AND (remna_user_id = $2 OR remna_username = $3 OR ($4::text IS NOT NULL AND short_uuid = $4)) \
         ORDER BY id FOR UPDATE",
    )
    .bind(exclude.map(|id| id.0))
    .bind(user.id.0)
    .bind(&user.username)
    .bind(&short)
    .fetch_all(tx.conn())
    .await?;
    if let Some(row) = rows
        .iter()
        .find(|row| row.ended_at.is_none() && row.holds(user))
    {
        let holder_client = ClientId(row.client_id);
        let people = tx.clients().people(&[holder_client]).await?;
        return Ok(Err(Holder {
            subscription_id: SubscriptionId(row.id),
            client: people
                .get(&holder_client)
                .cloned()
                .unwrap_or_else(|| nobody(holder_client)),
            same_client: holder_client == client_id,
        }));
    }
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    if !ids.is_empty() {
        sqlx::query(
            "UPDATE subscriptions SET \
             remna_user_id = CASE WHEN remna_user_id = $2 THEN NULL ELSE remna_user_id END, \
             remna_username = CASE WHEN remna_username = $3 THEN NULL ELSE remna_username END, \
             short_uuid = CASE WHEN $4::text IS NOT NULL AND short_uuid = $4 THEN NULL ELSE short_uuid END, \
             remna_missing_at = CASE WHEN ended_at IS NULL THEN COALESCE(remna_missing_at, now()) ELSE remna_missing_at END \
             WHERE id = ANY($1)",
        )
        .bind(&ids)
        .bind(user.id.0)
        .bind(&user.username)
        .bind(&short)
        .execute(tx.conn())
        .await?;
    }
    Ok(Ok(rows))
}

fn token_like(value: &str) -> bool {
    (3..=64).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn shorts_in_link(raw: &str) -> Vec<String> {
    let rest = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    let path = rest.split(['?', '#']).next().unwrap_or_default();
    let parts: Vec<&str> = path
        .split('/')
        .skip(1)
        .filter(|part| token_like(part))
        .collect();
    parts
        .into_iter()
        .rev()
        .take(3)
        .map(str::to_string)
        .collect()
}

fn lookups(query: &str) -> Vec<Lookup> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    if query.contains('/') {
        return shorts_in_link(query)
            .into_iter()
            .map(Lookup::Short)
            .collect();
    }
    if let Some(name) = query.strip_prefix('@') {
        return if token_like(name) {
            vec![Lookup::Username(name.to_string())]
        } else {
            Vec::new()
        };
    }
    if query.contains('@') {
        let mut out = vec![Lookup::Email(query.to_string())];
        let lower = query.to_lowercase();
        if lower != query {
            out.push(Lookup::Email(lower));
        }
        return out;
    }
    let mut out = Vec::new();
    if query.chars().all(|c| c.is_ascii_digit())
        && let Ok(id) = query.parse::<i64>()
    {
        out.push(Lookup::Telegram(id));
    }
    if token_like(query) {
        out.push(Lookup::Username(query.to_string()));
        out.push(Lookup::Short(query.to_string()));
    }
    out
}

fn nothing(err: &RemnaError) -> bool {
    matches!(
        err,
        RemnaError::NotFound { .. } | RemnaError::BadRequest { .. }
    )
}

fn one(found: Result<RemnaUser, RemnaError>) -> ServiceResult<Option<RemnaUser>> {
    match found {
        Ok(user) => Ok(Some(user)),
        Err(err) if nothing(&err) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn stream(
    remna: &dyn RemnawaveApi,
    query: UsersStreamQuery,
) -> ServiceResult<Vec<RemnaUser>> {
    match remna.stream_users(&query).await {
        Ok(page) => Ok(page.users),
        Err(err) if nothing(&err) => Ok(Vec::new()),
        Err(err) => Err(err.into()),
    }
}

async fn find(
    remna: &dyn RemnawaveApi,
    lookups: &[Lookup],
) -> ServiceResult<Vec<(RemnaUser, Vec<FoundBy>)>> {
    let mut found: Vec<(RemnaUser, Vec<FoundBy>)> = Vec::new();
    let mut add =
        |user: RemnaUser, by: FoundBy| match found.iter_mut().find(|(u, _)| u.id == user.id) {
            Some((_, ways)) => {
                if !ways.contains(&by) {
                    ways.push(by);
                    ways.sort();
                }
            }
            None => found.push((user, vec![by])),
        };
    for lookup in lookups {
        match lookup {
            Lookup::Username(name) => {
                if let Some(user) = one(remna.get_user_by_username(name).await)? {
                    add(user, FoundBy::Username);
                }
            }
            Lookup::Short(short) => {
                if let Some(user) = one(remna.get_user_by_short_uuid(short).await)? {
                    add(user, FoundBy::ShortLink);
                }
            }
            Lookup::Telegram(id) => {
                let query = UsersStreamQuery {
                    size: Some(FOUND_LIMIT),
                    telegram_id: Some(id.to_string()),
                    ..UsersStreamQuery::default()
                };
                for user in stream(remna, query).await? {
                    add(user, FoundBy::Telegram);
                }
            }
            Lookup::Email(email) => {
                let query = UsersStreamQuery {
                    size: Some(FOUND_LIMIT),
                    email: Some(email.clone()),
                    ..UsersStreamQuery::default()
                };
                for user in stream(remna, query).await? {
                    add(user, FoundBy::Email);
                }
            }
        }
    }
    found.truncate(FOUND_LIMIT as usize);
    Ok(found)
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(0)
}

fn user_terms(user: &RemnaUser) -> Terms {
    Terms {
        status: Some(user.status.into()),
        expire_at: Some(user.expire_at),
        traffic_limit_bytes: Some(to_u64(user.traffic_limit_bytes)),
        traffic_used_bytes: Some(to_u64(user.user_traffic.used_traffic_bytes)),
        traffic_strategy: Some(user.traffic_limit_strategy),
        hwid_limit: user
            .hwid_device_limit
            .and_then(|limit| u32::try_from(limit).ok()),
        squads: user
            .active_internal_squads
            .iter()
            .map(|squad| squad.name.clone())
            .collect(),
        synced: true,
    }
}

fn person_text(person: &ClientPerson) -> String {
    if let Some(username) = person
        .telegram_username
        .as_deref()
        .filter(|u| !u.is_empty())
    {
        return format!("@{username}");
    }
    if let Some(name) = person.name.as_deref().filter(|n| !n.is_empty()) {
        return format!("«{name}»");
    }
    format!("№{}", person.client_id.0)
}

fn taken_text(user: &RemnaUser, holder: &Holder) -> String {
    if holder.same_client {
        format!(
            "Пользователь Remnawave {} уже привязан к другой подписке этого клиента (№{})",
            user.username, holder.subscription_id.0
        )
    } else {
        format!(
            "Пользователь Remnawave {} уже привязан к подписке №{} клиента {}",
            user.username,
            holder.subscription_id.0,
            person_text(&holder.client)
        )
    }
}

fn actor_fields(actor: Actor) -> (ActorKind, Option<i64>) {
    match actor {
        Actor::Admin(id) => (ActorKind::Admin, Some(id.0)),
        Actor::Client(id) => (ActorKind::Client, Some(id.0)),
        Actor::System => (ActorKind::System, None),
    }
}

pub struct RemnaLinks<'a> {
    services: &'a Services,
}

impl<'a> RemnaLinks<'a> {
    pub fn new(services: &'a Services) -> Self {
        RemnaLinks { services }
    }

    async fn open(
        &self,
        id: SubscriptionId,
    ) -> ServiceResult<(Subscription, Option<DateTime<Utc>>)> {
        let mut conn = self.services.store().conn().await?;
        let subscription = conn.subscriptions().get(id).await?;
        if subscription.ended_at.is_some() {
            return Err(ServiceError::rejected(
                ENDED,
                "Подписка завершена, привязать её нельзя",
            ));
        }
        let client = conn.clients().get(subscription.client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::rejected(
                BLOCKED,
                "Клиент заблокирован: чтобы привязать подписку, сначала разблокируйте его",
            ));
        }
        let missing = conn.subscriptions().missing_since(id).await?;
        Ok((subscription, missing))
    }

    async fn live_count(db: &mut impl Db, client_id: ClientId) -> ServiceResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM subscriptions WHERE client_id = $1 AND ended_at IS NULL",
        )
        .bind(client_id.0)
        .fetch_one(db.conn())
        .await?)
    }

    fn single_refusal() -> ServiceError {
        ServiceError::rejected(
            SINGLE,
            "У клиента уже есть подписка, а в панели разрешена одна подписка на клиента. Если у неё нет пользователя Remnawave или он пропал, привяжите её: «Привязать к пользователю Remnawave» в блоке подписки",
        )
    }

    async fn addable(&self, client_id: ClientId) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let client = conn.clients().get(client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::rejected(
                BLOCKED,
                "Клиент заблокирован: чтобы добавить подписку, сначала разблокируйте его",
            ));
        }
        if !self.services.settings().multi_subscriptions
            && Self::live_count(&mut conn, client_id).await? > 0
        {
            return Err(Self::single_refusal());
        }
        Ok(())
    }

    async fn chosen(
        &self,
        remna: &dyn RemnawaveApi,
        request: &LinkRequest,
    ) -> ServiceResult<RemnaUser> {
        let user = match remna.get_user(request.user_id).await {
            Ok(user) => user,
            Err(err) if err.is_not_found() => {
                return Err(ServiceError::rejected(
                    USER_GONE,
                    "Этого пользователя в Remnawave уже нет. Найдите пользователя заново",
                ));
            }
            Err(err) => return Err(err.into()),
        };
        if user.username != request.username {
            return Err(ServiceError::rejected(
                USER_CHANGED,
                format!(
                    "Под номером {} в Remnawave теперь пользователь {}. Найдите пользователя заново",
                    user.id.0, user.username
                ),
            ));
        }
        Ok(user)
    }

    async fn refresh_or_queue(&self, id: SubscriptionId) -> ServiceResult<bool> {
        match self.services.remna_sync().refresh_subscription(id).await {
            Ok(done) => Ok(done),
            Err(err) => {
                tracing::warn!(subscription = id.0, error = %err, "сверка с Remnawave после привязки не прошла");
                let mut tx = self.services.store().begin().await?;
                tx.jobs()
                    .enqueue(
                        &JobPayload::SyncSubscription {
                            subscription_id: id,
                        },
                        None,
                    )
                    .await?;
                tx.commit().await?;
                Ok(false)
            }
        }
    }

    async fn panel_terms(&self, subscription: &Subscription) -> ServiceResult<Terms> {
        let mirror = &subscription.mirror;
        let mut conn = self.services.store().conn().await?;
        let tariff = match subscription.tariff_id {
            Some(id) => conn.tariffs().get(id).await.ok(),
            None => None,
        };
        let saved: Option<Vec<String>> = sqlx::query_scalar(
            "SELECT squad_names FROM remna_snapshots WHERE subscription_id = $1",
        )
        .bind(subscription.id.0)
        .fetch_optional(conn.conn())
        .await?;
        drop(conn);
        let squads = match (saved.filter(|names| !names.is_empty()), &tariff) {
            (Some(names), _) => names,
            (None, Some(tariff)) if !tariff.squads.is_empty() => {
                self.squad_names(&tariff.squads.iter().map(|s| s.0).collect::<Vec<_>>())
                    .await
            }
            _ => Vec::new(),
        };
        Ok(Terms {
            status: mirror.status,
            expire_at: mirror.expire_at,
            traffic_limit_bytes: mirror
                .traffic_limit_bytes
                .or_else(|| tariff.as_ref().map(|t| t.traffic.limit_bytes.unwrap_or(0))),
            traffic_used_bytes: mirror.traffic_used_bytes,
            traffic_strategy: mirror.traffic_strategy,
            hwid_limit: mirror.hwid_limit.or_else(|| {
                tariff.as_ref().map(|t| {
                    t.devices
                        .included
                        .saturating_add(subscription.extra_devices)
                })
            }),
            squads,
            synced: mirror.synced_at.is_some(),
        })
    }

    async fn squad_names(&self, uuids: &[Uuid]) -> Vec<String> {
        let known: HashMap<Uuid, String> = match self.services.remna() {
            Ok(remna) => match remna.internal_squads().await {
                Ok(list) => list
                    .internal_squads
                    .into_iter()
                    .map(|squad| (squad.uuid, squad.name))
                    .collect(),
                Err(err) => {
                    tracing::warn!(error = %err, "сквады Remnawave не получены");
                    HashMap::new()
                }
            },
            Err(_) => HashMap::new(),
        };
        uuids
            .iter()
            .map(|uuid| known.get(uuid).cloned().unwrap_or_else(|| uuid.to_string()))
            .collect()
    }

    async fn holders(
        &self,
        exclude: Option<SubscriptionId>,
        client_id: ClientId,
        users: &[RemnaUser],
    ) -> ServiceResult<HashMap<RemnaUserId, Holder>> {
        let names: Vec<String> = users.iter().map(|u| u.username.clone()).collect();
        let shorts: Vec<String> = users.iter().filter_map(short_of).collect();
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<HolderRow> = sqlx::query_as(
            "SELECT id, client_id, remna_user_id, remna_username, short_uuid, ended_at FROM subscriptions \
             WHERE ($1::bigint IS NULL OR id <> $1) AND ended_at IS NULL \
             AND (remna_username = ANY($2) OR short_uuid = ANY($3)) ORDER BY id",
        )
        .bind(exclude.map(|id| id.0))
        .bind(&names)
        .bind(&shorts)
        .fetch_all(conn.conn())
        .await?;
        let clients: Vec<ClientId> = rows.iter().map(|row| ClientId(row.client_id)).collect();
        let people = conn.clients().people(&clients).await?;
        let mut out = HashMap::new();
        for user in users {
            let Some(row) = rows.iter().find(|row| row.holds(user)) else {
                continue;
            };
            let holder_client = ClientId(row.client_id);
            out.insert(
                user.id,
                Holder {
                    subscription_id: SubscriptionId(row.id),
                    client: people
                        .get(&holder_client)
                        .cloned()
                        .unwrap_or_else(|| nobody(holder_client)),
                    same_client: holder_client == client_id,
                },
            );
        }
        Ok(out)
    }

    pub async fn search(&self, id: SubscriptionId, query: &str) -> ServiceResult<LinkSearch> {
        let (subscription, missing_since) = self.open(id).await?;
        let wanted = query_lookups(query)?;
        let remna = self.services.remna()?;
        let found = find(&*remna, &wanted).await?;
        let users: Vec<RemnaUser> = found.iter().map(|(user, _)| user.clone()).collect();
        let mut holders = self
            .holders(Some(id), subscription.client_id, &users)
            .await?;
        let panel = self.panel_terms(&subscription).await?;
        let state = match (&subscription.remna, missing_since) {
            (None, _) => LinkState::Unlinked,
            (Some(_), Some(_)) => LinkState::Missing,
            (Some(_), None) => LinkState::Linked,
        };
        Ok(LinkSearch {
            state,
            current: subscription.remna.clone(),
            missing_since,
            panel,
            users: found
                .into_iter()
                .map(|(user, found_by)| {
                    let holder = holders.remove(&user.id);
                    candidate(user, found_by, holder, None)
                })
                .collect(),
        })
    }

    async fn still_working(
        &self,
        remna: &dyn RemnawaveApi,
        link: &RemnaLink,
    ) -> ServiceResult<bool> {
        match remna.get_user(link.user_id).await {
            Ok(current) => Ok(same_user(link, &current)),
            Err(err) if err.is_not_found() => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    pub async fn link(
        &self,
        id: SubscriptionId,
        request: &LinkRequest,
        actor: Actor,
    ) -> ServiceResult<Linked> {
        if matches!(actor, Actor::Client(_)) {
            return Err(ServiceError::Forbidden);
        }
        let (subscription, missing_since) = self.open(id).await?;
        let remna = self.services.remna()?;
        let user = self.chosen(&*remna, request).await?;
        if let Some(link) = subscription
            .remna
            .as_ref()
            .filter(|_| missing_since.is_none())
        {
            if link.user_id == user.id && same_user(link, &user) {
                return Ok(Linked {
                    user_id: user.id,
                    username: user.username,
                    refreshed: false,
                    released: Vec::new(),
                });
            }
            if self.still_working(&*remna, link).await? {
                return Err(ServiceError::rejected(
                    WORKING,
                    format!(
                        "Подписка уже привязана к пользователю Remnawave {} и работает",
                        link.username
                    ),
                ));
            }
        }
        let now = self.services.now();
        let short = short_of(&user);
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Subscription, id.0).await?;
        let current = tx.subscriptions().lock(id).await?;
        if current.ended_at.is_some() || current.remna != subscription.remna {
            tx.rollback().await?;
            return Err(ServiceError::rejected(
                CHANGED,
                "Подписка только что изменилась. Откройте карточку заново и повторите",
            ));
        }
        let rows = match release(&mut tx, Some(id), current.client_id, &user).await? {
            Ok(rows) => rows,
            Err(holder) => {
                tx.rollback().await?;
                return Err(ServiceError::rejected(TAKEN, taken_text(&user, &holder)));
            }
        };
        let linked = sqlx::query(
            "UPDATE subscriptions SET remna_user_id = $2, remna_username = $3, short_uuid = $4, \
             remna_missing_at = NULL, remna_updated_at = NULL WHERE id = $1",
        )
        .bind(id.0)
        .bind(user.id.0)
        .bind(&user.username)
        .bind(&short)
        .execute(tx.conn())
        .await;
        match linked {
            Ok(_) => {}
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                tx.rollback().await?;
                return Err(taken_race(&user));
            }
            Err(err) => return Err(err.into()),
        }
        let (actor_kind, actor_id) = actor_fields(actor);
        tx.audit()
            .record(&AuditEntry {
                actor_kind,
                actor_id,
                action: REMNA_LINKED.to_string(),
                entity_kind: Some("subscription".into()),
                entity_id: Some(id.0),
                details: json!({
                    "client_id": current.client_id,
                    "remna_user_id": user.id,
                    "remna_username": user.username,
                    "short_uuid": short,
                    "previous": current.remna,
                    "missing_since": missing_since,
                    "released": rows.iter().map(HolderRow::noted).collect::<Vec<_>>(),
                    "at": now,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        let refreshed = self.refresh_or_queue(id).await?;
        Ok(Linked {
            user_id: user.id,
            username: user.username,
            refreshed,
            released: rows.iter().map(|row| SubscriptionId(row.id)).collect(),
        })
    }

    pub async fn add_search(&self, client_id: ClientId, query: &str) -> ServiceResult<AddSearch> {
        self.addable(client_id).await?;
        let wanted = query_lookups(query)?;
        let remna = self.services.remna()?;
        let found = find(&*remna, &wanted).await?;
        let users: Vec<RemnaUser> = found.iter().map(|(user, _)| user.clone()).collect();
        let mut holders = self.holders(None, client_id, &users).await?;
        let tariffs = tariffs(self.services).await?;
        Ok(AddSearch {
            users: found
                .into_iter()
                .map(|(user, found_by)| {
                    let holder = holders.remove(&user.id);
                    let tariff = TariffPick::from(match_tariff(&user, &tariffs));
                    candidate(user, found_by, holder, Some(tariff))
                })
                .collect(),
        })
    }

    pub async fn add(
        &self,
        client_id: ClientId,
        request: &LinkRequest,
        actor: Actor,
    ) -> ServiceResult<Added> {
        if matches!(actor, Actor::Client(_)) {
            return Err(ServiceError::Forbidden);
        }
        self.addable(client_id).await?;
        let remna = self.services.remna()?;
        let user = self.chosen(&*remna, request).await?;
        let tariffs = tariffs(self.services).await?;
        let pick = TariffPick::from(match_tariff(&user, &tariffs));
        let (tariff_id, tariff_name, extra) = match &pick {
            TariffPick::One {
                id,
                name,
                extra_devices,
            } => (Some(*id), Some(name.clone()), *extra_devices),
            _ => (None, None, 0),
        };
        let kind = if user.tag.as_deref() == Some(TRIAL_TAG) {
            SubscriptionKind::Trial
        } else if tariff_id.is_some() {
            SubscriptionKind::Paid
        } else {
            SubscriptionKind::Manual
        };
        let now = self.services.now();
        let short = short_of(&user);
        let multi = self.services.settings().multi_subscriptions;
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Client, client_id.0).await?;
        if !multi && Self::live_count(&mut tx, client_id).await? > 0 {
            tx.rollback().await?;
            return Err(Self::single_refusal());
        }
        let rows = match release(&mut tx, None, client_id, &user).await? {
            Ok(rows) => rows,
            Err(holder) => {
                tx.rollback().await?;
                return Err(ServiceError::rejected(TAKEN, taken_text(&user, &holder)));
            }
        };
        let created: Option<i64> = sqlx::query_scalar(
            "INSERT INTO subscriptions (client_id, tariff_id, kind, extra_devices, remna_user_id, remna_username, \
             short_uuid, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(client_id.0)
        .bind(tariff_id)
        .bind(kind.as_str())
        .bind(i32::try_from(extra).unwrap_or(0))
        .bind(user.id.0)
        .bind(&user.username)
        .bind(&short)
        .bind(user.created_at)
        .fetch_optional(tx.conn())
        .await?;
        let Some(created) = created.map(SubscriptionId) else {
            tx.rollback().await?;
            return Err(taken_race(&user));
        };
        tx.subscriptions()
            .apply_mirror(created, &user.mirror(None, now))
            .await?;
        snapshot::save(&mut tx, self.services.secrets(), created, &user).await?;
        let (actor_kind, actor_id) = actor_fields(actor);
        tx.audit()
            .record(&AuditEntry {
                actor_kind,
                actor_id,
                action: REMNA_ADDED.to_string(),
                entity_kind: Some("subscription".into()),
                entity_id: Some(created.0),
                details: json!({
                    "client_id": client_id,
                    "remna_user_id": user.id,
                    "remna_username": user.username,
                    "short_uuid": short,
                    "kind": kind.as_str(),
                    "tariff": pick,
                    "released": rows.iter().map(HolderRow::noted).collect::<Vec<_>>(),
                    "at": now,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        let refreshed = self.refresh_or_queue(created).await?;
        Ok(Added {
            subscription_id: created,
            user_id: user.id,
            username: user.username,
            tariff: tariff_name,
            refreshed,
        })
    }
}
