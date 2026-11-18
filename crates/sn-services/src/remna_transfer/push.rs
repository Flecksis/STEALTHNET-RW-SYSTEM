use super::remna::{Pace, RemnaIndex, panel_down, retry};
use super::runs::NoteRow;
use super::snapshot::{self, Snapshot};
use super::{
    Control, Example, Mode, Plan, Progress, RemnaSquad, RunEnd, SquadPlan, StartOptions,
    merge_progress, text,
};
use crate::error::ServiceResult;
use crate::remna_sync::store_user;
use crate::services::Services;
use chrono::{DateTime, FixedOffset, Utc};
use serde::Deserialize;
use sn_domain::client::{IdentityKind, normalize_identity};
use sn_domain::ids::SubscriptionId;
use sn_domain::subscription::{
    REMNA_USERNAME_MAX, REMNA_USERNAME_MIN, RemnaLink, TRIAL_TAG, remna_description,
};
use sn_domain::traffic::TrafficStrategy;
use sn_remnawave::{
    CreateUserRequest, RemnaError, RemnaUser, RemnawaveApi, RevokeRequest, UpdateUserRequest,
    UserKeys, UserStatus,
};
use sn_store::{Db, SecretBox};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

const PAGE: i64 = 500;
const USERNAME_TAKEN: &str = "A019";
const SHORT_TAKEN: &str = "A020";
const VLESS_TAKEN: &str = "A021";

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct PushRow {
    pub id: i64,
    pub client_id: i64,
    pub client_public_id: Uuid,
    pub client_status: String,
    pub client_deleted: bool,
    pub kind: String,
    pub remna_user_id: Option<i64>,
    pub remna_username: Option<String>,
    pub short_uuid: Option<String>,
    pub status: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub traffic_limit_bytes: Option<i64>,
    pub traffic_strategy: Option<String>,
    pub hwid_limit: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub ended: bool,
    pub telegram: Option<String>,
    pub tg_username: Option<String>,
    pub email: Option<String>,
    pub snap: bool,
    pub snap_squads: Option<Vec<Uuid>>,
    pub snap_squad_names: Option<Vec<String>>,
    pub snap_external: Option<Uuid>,
    pub snap_tag: Option<String>,
    pub snap_telegram: Option<i64>,
    pub snap_email: Option<String>,
    pub snap_description: Option<String>,
    pub snap_created_at: Option<DateTime<Utc>>,
    pub snap_reset_at: Option<DateTime<Utc>>,
    pub snap_url: Option<String>,
    pub keys_nonce: Option<Vec<u8>>,
    pub keys_sealed: Option<Vec<u8>>,
    pub tariff_squads: Option<Vec<Uuid>>,
    pub tariff_external: Option<Uuid>,
    pub last_target: Option<serde_json::Value>,
}

const ROWS: &str = "SELECT s.id, s.client_id, c.public_id AS client_public_id, c.status AS client_status, \
     (c.deleted_at IS NOT NULL) AS client_deleted, s.kind, s.remna_user_id, s.remna_username, s.short_uuid, \
     s.status, s.expire_at, s.traffic_limit_bytes, s.traffic_strategy, s.hwid_limit, s.created_at, \
     (s.ended_at IS NOT NULL) AS ended, tg.value AS telegram, tg.meta->>'username' AS tg_username, em.value AS email, \
     (n.subscription_id IS NOT NULL) AS snap, n.squads AS snap_squads, n.squad_names AS snap_squad_names, \
     n.external_squad_uuid AS snap_external, n.tag AS snap_tag, n.telegram_id AS snap_telegram, n.email AS snap_email, \
     n.description AS snap_description, n.remna_created_at AS snap_created_at, \
     n.last_traffic_reset_at AS snap_reset_at, n.subscription_url AS snap_url, n.keys_nonce, n.keys_sealed, \
     (SELECT array_agg(q.squad_uuid ORDER BY q.position, q.squad_uuid) FROM tariff_squads q WHERE q.tariff_id = s.tariff_id) AS tariff_squads, \
     t.external_squad_uuid AS tariff_external, \
     (SELECT o.plan->'target' FROM orders o WHERE o.subscription_id = s.id AND o.plan IS NOT NULL \
        AND o.status = 'fulfilled' ORDER BY o.id DESC LIMIT 1) AS last_target \
     FROM subscriptions s JOIN clients c ON c.id = s.client_id \
     LEFT JOIN LATERAL (SELECT value, meta FROM client_identities WHERE client_id = s.client_id AND kind = 'telegram' ORDER BY id LIMIT 1) tg ON true \
     LEFT JOIN LATERAL (SELECT value FROM client_identities WHERE client_id = s.client_id AND kind = 'email' ORDER BY id LIMIT 1) em ON true \
     LEFT JOIN remna_snapshots n ON n.subscription_id = s.id \
     LEFT JOIN tariffs t ON t.id = s.tariff_id";

