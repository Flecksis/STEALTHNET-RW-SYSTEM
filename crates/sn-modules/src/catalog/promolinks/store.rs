use super::{LinkState, PromoLink};
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId, OrderId, SquadId};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::traffic::TrafficReset;
use sn_store::{Db, StoreError, StoreResult};
use uuid::Uuid;

const COLUMNS: &str = "l.id, l.code, l.name, l.squads, l.days, l.traffic_limit_bytes, l.traffic_reset, \
    l.devices, l.max_activations, l.activations, l.expires_at, l.enabled, l.created_at, l.disabled_at";

const PURCHASED: &str = "(SELECT count(*) FROM promo_link_activations a WHERE a.link_id = l.id AND EXISTS ( \
    SELECT 1 FROM orders o WHERE o.client_id = a.client_id AND o.kind IN ('new', 'renewal', 'change') \
    AND o.status IN ('paid', 'fulfilled', 'review') AND o.paid_at > a.activated_at))";

#[derive(sqlx::FromRow)]
pub struct LinkRow {
    id: i64,
    code: String,
    name: String,
    squads: Vec<Uuid>,
    days: i32,
    traffic_limit_bytes: Option<i64>,
    traffic_reset: String,
    devices: i32,
    max_activations: Option<i32>,
    activations: i32,
    expires_at: Option<DateTime<Utc>>,
    enabled: bool,
    created_at: DateTime<Utc>,
    disabled_at: Option<DateTime<Utc>>,
}

impl TryFrom<LinkRow> for PromoLink {
    type Error = StoreError;

    fn try_from(row: LinkRow) -> StoreResult<PromoLink> {
        Ok(PromoLink {
            id: row.id,
            code: row.code,
            name: row.name,
            squads: row.squads.into_iter().map(SquadId).collect(),
            days: u32::try_from(row.days).unwrap_or(0),
            traffic_limit_bytes: row.traffic_limit_bytes.and_then(|b| u64::try_from(b).ok()),
            traffic_reset: TrafficReset::parse(&row.traffic_reset)
                .map_err(|e| StoreError::Invalid(e.to_string()))?,
            devices: u32::try_from(row.devices).unwrap_or(0),
            max_activations: row.max_activations.and_then(|m| u32::try_from(m).ok()),
            activations: u32::try_from(row.activations).unwrap_or(0),
            expires_at: row.expires_at,
            enabled: row.enabled,
            created_at: row.created_at,
            disabled_at: row.disabled_at,
        })
    }
}

pub struct NewLinkRow<'a> {
    pub code: &'a str,
    pub name: &'a str,
    pub squads: &'a [SquadId],
    pub days: u32,
    pub traffic_limit_bytes: Option<u64>,
    pub traffic_reset: TrafficReset,
    pub devices: u32,
    pub max_activations: Option<u32>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<AdminId>,
}

pub async fn insert(db: &mut impl Db, link: &NewLinkRow<'_>) -> StoreResult<PromoLink> {
    let squads: Vec<Uuid> = link.squads.iter().map(|s| s.0).collect();
    let row: LinkRow = sqlx::query_as(&format!(
        "INSERT INTO promo_links AS l (code, name, squads, days, traffic_limit_bytes, traffic_reset, devices, \
         max_activations, expires_at, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         RETURNING {COLUMNS}"
    ))
    .bind(link.code)
    .bind(link.name)
    .bind(&squads)
    .bind(i32::try_from(link.days).unwrap_or(i32::MAX))
    .bind(link.traffic_limit_bytes.map(|b| i64::try_from(b).unwrap_or(i64::MAX)))
    .bind(link.traffic_reset.as_str())
    .bind(i32::try_from(link.devices).unwrap_or(i32::MAX))
    .bind(link.max_activations.map(|m| i32::try_from(m).unwrap_or(i32::MAX)))
    .bind(link.expires_at)
    .bind(link.created_by.map(|a| a.0))
    .fetch_one(db.conn())
    .await?;
    row.try_into()
}

