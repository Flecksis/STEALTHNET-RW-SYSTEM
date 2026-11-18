use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId, OrderId};
use sn_domain::paging::{Page, PageRequest};
use sn_store::{Db, StoreResult};
use uuid::Uuid;

pub async fn config(
    db: &mut impl Db,
) -> StoreResult<Option<(serde_json::Value, i32, DateTime<Utc>)>> {
    Ok(
        sqlx::query_as("SELECT config, version, updated_at FROM roulette_config WHERE id = 1")
            .fetch_optional(db.conn())
            .await?,
    )
}

pub async fn lock_config(db: &mut impl Db) -> StoreResult<Option<i32>> {
    Ok(
        sqlx::query_scalar("SELECT version FROM roulette_config WHERE id = 1 FOR UPDATE")
            .fetch_optional(db.conn())
            .await?,
    )
}

pub async fn save_config(
    db: &mut impl Db,
    config: &serde_json::Value,
    by: Option<AdminId>,
) -> StoreResult<i32> {
    Ok(sqlx::query_scalar(
        "INSERT INTO roulette_config (id, config, version, updated_by, updated_at) VALUES (1, $1, 1, $2, now()) \
         ON CONFLICT (id) DO UPDATE SET config = EXCLUDED.config, version = roulette_config.version + 1, \
         updated_by = EXCLUDED.updated_by, updated_at = now() RETURNING version",
    )
    .bind(config)
    .bind(by.map(|a| a.0))
    .fetch_one(db.conn())
    .await?)
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SpinRow {
    pub id: i64,
    pub client_id: i64,
    pub request_id: Uuid,
    pub source: String,
    pub config_version: i32,
    pub sector: i32,
    pub prize: serde_json::Value,
    pub label: String,
    pub state: String,
    pub subscription_id: Option<i64>,
    pub note: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub issued_at: Option<DateTime<Utc>>,
}

const SPIN_COLUMNS: &str = "s.id, s.client_id, s.request_id, s.source, s.config_version, s.sector, s.prize, s.label, \
                            s.state, s.subscription_id, s.note, s.code, s.error, s.created_at, s.issued_at";

pub async fn spin_by_request(
    db: &mut impl Db,
    client_id: ClientId,
    request_id: Uuid,
) -> StoreResult<Option<SpinRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SPIN_COLUMNS} FROM roulette_spins s WHERE s.client_id = $1 AND s.request_id = $2"
    ))
    .bind(client_id.0)
    .bind(request_id)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn spin(db: &mut impl Db, id: i64) -> StoreResult<Option<SpinRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SPIN_COLUMNS} FROM roulette_spins s WHERE s.id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn history(
    db: &mut impl Db,
    client_id: ClientId,
    limit: i64,
) -> StoreResult<Vec<SpinRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SPIN_COLUMNS} FROM roulette_spins s WHERE s.client_id = $1 ORDER BY s.id DESC LIMIT $2"
    ))
    .bind(client_id.0)
    .bind(limit)
    .fetch_all(db.conn())
    .await?)
}

pub async fn recent(db: &mut impl Db, page: PageRequest) -> StoreResult<Page<SpinRow>> {
    let rows: Vec<SpinRow> = sqlx::query_as(&format!(
        "SELECT {SPIN_COLUMNS} FROM roulette_spins s WHERE ($1::bigint IS NULL OR s.id < $1) \
         ORDER BY s.id DESC LIMIT $2"
    ))
    .bind(page.after)
    .bind(page.fetch_limit())
    .fetch_all(db.conn())
    .await?;
    Ok(Page::from_overfetch(rows, page, |r| r.id))
}

pub async fn last_free(
    db: &mut impl Db,
    client_id: ClientId,
) -> StoreResult<Option<DateTime<Utc>>> {
    Ok(sqlx::query_scalar(
        "SELECT max(created_at) FROM roulette_spins WHERE client_id = $1 AND source = 'free'",
    )
    .bind(client_id.0)
    .fetch_one(db.conn())
    .await?)
}

pub async fn open_tickets(db: &mut impl Db, client_id: ClientId) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM roulette_tickets WHERE client_id = $1 AND used_at IS NULL AND revoked_at IS NULL",
    )
    .bind(client_id.0)
    .fetch_one(db.conn())
    .await?)
}

pub async fn take_ticket(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM roulette_tickets WHERE client_id = $1 AND used_at IS NULL AND revoked_at IS NULL \
         ORDER BY id LIMIT 1 FOR UPDATE",
    )
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn use_ticket(db: &mut impl Db, id: i64, now: DateTime<Utc>) -> StoreResult<()> {
    sqlx::query("UPDATE roulette_tickets SET used_at = $2 WHERE id = $1")
        .bind(id)
        .bind(now)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn grant_ticket(
    db: &mut impl Db,
    client_id: ClientId,
    order_id: OrderId,
) -> StoreResult<bool> {
    Ok(sqlx::query(
        "INSERT INTO roulette_tickets (client_id, order_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(client_id.0)
    .bind(order_id.0)
    .execute(db.conn())
    .await?
    .rows_affected()
        > 0)
}

pub async fn revoke_ticket(db: &mut impl Db, order_id: OrderId) -> StoreResult<u64> {
    Ok(sqlx::query(
        "UPDATE roulette_tickets SET revoked_at = now() WHERE order_id = $1 AND used_at IS NULL AND revoked_at IS NULL",
    )
    .bind(order_id.0)
    .execute(db.conn())
    .await?
    .rows_affected())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct Spent {
    pub balance_minor: i64,
    pub days: i64,
    pub promo: i64,
}

pub async fn spent(db: &mut impl Db, since: DateTime<Utc>) -> StoreResult<Spent> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(sum(balance_minor), 0)::bigint AS balance_minor, COALESCE(sum(days), 0)::bigint AS days, \
         COALESCE(sum(promo), 0)::bigint AS promo FROM roulette_spins \
         WHERE created_at >= $1 AND state IN ('issuing', 'issued')",
    )
    .bind(since)
    .fetch_one(db.conn())
    .await?)
}