async fn page(db: &mut impl Db, after: i64) -> ServiceResult<Vec<PushRow>> {
    Ok(
        sqlx::query_as::<_, PushRow>(&format!("{ROWS} WHERE s.id > $1 ORDER BY s.id LIMIT $2"))
            .bind(after)
            .bind(PAGE)
            .fetch_all(db.conn())
            .await?,
    )
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LastTarget {
    #[serde(default)]
    expire_at: Option<DateTime<Utc>>,
    #[serde(default)]
    squads: Vec<Uuid>,
    #[serde(default)]
    external_squad: Option<Uuid>,
    #[serde(default)]
    traffic_limit_bytes: Option<u64>,
    #[serde(default)]
    traffic_strategy: Option<TrafficStrategy>,
    #[serde(default)]
    hwid_device_limit: Option<u32>,
    #[serde(default)]
    tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub username: String,
    pub short: Option<String>,
    pub status: UserStatus,
    pub expire_at: DateTime<Utc>,
    pub traffic: i64,
    pub strategy: TrafficStrategy,
    pub hwid: Option<i64>,
    pub squads: Vec<Uuid>,
    pub old_squads: Vec<Uuid>,
    pub external: Option<Uuid>,
    pub telegram: Option<i64>,
    pub email: Option<String>,
    pub description: String,
    pub tag: Option<String>,
    pub keys: UserKeys,
    pub created_at: Option<DateTime<Utc>>,
    pub reset_at: Option<DateTime<Utc>>,
    pub old_url: Option<String>,
}

pub(crate) struct Squads {
    pub known: bool,
    pub names: HashMap<Uuid, String>,
    pub mapping: HashMap<Uuid, Option<Uuid>>,
    pub externals: Option<HashSet<Uuid>>,
}

impl Squads {
    fn map(&self, old: &[Uuid]) -> Vec<Uuid> {
        let mut out: Vec<Uuid> = Vec::new();
        for uuid in old {
            let mapped = if !self.known || self.names.contains_key(uuid) {
                Some(*uuid)
            } else {
                self.mapping.get(uuid).copied().flatten()
            };
            if let Some(uuid) = mapped
                && !out.contains(&uuid)
            {
                out.push(uuid);
            }
        }
        out.sort();
        out
    }

    fn external(&self, old: Option<Uuid>) -> Option<Uuid> {
        let old = old?;
        match &self.externals {
            Some(known) if !known.contains(&old) => None,
            _ => Some(old),
        }
    }
}

pub(crate) struct Ctx<'a> {
    pub offset: FixedOffset,
    pub secrets: Option<&'a SecretBox>,
    pub squads: Squads,
    pub now: DateTime<Utc>,
}

pub(crate) enum Prepared {
    Skip(&'static str, &'static str),
    Ready(Box<Wanted>),
}