pub async fn get(db: &mut impl Db, id: i64) -> StoreResult<Option<PromoLink>> {
    let row: Option<LinkRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM promo_links l WHERE l.id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

pub async fn by_code(db: &mut impl Db, code: &str) -> StoreResult<Option<PromoLink>> {
    select_by_code(db, code, "").await
}

pub async fn lock_by_code(db: &mut impl Db, code: &str) -> StoreResult<Option<PromoLink>> {
    select_by_code(db, code, " FOR UPDATE").await
}

async fn select_by_code(
    db: &mut impl Db,
    code: &str,
    lock: &str,
) -> StoreResult<Option<PromoLink>> {
    let row: Option<LinkRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM promo_links l WHERE l.code = $1{lock}"
    ))
    .bind(code)
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

pub async fn set_enabled(
    db: &mut impl Db,
    id: i64,
    enabled: bool,
) -> StoreResult<Option<PromoLink>> {
    let row: Option<LinkRow> = sqlx::query_as(&format!(
        "UPDATE promo_links AS l SET enabled = $2, \
         disabled_at = CASE WHEN $2 THEN NULL ELSE COALESCE(l.disabled_at, now()) END \
         WHERE l.id = $1 RETURNING {COLUMNS}"
    ))
    .bind(id)
    .bind(enabled)
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

pub async fn activated_by(
    db: &mut impl Db,
    link_id: i64,
    client_id: ClientId,
) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM promo_link_activations WHERE link_id = $1 AND client_id = $2)",
    )
    .bind(link_id)
    .bind(client_id.0)
    .fetch_one(db.conn())
    .await?)
}

pub async fn record_activation(
    db: &mut impl Db,
    link_id: i64,
    client_id: ClientId,
    order_id: OrderId,
) -> StoreResult<()> {
    sqlx::query(
        "INSERT INTO promo_link_activations (link_id, client_id, order_id) VALUES ($1, $2, $3)",
    )
    .bind(link_id)
    .bind(client_id.0)
    .bind(order_id.0)
    .execute(db.conn())
    .await?;
    sqlx::query("UPDATE promo_links SET activations = activations + 1 WHERE id = $1")
        .bind(link_id)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn purchased(db: &mut impl Db, id: i64) -> StoreResult<u32> {
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT {PURCHASED} FROM promo_links l WHERE l.id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .unwrap_or(0);
    Ok(u32::try_from(count).unwrap_or(0))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFilter {
    All,
    Active,
    Off,
}

impl ListFilter {
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        Some(match raw.map(str::trim).unwrap_or("all") {
            "" | "all" => ListFilter::All,
            "active" => ListFilter::Active,
            "off" => ListFilter::Off,
            _ => return None,
        })
    }

    fn condition(self) -> &'static str {
        match self {
            ListFilter::All => "true",
            ListFilter::Active => {
                "l.enabled AND (l.expires_at IS NULL OR l.expires_at > $3) \
                AND (l.max_activations IS NULL OR l.activations < l.max_activations)"
            }
            ListFilter::Off => {
                "NOT (l.enabled AND (l.expires_at IS NULL OR l.expires_at > $3) \
                AND (l.max_activations IS NULL OR l.activations < l.max_activations))"
            }
        }
    }
}

#[derive(sqlx::FromRow)]
struct ListRow {
    #[sqlx(flatten)]
    link: LinkRow,
    purchased: i64,
}

pub async fn list(
    db: &mut impl Db,
    filter: ListFilter,
    page: PageRequest,
    now: DateTime<Utc>,
) -> StoreResult<Page<(PromoLink, u32)>> {
    let rows: Vec<ListRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS}, {PURCHASED} AS purchased FROM promo_links l \
         WHERE {} AND ($1::bigint IS NULL OR l.id < $1) ORDER BY l.id DESC LIMIT $2",
        filter.condition()
    ))
    .bind(page.after)
    .bind(page.fetch_limit())
    .bind(now)
    .fetch_all(db.conn())
    .await?;
    let items = rows
        .into_iter()
        .map(|row| {
            let link: PromoLink = row.link.try_into()?;
            Ok((link, u32::try_from(row.purchased).unwrap_or(0)))
        })
        .collect::<StoreResult<Vec<_>>>()?;
    Ok(Page::from_overfetch(items, page, |(link, _)| link.id))
}

pub fn state(link: &PromoLink, now: DateTime<Utc>) -> LinkState {
    if !link.enabled {
        LinkState::Disabled
    } else if link.expires_at.is_some_and(|at| at <= now) {
        LinkState::Expired
    } else if link
        .max_activations
        .is_some_and(|max| link.activations >= max)
    {
        LinkState::Exhausted
    } else {
        LinkState::Active
    }
}