pub struct NewSpin<'a> {
    pub client_id: ClientId,
    pub request_id: Uuid,
    pub source: &'a str,
    pub ticket_id: Option<i64>,
    pub config_version: i32,
    pub sector: i32,
    pub prize: serde_json::Value,
    pub label: &'a str,
    pub balance_minor: i64,
    pub days: i32,
    pub promo: i32,
    pub state: &'a str,
    pub subscription_id: Option<i64>,
}

pub async fn insert_spin(db: &mut impl Db, spin: &NewSpin<'_>) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO roulette_spins (client_id, request_id, source, ticket_id, config_version, sector, prize, label, \
         balance_minor, days, promo, state, subscription_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING id",
    )
    .bind(spin.client_id.0)
    .bind(spin.request_id)
    .bind(spin.source)
    .bind(spin.ticket_id)
    .bind(spin.config_version)
    .bind(spin.sector)
    .bind(&spin.prize)
    .bind(spin.label)
    .bind(spin.balance_minor)
    .bind(spin.days)
    .bind(spin.promo)
    .bind(spin.state)
    .bind(spin.subscription_id)
    .fetch_one(db.conn())
    .await?)
}

pub async fn set_spin(
    db: &mut impl Db,
    id: i64,
    state: &str,
    note: Option<&str>,
    code: Option<&str>,
    error: Option<&str>,
    issued_at: Option<DateTime<Utc>>,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE roulette_spins SET state = $2, note = $3, code = $4, error = $5, issued_at = $6 WHERE id = $1",
    )
    .bind(id)
    .bind(state)
    .bind(note)
    .bind(code)
    .bind(error)
    .bind(issued_at)
    .execute(db.conn())
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, sqlx::FromRow)]
pub struct Tally {
    pub spins: i64,
    pub free: i64,
    pub purchase: i64,
    pub empty: i64,
    pub balance_count: i64,
    pub balance_minor: i64,
    pub days_count: i64,
    pub days: i64,
    pub promo: i64,
    pub failed: i64,
    pub clients: i64,
}

pub async fn tally(db: &mut impl Db, since: DateTime<Utc>) -> StoreResult<Tally> {
    Ok(sqlx::query_as(
        "SELECT count(*) AS spins, count(*) FILTER (WHERE source = 'free') AS free, \
         count(*) FILTER (WHERE source = 'purchase') AS purchase, count(*) FILTER (WHERE state = 'empty') AS empty, \
         count(*) FILTER (WHERE balance_minor > 0 AND state IN ('issuing', 'issued')) AS balance_count, \
         COALESCE(sum(balance_minor) FILTER (WHERE state IN ('issuing', 'issued')), 0)::bigint AS balance_minor, \
         count(*) FILTER (WHERE days > 0 AND state IN ('issuing', 'issued')) AS days_count, \
         COALESCE(sum(days) FILTER (WHERE state IN ('issuing', 'issued')), 0)::bigint AS days, \
         COALESCE(sum(promo) FILTER (WHERE state IN ('issuing', 'issued')), 0)::bigint AS promo, \
         count(*) FILTER (WHERE state = 'failed') AS failed, count(DISTINCT client_id) AS clients \
         FROM roulette_spins WHERE created_at >= $1",
    )
    .bind(since)
    .fetch_one(db.conn())
    .await?)
}

pub async fn tickets_open(db: &mut impl Db) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM roulette_tickets WHERE used_at IS NULL AND revoked_at IS NULL",
    )
    .fetch_one(db.conn())
    .await?)
}

pub async fn client_state(
    db: &mut impl Db,
    client_id: ClientId,
) -> StoreResult<Option<(String, bool)>> {
    Ok(
        sqlx::query_as("SELECT status, deleted_at IS NOT NULL FROM clients WHERE id = $1")
            .bind(client_id.0)
            .fetch_optional(db.conn())
            .await?,
    )
}

pub async fn eligible_free(
    db: &mut impl Db,
    client_id: ClientId,
    now: DateTime<Utc>,
    paid_only: bool,
) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM subscriptions s WHERE s.client_id = $1 AND s.remna_user_id IS NOT NULL \
         AND s.ended_at IS NULL AND s.expire_at > $2 AND s.remna_missing_at IS NULL \
         AND (s.status IS NULL OR s.status <> 'disabled') AND (NOT $3 OR s.kind <> 'trial'))",
    )
    .bind(client_id.0)
    .bind(now)
    .bind(paid_only)
    .fetch_one(db.conn())
    .await?)
}