fn username_ok(name: &str) -> bool {
    let len = name.chars().count();
    (REMNA_USERNAME_MIN..=REMNA_USERNAME_MAX).contains(&len)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn tag_ok(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 16
        && tag
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn email_ok(email: Option<&str>) -> Option<String> {
    let email = email?.trim().to_lowercase();
    normalize_identity(IdentityKind::Email, &email).ok()
}

pub(crate) fn issued(row: &PushRow) -> bool {
    row.remna_username.is_some()
        && (row.short_uuid.as_deref().is_some_and(|s| !s.is_empty())
            || row.expire_at.is_some()
            || row.snap)
}

pub(crate) fn prepare(row: &PushRow, ctx: &Ctx<'_>) -> Prepared {
    if row.client_deleted {
        return Prepared::Skip("client_deleted", "клиент удалён");
    }
    if row.ended {
        return Prepared::Skip("ended", "подписка завершена");
    }
    let Some(username) = row.remna_username.clone().filter(|_| issued(row)) else {
        return Prepared::Skip("not_issued", "подписка ещё не выдана в Remnawave");
    };
    if !username_ok(&username) {
        return Prepared::Skip(
            "bad_name",
            "имя пользователя не подходит для Remnawave: нужно 3–36 латинских букв, цифр, «_» или «-»",
        );
    }
    let last: LastTarget = row
        .last_target
        .clone()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let Some(expire_at) = row.expire_at.or(last.expire_at) else {
        return Prepared::Skip("no_expire", "в панели нет срока подписки");
    };
    let snap = row.snap.then(|| Snapshot {
        squads: row.snap_squads.clone().unwrap_or_default(),
        squad_names: row.snap_squad_names.clone().unwrap_or_default(),
        external_squad: row.snap_external,
        tag: row.snap_tag.clone(),
        telegram_id: row.snap_telegram,
        email: row.snap_email.clone(),
        description: row.snap_description.clone(),
        created_at: row.snap_created_at,
        last_traffic_reset_at: row.snap_reset_at,
        subscription_url: row.snap_url.clone(),
        keys: snapshot::open_keys(
            ctx.secrets,
            SubscriptionId(row.id),
            row.keys_nonce.as_deref(),
            row.keys_sealed.as_deref(),
        ),
    });
    let disabled = row.client_status == "blocked" || row.status.as_deref() == Some("disabled");
    let old_squads = match &snap {
        Some(snap) => snap.squads.clone(),
        None if !last.squads.is_empty() => last.squads.clone(),
        None => row.tariff_squads.clone().unwrap_or_default(),
    };
    let external = match &snap {
        Some(snap) => snap.external_squad,
        None => last.external_squad.or(row.tariff_external),
    };
    let telegram = match &snap {
        Some(snap) => snap.telegram_id,
        None => row.telegram.as_deref().and_then(|t| t.parse().ok()),
    };
    let email = match &snap {
        Some(snap) => email_ok(snap.email.as_deref()),
        None => email_ok(row.email.as_deref()),
    };
    let tag = match &snap {
        Some(snap) => snap.tag.clone(),
        None => last
            .tag
            .clone()
            .or_else(|| (row.kind == "trial").then(|| TRIAL_TAG.to_string())),
    }
    .filter(|t| tag_ok(t));
    let wanted = Wanted {
        username,
        short: row.short_uuid.clone().filter(|s| !s.is_empty()),
        status: if disabled {
            UserStatus::Disabled
        } else {
            UserStatus::Active
        },
        expire_at,
        traffic: row
            .traffic_limit_bytes
            .or_else(|| last.traffic_limit_bytes.and_then(|v| i64::try_from(v).ok()))
            .unwrap_or(0),
        strategy: row
            .traffic_strategy
            .as_deref()
            .and_then(|s| TrafficStrategy::parse_remna(s).ok())
            .or(last.traffic_strategy)
            .unwrap_or_default(),
        hwid: row
            .hwid_limit
            .map(i64::from)
            .or_else(|| last.hwid_device_limit.map(i64::from)),
        squads: ctx.squads.map(&old_squads),
        old_squads,
        external: ctx.squads.external(external),
        telegram,
        email,
        description: snap
            .as_ref()
            .and_then(|s| s.description.clone())
            .unwrap_or_else(|| remna_description(row.client_public_id)),
        tag,
        keys: snap.as_ref().map(|s| s.keys.clone()).unwrap_or_default(),
        created_at: snap
            .as_ref()
            .and_then(|s| s.created_at)
            .or(Some(row.created_at)),
        reset_at: snap.as_ref().and_then(|s| s.last_traffic_reset_at),
        old_url: snap.and_then(|s| s.subscription_url),
    };
    Prepared::Ready(Box::new(wanted))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Field {
    Link,
    Expire,
    Status,
    Traffic,
    Strategy,
    Devices,
    Squads,
    External,
    Telegram,
    Email,
}

impl Field {
    pub const fn label(self) -> &'static str {
        match self {
            Field::Link => "Ссылка подписки",
            Field::Expire => "Срок",
            Field::Status => "Статус",
            Field::Traffic => "Лимит трафика",
            Field::Strategy => "Сброс трафика",
            Field::Devices => "Лимит устройств",
            Field::Squads => "Сквады",
            Field::External => "Внешний сквад",
            Field::Telegram => "Telegram ID",
            Field::Email => "Почта",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Diff {
    pub field: Field,
    pub panel: String,
    pub remna: String,
}

impl Diff {
    fn line(&self) -> String {
        format!(
            "{}: в панели {}, в Remnawave {}",
            self.field.label(),
            self.panel,
            self.remna
        )
    }
}

fn millis(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

fn sorted(list: &[Uuid]) -> Vec<Uuid> {
    let set: BTreeSet<Uuid> = list.iter().copied().collect();
    set.into_iter().collect()
}

pub(crate) fn compare(user: &RemnaUser, wanted: &Wanted, ctx: &Ctx<'_>) -> (Vec<Diff>, bool) {
    let mut diffs = Vec::new();
    let mut later = false;
    if let Some(short) = &wanted.short
        && short != &user.short_uuid
    {
        diffs.push(Diff {
            field: Field::Link,
            panel: text::short_id(short),
            remna: text::short_id(&user.short_uuid),
        });
    }
    let (want, have) = (millis(wanted.expire_at), millis(user.expire_at));
    if want > have + 1000 {
        diffs.push(Diff {
            field: Field::Expire,
            panel: text::date(wanted.expire_at, ctx.offset),
            remna: text::date(user.expire_at, ctx.offset),
        });
    } else if have > want + 1000 {
        later = true;
    }
    let status_differs = match wanted.status {
        UserStatus::Disabled => user.status != UserStatus::Disabled,
        _ => user.status == UserStatus::Disabled,
    };
    if status_differs {
        diffs.push(Diff {
            field: Field::Status,
            panel: text::status(wanted.status).into(),
            remna: text::status(user.status).into(),
        });
    }
    if wanted.traffic != user.traffic_limit_bytes {
        diffs.push(Diff {
            field: Field::Traffic,
            panel: text::traffic(wanted.traffic),
            remna: text::traffic(user.traffic_limit_bytes),
        });
    }
    if wanted.strategy != user.traffic_limit_strategy {
        diffs.push(Diff {
            field: Field::Strategy,
            panel: text::strategy(wanted.strategy).into(),
            remna: text::strategy(user.traffic_limit_strategy).into(),
        });
    }
    if wanted.hwid != user.hwid_device_limit {
        diffs.push(Diff {
            field: Field::Devices,
            panel: text::devices(wanted.hwid),
            remna: text::devices(user.hwid_device_limit),
        });
    }
    let have_squads: Vec<Uuid> = sorted(&user.squad_ids().iter().map(|s| s.0).collect::<Vec<_>>());
    if sorted(&wanted.squads) != have_squads {
        let mut names = ctx.squads.names.clone();
        for squad in &user.active_internal_squads {
            names
                .entry(squad.uuid)
                .or_insert_with(|| squad.name.clone());
        }
        diffs.push(Diff {
            field: Field::Squads,
            panel: text::squads(&wanted.squads, &names),
            remna: text::squads(&have_squads, &names),
        });
    }
    if wanted.external != user.external_squad_uuid {
        let show = |v: Option<Uuid>| {
            v.map_or_else(|| "нет".to_string(), |u| text::short_id(&u.to_string()))
        };
        diffs.push(Diff {
            field: Field::External,
            panel: show(wanted.external),
            remna: show(user.external_squad_uuid),
        });
    }
    if let Some(tg) = wanted.telegram
        && user.telegram_id != Some(tg)
    {
        diffs.push(Diff {
            field: Field::Telegram,
            panel: tg.to_string(),
            remna: user
                .telegram_id
                .map_or_else(|| "нет".to_string(), |v| v.to_string()),
        });
    }
    if let Some(email) = &wanted.email
        && user.email.as_deref().map(str::to_lowercase).as_deref() != Some(email.as_str())
    {
        diffs.push(Diff {
            field: Field::Email,
            panel: email.clone(),
            remna: user.email.clone().unwrap_or_else(|| "нет".into()),
        });
    }
    (diffs, later)
}

pub(crate) enum Decision<'i> {
    Create,
    Conflict(&'static str, String),
    Existing {
        user: &'i RemnaUser,
        diffs: Vec<Diff>,
        later: bool,
        relink: bool,
    },
}

fn ours_by_name(user: &RemnaUser, wanted: &Wanted) -> bool {
    user.description.as_deref() == Some(wanted.description.as_str())
        || (wanted.telegram.is_some() && user.telegram_id == wanted.telegram)
        || (wanted.email.is_some() && user.email.as_deref().map(str::to_lowercase) == wanted.email)
}

pub(crate) fn decide<'i>(
    row: &PushRow,
    wanted: &Wanted,
    index: &'i RemnaIndex,
    claimed: &HashMap<i64, i64>,
    ctx: &Ctx<'_>,
) -> Decision<'i> {
    let by_short = wanted.short.as_deref().and_then(|s| index.by_short(s));
    let user = match by_short {
        Some(user) => user,
        None => match index.by_name(&wanted.username) {
            None => return Decision::Create,
            Some(user) if ours_by_name(user, wanted) => user,
            Some(user) => {
                return Decision::Conflict(
                    "name_taken",
                    format!(
                        "имя {} в Remnawave занято другим пользователем (№{}) с другой ссылкой",
                        wanted.username, user.id.0
                    ),
                );
            }
        },
    };
    if let Some(other) = claimed.get(&user.id.0)
        && *other != row.id
    {
        return Decision::Conflict(
            "shared",
            format!(
                "пользователь Remnawave {} уже сопоставлен с подпиской №{other}",
                user.username
            ),
        );
    }
    let (diffs, later) = compare(user, wanted, ctx);
    let relink = row.remna_user_id != Some(user.id.0)
        || row.remna_username.as_deref() != Some(user.username.as_str())
        || (row.short_uuid.as_deref() != Some(user.short_uuid.as_str())
            && !diffs.iter().any(|d| d.field == Field::Link));
    Decision::Existing {
        user,
        diffs,
        later,
        relink,
    }
}

fn who(row: &PushRow) -> String {
    text::who(
        row.id,
        row.tg_username.as_deref(),
        row.telegram.as_deref(),
        row.email.as_deref(),
        row.client_id,
    )
}

fn note(row: &PushRow, code: &str, details: impl Into<String>) -> NoteRow {
    NoteRow {
        client_id: Some(row.client_id),
        subscription_id: Some(row.id),
        remna_user_id: row.remna_user_id,
        who: Some(who(row)),
        ..NoteRow::new(code, details)
    }
}

fn traffic_line(limit: i64, strategy: TrafficStrategy) -> String {
    if limit <= 0 {
        "Трафик без лимита".into()
    } else if strategy == TrafficStrategy::NoReset {
        format!("Трафик: {}, без сброса", text::traffic(limit))
    } else {
        format!(
            "Трафик: {}, сброс {}",
            text::traffic(limit),
            text::strategy(strategy)
        )
    }
}

fn create_lines(wanted: &Wanted, ctx: &Ctx<'_>) -> Vec<String> {
    let mut lines = vec![
        format!("Срок до {}", text::date(wanted.expire_at, ctx.offset)),
        traffic_line(wanted.traffic, wanted.strategy),
        format!("Устройства: {}", text::devices(wanted.hwid)),
        format!(
            "Сквады: {}",
            text::squads(&wanted.squads, &ctx.squads.names)
        ),
    ];
    lines.push(match &wanted.short {
        Some(short) => format!("Ссылка сохранится: …/{short}"),
        None => "Ссылка будет новой: в панели её нет".into(),
    });
    if wanted.status == UserStatus::Disabled {
        lines.push("Будет отключена, как в панели".into());
    }
    lines
}

async fn squads_ctx(
    services: &Services,
    remna: &dyn RemnawaveApi,
    pace: &Pace,
    mapping: HashMap<Uuid, Option<Uuid>>,
) -> ServiceResult<(Squads, Vec<RemnaSquad>)> {
    let (known, names, list) = match retry(pace, || remna.internal_squads()).await {
        Ok(found) => {
            let list: Vec<RemnaSquad> = found
                .internal_squads
                .iter()
                .map(|s| RemnaSquad {
                    uuid: s.uuid,
                    name: s.name.clone(),
                })
                .collect();
            let names = list.iter().map(|s| (s.uuid, s.name.clone())).collect();
            (true, names, list)
        }
        Err(err) if panel_down(&err) => return Err(err.into()),
        Err(_) => (false, HashMap::new(), Vec::new()),
    };
    let externals = match retry(pace, || remna.external_squads()).await {
        Ok(found) => Some(found.external_squads.iter().map(|s| s.uuid).collect()),
        Err(err) if panel_down(&err) => return Err(err.into()),
        Err(_) => None,
    };
    let _ = services;
    Ok((
        Squads {
            known,
            names,
            mapping,
            externals,
        },
        list,
    ))
}

fn suggest(name: Option<&str>, list: &[RemnaSquad]) -> Option<Uuid> {
    let name = name?.trim().to_lowercase();
    let found: Vec<&RemnaSquad> = list
        .iter()
        .filter(|s| s.name.trim().to_lowercase() == name)
        .collect();
    (found.len() == 1).then(|| found[0].uuid)
}

async fn known_names(db: &mut impl Db) -> ServiceResult<HashMap<Uuid, String>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (squad) squad, name FROM remna_snapshots, \
         unnest(squads, squad_names) AS pair(squad, name) WHERE name IS NOT NULL ORDER BY squad, name",
    )
    .fetch_all(db.conn())
    .await?;
    Ok(rows.into_iter().collect())
}

async fn tariff_names(db: &mut impl Db) -> ServiceResult<HashMap<Uuid, Vec<String>>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT q.squad_uuid, t.name FROM tariff_squads q JOIN tariffs t ON t.id = q.tariff_id \
         WHERE t.deleted_at IS NULL ORDER BY t.sort, t.id",
    )
    .fetch_all(db.conn())
    .await?;
    let mut out: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (uuid, name) in rows {
        out.entry(uuid).or_default().push(name);
    }
    Ok(out)
}

fn host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_lowercase))
}

pub(crate) async fn plan(services: &Services, pace: &Pace) -> ServiceResult<(Plan, Vec<NoteRow>)> {
    let remna = services.remna()?;
    let index = RemnaIndex::load(remna.as_ref(), pace).await?;
    let (squads, remna_squads) = squads_ctx(services, remna.as_ref(), pace, HashMap::new()).await?;
    let sub_domain = match retry(pace, || remna.configuration()).await {
        Ok(config) => Some(config.misc.sub_public_domain).filter(|d| !d.trim().is_empty()),
        Err(_) => None,
    };
    let mut conn = services.store().conn().await?;
    let mut squads = squads;
    if squads.known {
        for (old, name) in known_names(&mut conn).await? {
            if !squads.names.contains_key(&old) {
                squads
                    .mapping
                    .insert(old, suggest(Some(&name), &remna_squads));
            }
        }
    }
    let ctx = Ctx {
        offset: services.settings().remna_offset(),
        secrets: services.secrets(),
        squads,
        now: services.now(),
    };
    let mut plan = Plan {
        direction: super::Direction::Push,
        remna_users: index.len() as u64,
        sub_domain: sub_domain.clone(),
        secrets: ctx.secrets.is_some(),
        remna_squads: remna_squads.clone(),
        ..Plan::default()
    };
    let mut notes = Vec::new();
    let mut claimed: HashMap<i64, i64> = HashMap::new();
    let mut missing: HashMap<Uuid, SquadPlan> = HashMap::new();
    let mut hosts: HashMap<String, u64> = HashMap::new();
    let mut after = 0i64;
    let tariffs = tariff_names(&mut conn).await?;
    loop {
        let rows = page(&mut conn, after).await?;
        let Some(last) = rows.last() else { break };
        after = last.id;
        for row in &rows {
            plan.scanned += 1;
            let wanted = match prepare(row, &ctx) {
                Prepared::Skip(code, reason) => {
                    if !matches!(code, "ended" | "client_deleted" | "not_issued") {
                        notes.push(note(row, code, reason));
                    }
                    plan.skip(code, reason, Some(who(row)));
                    continue;
                }
                Prepared::Ready(wanted) => wanted,
            };
            if row.snap {
                plan.snapshots += 1;
            }
            if let Some(url) = wanted.old_url.as_deref().and_then(host) {
                *hosts.entry(url).or_default() += 1;
            }
            if ctx.squads.known {
                let names = row.snap_squad_names.clone().unwrap_or_default();
                let snap_squads = row.snap_squads.clone().unwrap_or_default();
                for old in &wanted.old_squads {
                    if ctx.squads.names.contains_key(old) {
                        continue;
                    }
                    let name = snap_squads
                        .iter()
                        .position(|u| u == old)
                        .and_then(|at| names.get(at).cloned());
                    let entry = missing.entry(*old).or_insert_with(|| SquadPlan {
                        old: *old,
                        name: None,
                        subscriptions: 0,
                        tariffs: tariffs.get(old).cloned().unwrap_or_default(),
                        suggested: None,
                    });
                    entry.subscriptions += 1;
                    if entry.name.is_none() {
                        entry.name = name;
                    }
                }
            }
            if wanted.external.is_none() {
                let had = if row.snap {
                    row.snap_external
                } else {
                    row.tariff_external
                };
                if had.is_some() {
                    plan.external_dropped += 1;
                }
            }
            match decide(row, &wanted, &index, &claimed, &ctx) {
                Decision::Create => {
                    if wanted.keys.is_empty() {
                        plan.keys_new += 1;
                    } else {
                        plan.keys_kept += 1;
                    }
                    if wanted.expire_at <= ctx.now {
                        plan.expired += 1;
                    }
                    if wanted.status == UserStatus::Disabled {
                        plan.disabled += 1;
                    }
                    if wanted.short.is_none() {
                        plan.link_change.add(|| Example {
                            who: who(row),
                            details: vec!["в панели нет ссылки этой подписки".into()],
                        });
                        notes.push(note(
                            row,
                            "link_changed",
                            "в панели нет ссылки этой подписки — Remnawave выдаст новую",
                        ));
                    }
                    let lines = create_lines(&wanted, &ctx);
                    plan.create.add(|| Example {
                        who: who(row),
                        details: lines,
                    });
                }
                Decision::Conflict(code, reason) => {
                    notes.push(note(row, code, reason.clone()));
                    plan.skip(
                        code,
                        conflict_title(code),
                        Some(format!("{}: {reason}", who(row))),
                    );
                }
                Decision::Existing {
                    user,
                    diffs,
                    later,
                    relink,
                } => {
                    claimed.insert(user.id.0, row.id);
                    if later {
                        plan.later_in_remna += 1;
                        notes.push(note(
                            row,
                            "later",
                            format!(
                                "в Remnawave срок до {}, в панели до {} — срок не сокращаем",
                                text::date(user.expire_at, ctx.offset),
                                text::date(wanted.expire_at, ctx.offset)
                            ),
                        ));
                    }
                    if diffs.is_empty() {
                        plan.same += 1;
                        if relink {
                            plan.relink.add(|| Example {
                                who: who(row),
                                details: vec![format!(
                                    "Пользователь Remnawave №{} ({})",
                                    user.id.0, user.username
                                )],
                            });
                        }
                    } else {
                        for diff in &diffs {
                            *plan
                                .fields
                                .entry(diff.field.label().to_string())
                                .or_default() += 1;
                        }
                        let lines: Vec<String> = diffs.iter().map(Diff::line).collect();
                        notes.push(note(row, "differs", lines.join("; ")));
                        plan.fix.add(|| Example {
                            who: who(row),
                            details: lines,
                        });
                    }
                }
            }
        }
    }
    let mut squads: Vec<SquadPlan> = missing
        .into_values()
        .map(|mut s| {
            s.suggested = suggest(s.name.as_deref(), &remna_squads);
            s
        })
        .collect();
    squads.sort_by(|a, b| {
        b.subscriptions
            .cmp(&a.subscriptions)
            .then(a.old.cmp(&b.old))
    });
    plan.squads = squads;
    if let Some(domain) = sub_domain.as_deref() {
        let domain = domain.trim().to_lowercase();
        let other: u64 = hosts
            .iter()
            .filter(|(host, _)| !domain.contains(host.as_str()) && !host.contains(&domain))
            .map(|(_, n)| *n)
            .sum();
        if other > 0 {
            plan.warnings.push(format!(
                "Адрес подписок в этой Remnawave — {domain}, а у {} ссылки на другом адресе. Ссылка = адрес подписок + код ссылки: код сохраним, но адрес сменится. Чтобы ссылки не поменялись, задайте в Remnawave прежний адрес подписок (SUB_PUBLIC_DOMAIN).",
                text::count(other, "подписки", "подписок", "подписок")
            ));
        }
    }
    if !plan.secrets {
        plan.warnings.push(
            "Ключ шифрования панели не задан, поэтому ключи протоколов не сохранялись: у созданных пользователей ключи будут новыми. Приложения получат их при обновлении подписки, ссылка не изменится.".into(),
        );
    }
    Ok((plan, notes))
}

fn conflict_title(code: &str) -> &'static str {
    match code {
        "name_taken" => "имя занято другим пользователем Remnawave",
        _ => "пользователь Remnawave уже у другой подписки",
    }
}

fn create_request(wanted: &Wanted) -> CreateUserRequest {
    CreateUserRequest {
        username: wanted.username.clone(),
        status: (wanted.status == UserStatus::Disabled).then_some(UserStatus::Disabled),
        short_uuid: wanted.short.clone(),
        vless_uuid: wanted.keys.vless_uuid.clone(),
        trojan_password: wanted.keys.trojan_password.clone(),
        ss_password: wanted.keys.ss_password.clone(),
        expire_at: wanted.expire_at,
        created_at: wanted.created_at,
        last_traffic_reset_at: wanted.reset_at,
        traffic_limit_bytes: wanted.traffic.max(0),
        traffic_limit_strategy: wanted.strategy,
        description: Some(wanted.description.clone()),
        tag: wanted.tag.clone(),
        telegram_id: wanted.telegram,
        email: wanted.email.clone(),
        hwid_device_limit: wanted.hwid,
        active_internal_squads: wanted.squads.clone(),
        external_squad_uuid: wanted.external,
    }
}

enum Made {
    User(Box<RemnaUser>, Option<&'static str>),
    Failed(String),
    Down(String),
}

async fn find_after_conflict(
    remna: &dyn RemnawaveApi,
    pace: &Pace,
    wanted: &Wanted,
) -> Result<Option<RemnaUser>, RemnaError> {
    if let Some(short) = &wanted.short {
        match retry(pace, || remna.get_user_by_short_uuid(short)).await {
            Ok(user) if user.username == wanted.username => return Ok(Some(user)),
            Ok(_) => {}
            Err(err) if err.is_not_found() => {}
            Err(err) => return Err(err),
        }
    }
    match retry(pace, || remna.get_user_by_username(&wanted.username)).await {
        Ok(user)
            if wanted.short.as_deref() == Some(user.short_uuid.as_str())
                || user.description.as_deref() == Some(wanted.description.as_str()) =>
        {
            Ok(Some(user))
        }
        Ok(_) => Ok(None),
        Err(err) if err.is_not_found() => Ok(None),
        Err(err) => Err(err),
    }
}

async fn create(remna: &dyn RemnawaveApi, pace: &Pace, wanted: &Wanted) -> Made {
    let mut request = create_request(wanted);
    let mut changed: Option<&'static str> = None;
    for _ in 0..3 {
        match retry(pace, || remna.create_user(&request)).await {
            Ok(user) => return Made::User(Box::new(user), changed),
            Err(err) if matches!(err.code(), Some(USERNAME_TAKEN | SHORT_TAKEN)) => {
                match find_after_conflict(remna, pace, wanted).await {
                    Ok(Some(user)) => return Made::User(Box::new(user), changed),
                    Ok(None) if err.code() == Some(SHORT_TAKEN) && request.short_uuid.is_some() => {
                        request.short_uuid = None;
                        changed = Some(
                            "код ссылки занят другим пользователем Remnawave — выдана новая ссылка",
                        );
                    }
                    Ok(None) => {
                        return Made::Failed(format!(
                            "имя {} в Remnawave заняли во время переноса",
                            wanted.username
                        ));
                    }
                    Err(err) if panel_down(&err) => return Made::Down(err.to_string()),
                    Err(err) => return Made::Failed(err.to_string()),
                }
            }
            Err(err) if err.code() == Some(VLESS_TAKEN) => {
                request.vless_uuid = None;
                request.trojan_password = None;
                request.ss_password = None;
            }
            Err(err) if panel_down(&err) => return Made::Down(err.to_string()),
            Err(err) => return Made::Failed(err.to_string()),
        }
    }
    Made::Failed("Remnawave не приняла пользователя после нескольких попыток".into())
}

async fn fix(
    remna: &dyn RemnawaveApi,
    pace: &Pace,
    user: &RemnaUser,
    wanted: &Wanted,
    diffs: &[Diff],
) -> Made {
    let mut request = UpdateUserRequest::new(user.id);
    let mut patch = false;
    let mut link = false;
    for diff in diffs {
        patch |= diff.field != Field::Link;
        match diff.field {
            Field::Link => link = true,
            Field::Expire => request.expire_at = Some(wanted.expire_at),
            Field::Status => request.status = Some(wanted.status),
            Field::Traffic => request.traffic_limit_bytes = Some(wanted.traffic.max(0)),
            Field::Strategy => request.traffic_limit_strategy = Some(wanted.strategy),
            Field::Devices => request.hwid_device_limit = Some(wanted.hwid),
            Field::Squads => request.active_internal_squads = Some(wanted.squads.clone()),
            Field::External => request.external_squad_uuid = Some(wanted.external),
            Field::Telegram => request.telegram_id = Some(wanted.telegram),
            Field::Email => request.email = Some(wanted.email.clone()),
        }
    }
    let mut current = user.clone();
    if patch {
        match retry(pace, || remna.update_user(&request)).await {
            Ok(updated) => current = updated,
            Err(err) if panel_down(&err) => return Made::Down(err.to_string()),
            Err(err) => return Made::Failed(err.to_string()),
        }
    }
    if link && let Some(short) = &wanted.short {
        let revoke = RevokeRequest {
            revoke_only_passwords: None,
            short_uuid: Some(short.clone()),
        };
        match retry(pace, || remna.revoke_user(user.id, &revoke)).await {
            Ok(updated) => current = updated,
            Err(err) if panel_down(&err) => return Made::Down(err.to_string()),
            Err(err) => return Made::Failed(format!("ссылку вернуть не удалось: {err}")),
        }
    }
    Made::User(Box::new(current), None)
}

pub(crate) async fn bind(
    services: &Services,
    subscription: SubscriptionId,
    user: &RemnaUser,
) -> ServiceResult<Option<i64>> {
    let now = services.now();
    let mut tx = services.store().begin().await?;
    let other: Option<i64> =
        sqlx::query_scalar("SELECT id FROM subscriptions WHERE remna_username = $1 AND id <> $2")
            .bind(&user.username)
            .bind(subscription.0)
            .fetch_optional(tx.conn())
            .await?;
    if other.is_some() {
        tx.rollback().await?;
        return Ok(other);
    }
    sqlx::query(
        "UPDATE subscriptions SET remna_user_id = NULL WHERE remna_user_id = $1 AND id <> $2",
    )
    .bind(user.id.0)
    .bind(subscription.0)
    .execute(tx.conn())
    .await?;
    tx.subscriptions()
        .link_remna(
            subscription,
            &RemnaLink {
                user_id: user.id,
                username: user.username.clone(),
                short_uuid: Some(user.short_uuid.clone()).filter(|s| !s.is_empty()),
            },
        )
        .await?;
    tx.subscriptions().mark_missing(subscription, false).await?;
    let stored = store_user(&mut tx, subscription, user, None, now).await?;
    if stored.applied {
        snapshot::save(&mut tx, services.secrets(), subscription, user).await?;
    }
    tx.commit().await?;
    Ok(None)
}

fn taken(other: i64, user: &RemnaUser) -> String {
    format!(
        "имя {} в панели уже у подписки №{other} — привяжите вручную",
        user.username
    )
}

async fn update_tariffs(
    services: &Services,
    mapping: &HashMap<Uuid, Option<Uuid>>,
) -> ServiceResult<u64> {
    let mut tx = services.store().begin().await?;
    let mut changed = 0u64;
    for (old, new) in mapping {
        let Some(new) = new else { continue };
        if old == new {
            continue;
        }
        let moved = sqlx::query(
            "UPDATE tariff_squads q SET squad_uuid = $2 WHERE q.squad_uuid = $1 \
             AND NOT EXISTS (SELECT 1 FROM tariff_squads x WHERE x.tariff_id = q.tariff_id AND x.squad_uuid = $2)",
        )
        .bind(old)
        .bind(new)
        .execute(tx.conn())
        .await?;
        let dropped = sqlx::query("DELETE FROM tariff_squads WHERE squad_uuid = $1")
            .bind(old)
            .execute(tx.conn())
            .await?;
        changed += moved.rows_affected() + dropped.rows_affected();
    }
    tx.commit().await?;
    Ok(changed)
}

fn resolve_mapping(options: &StartOptions, plan: &Plan) -> HashMap<Uuid, Option<Uuid>> {
    let mut mapping: HashMap<Uuid, Option<Uuid>> =
        plan.squads.iter().map(|s| (s.old, s.suggested)).collect();
    for (old, new) in &options.squads {
        mapping.insert(*old, *new);
    }
    mapping
}

pub(crate) async fn apply(
    services: &Services,
    pace: &Pace,
    ctl: &Control<'_>,
    mode: Mode,
    options: &StartOptions,
    plan: Plan,
) -> ServiceResult<RunEnd> {
    let remna = services.remna()?;
    let base = ctl.base();
    let mut pass = Progress::default();
    let index = match RemnaIndex::load(remna.as_ref(), pace).await {
        Ok(index) => index,
        Err(err) if panel_down(&err) => {
            return Ok(RunEnd::Stopped(
                base,
                format!("Remnawave не отвечает: {err}"),
            ));
        }
        Err(err) => return Err(err.into()),
    };
    let mapping = resolve_mapping(options, &plan);
    let (squads, _) = match squads_ctx(services, remna.as_ref(), pace, mapping.clone()).await {
        Ok(found) => found,
        Err(err) => {
            return Ok(RunEnd::Stopped(
                base,
                format!("Remnawave не отвечает: {err}"),
            ));
        }
    };
    let ctx = Ctx {
        offset: services.settings().remna_offset(),
        secrets: services.secrets(),
        squads,
        now: services.now(),
    };
    let mut index = index;
    pass.total = {
        let mut conn = services.store().conn().await?;
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions")
            .fetch_one(conn.conn())
            .await?;
        u64::try_from(total).unwrap_or(0)
    };
    if options.tariffs && !mapping.is_empty() && base.tariffs == 0 {
        pass.tariffs = update_tariffs(services, &mapping).await?;
    }
    let mut claimed: HashMap<i64, i64> = HashMap::new();
    let mut after = 0i64;
    let mut writes = 0usize;
    let mut pending_notes: Vec<NoteRow> = Vec::new();
    let remna: Arc<dyn RemnawaveApi> = remna;
    loop {
        let rows = {
            let mut conn = services.store().conn().await?;
            page(&mut conn, after).await?
        };
        let Some(last) = rows.last() else { break };
        after = last.id;
        for row in &rows {
            pass.done += 1;
            let wanted = match prepare(row, &ctx) {
                Prepared::Skip(code, reason) => {
                    pass.skipped += 1;
                    if !matches!(code, "ended" | "client_deleted" | "not_issued") {
                        pending_notes.push(NoteRow {
                            stage: "apply".into(),
                            ..note(row, code, reason)
                        });
                    }
                    continue;
                }
                Prepared::Ready(wanted) => wanted,
            };
            let decision = decide(row, &wanted, &index, &claimed, &ctx);
            let made = match decision {
                Decision::Conflict(code, reason) => {
                    pass.skipped += 1;
                    pending_notes.push(NoteRow {
                        stage: "apply".into(),
                        ..note(row, code, reason)
                    });
                    continue;
                }
                Decision::Existing {
                    user,
                    diffs,
                    relink,
                    ..
                } => {
                    claimed.insert(user.id.0, row.id);
                    if diffs.is_empty() || mode == Mode::Create {
                        if diffs.is_empty() {
                            pass.same += 1;
                        } else {
                            pass.differs += 1;
                        }
                        if relink {
                            let user = user.clone();
                            match bind(services, SubscriptionId(row.id), &user).await? {
                                None => {
                                    pass.relinked += 1;
                                    pending_notes.push(NoteRow {
                                        stage: "apply".into(),
                                        remna_user_id: Some(user.id.0),
                                        ..note(
                                            row,
                                            "relinked",
                                            format!(
                                                "привязана к пользователю Remnawave №{}",
                                                user.id.0
                                            ),
                                        )
                                    });
                                }
                                Some(other) => {
                                    pass.failed += 1;
                                    pending_notes.push(NoteRow {
                                        stage: "apply".into(),
                                        remna_user_id: Some(user.id.0),
                                        ..note(row, "failed", taken(other, &user))
                                    });
                                }
                            }
                        }
                        continue;
                    }
                    let link = diffs.iter().any(|d| d.field == Field::Link);
                    let lines: Vec<String> = diffs.iter().map(Diff::line).collect();
                    let user = user.clone();
                    writes += 1;
                    match fix(remna.as_ref(), pace, &user, &wanted, &diffs).await {
                        Made::User(updated, _) => {
                            pass.fixed += 1;
                            pending_notes.push(NoteRow {
                                stage: "apply".into(),
                                remna_user_id: Some(updated.id.0),
                                ..note(row, "fixed", lines.join("; "))
                            });
                            if link {
                                pending_notes.push(NoteRow {
                                    stage: "apply".into(),
                                    remna_user_id: Some(updated.id.0),
                                    data: serde_json::json!({ "url": updated.subscription_url }),
                                    ..note(row, "relinked", "в Remnawave вернули ссылку из панели, ключи протоколов выпущены новые")
                                });
                            }
                            Made::User(updated, None)
                        }
                        other => other,
                    }
                }
                Decision::Create => {
                    writes += 1;
                    match create(remna.as_ref(), pace, &wanted).await {
                        Made::User(user, changed) => {
                            pass.created += 1;
                            let new_link =
                                wanted.short.as_deref() != Some(user.short_uuid.as_str());
                            if new_link {
                                pass.link_changed += 1;
                                pending_notes.push(NoteRow {
                                    stage: "apply".into(),
                                    remna_user_id: Some(user.id.0),
                                    data: serde_json::json!({ "url": user.subscription_url }),
                                    ..note(
                                        row,
                                        "link_changed",
                                        changed.unwrap_or(
                                            "в панели не было ссылки — Remnawave выдала новую",
                                        ),
                                    )
                                });
                            }
                            pending_notes.push(NoteRow {
                                stage: "apply".into(),
                                remna_user_id: Some(user.id.0),
                                ..note(
                                    row,
                                    "created",
                                    format!(
                                        "создан пользователь Remnawave №{} ({})",
                                        user.id.0, user.username
                                    ),
                                )
                            });
                            Made::User(user, changed)
                        }
                        other => other,
                    }
                }
            };
            match made {
                Made::User(user, _) => {
                    claimed.insert(user.id.0, row.id);
                    if let Some(other) = bind(services, SubscriptionId(row.id), &user).await? {
                        pass.failed += 1;
                        pending_notes.push(NoteRow {
                            stage: "apply".into(),
                            remna_user_id: Some(user.id.0),
                            ..note(row, "failed", taken(other, &user))
                        });
                    }
                    index.put(*user);
                }
                Made::Failed(reason) => {
                    pass.failed += 1;
                    pending_notes.push(NoteRow {
                        stage: "apply".into(),
                        ..note(row, "failed", reason)
                    });
                }
                Made::Down(reason) => {
                    let progress = merge_progress(&base, &pass);
                    ctl.save(&progress, &pending_notes).await?;
                    return Ok(RunEnd::Stopped(
                        progress,
                        format!("Remnawave не отвечает: {reason}"),
                    ));
                }
            }
            if writes >= pace.batch {
                writes = 0;
                let progress = merge_progress(&base, &pass);
                let notes = std::mem::take(&mut pending_notes);
                if ctl.save(&progress, &notes).await? {
                    return Ok(RunEnd::Paused(progress));
                }
                if !pace.pause.is_zero() {
                    tokio::time::sleep(pace.pause).await;
                }
            }
        }
        let progress = merge_progress(&base, &pass);
        let notes = std::mem::take(&mut pending_notes);
        if ctl.save(&progress, &notes).await? {
            return Ok(RunEnd::Paused(progress));
        }
    }
    let progress = merge_progress(&base, &pass);
    ctl.save(&progress, &pending_notes).await?;
    Ok(RunEnd::Done(progress))
}
