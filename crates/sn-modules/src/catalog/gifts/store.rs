use super::{Gift, GiftStatus};
use chrono::{DateTime, Utc};
use sn_domain::ids::{ClientId, OrderId, PeriodId, TariffId};
use sn_domain::paging::{Page, PageRequest};
use sn_store::{ClientPerson, Db, StoreError, StoreResult};
use uuid::Uuid;

pub const COLUMNS: &str = "g.id, g.public_id, g.code, g.order_id, g.giver_id, g.tariff_id, g.period_id, \
    g.days, g.tariff_name, g.status, g.expires_at, g.redeemed_by, g.redeemed_at, g.grant_order_id, \
    g.canceled_at, g.created_at";

#[derive(sqlx::FromRow)]
pub struct GiftRow {
    id: i64,
    public_id: Uuid,
    code: String,
    order_id: i64,
    giver_id: i64,
    tariff_id: i64,
    period_id: i64,
    days: i32,
    tariff_name: String,
    status: String,
    expires_at: Option<DateTime<Utc>>,
    redeemed_by: Option<i64>,
    redeemed_at: Option<DateTime<Utc>>,
    grant_order_id: Option<i64>,
    canceled_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl GiftRow {
    pub fn into_gift(self, now: DateTime<Utc>) -> StoreResult<Gift> {
        let stored = match self.status.as_str() {
            "active" => GiftStatus::Active,
            "redeemed" => GiftStatus::Redeemed,
            "canceled" => GiftStatus::Canceled,
            other => {
                return Err(StoreError::Invalid(format!(
                    "неизвестное состояние подарка {other}"
                )));
            }
        };
        let status = match (stored, self.expires_at) {
            (GiftStatus::Active, Some(at)) if at <= now => GiftStatus::Expired,
            (status, _) => status,
        };
        Ok(Gift {
            id: self.id,
            public_id: self.public_id,
            code: self.code,
            order_id: OrderId(self.order_id),
            giver_id: ClientId(self.giver_id),
            tariff_id: TariffId(self.tariff_id),
            period_id: PeriodId(self.period_id),
            days: u32::try_from(self.days).unwrap_or(0),
            tariff_name: self.tariff_name,
            status,
            expires_at: self.expires_at,
            redeemed_by: self.redeemed_by.map(ClientId),
            redeemed_at: self.redeemed_at,
            grant_order_id: self.grant_order_id.map(OrderId),
            canceled_at: self.canceled_at,
            created_at: self.created_at,
        })
    }
}

pub struct NewGift<'a> {
    pub code: &'a str,
    pub order_id: OrderId,
    pub giver_id: ClientId,
    pub tariff_id: TariffId,
    pub period_id: PeriodId,
    pub days: u32,
    pub tariff_name: &'a str,
    pub expires_at: Option<DateTime<Utc>>,
}

pub async fn insert(db: &mut impl Db, gift: &NewGift<'_>) -> StoreResult<Option<i64>> {
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO gifts (code, order_id, giver_id, tariff_id, period_id, days, tariff_name, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (order_id) DO NOTHING RETURNING id",
    )
    .bind(gift.code)
    .bind(gift.order_id.0)
    .bind(gift.giver_id.0)
    .bind(gift.tariff_id.0)
    .bind(gift.period_id.0)
    .bind(i32::try_from(gift.days).unwrap_or(i32::MAX))
    .bind(gift.tariff_name)
    .bind(gift.expires_at)
    .fetch_optional(db.conn())
    .await?;
    Ok(id)
}

pub async fn by_order(
    db: &mut impl Db,
    order_id: OrderId,
    now: DateTime<Utc>,
) -> StoreResult<Option<Gift>> {
    let row: Option<GiftRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM gifts g WHERE g.order_id = $1"
    ))
    .bind(order_id.0)
    .fetch_optional(db.conn())
    .await?;
    row.map(|r| r.into_gift(now)).transpose()
}

pub async fn by_code(
    db: &mut impl Db,
    code: &str,
    now: DateTime<Utc>,
) -> StoreResult<Option<Gift>> {
    let row: Option<GiftRow> =
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM gifts g WHERE g.code = $1"))
            .bind(code)
            .fetch_optional(db.conn())
            .await?;
    row.map(|r| r.into_gift(now)).transpose()
}

pub async fn lock_by_code(
    db: &mut impl Db,
    code: &str,
    now: DateTime<Utc>,
) -> StoreResult<Option<Gift>> {
    let row: Option<GiftRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM gifts g WHERE g.code = $1 FOR UPDATE"
    ))
    .bind(code)
    .fetch_optional(db.conn())
    .await?;
    row.map(|r| r.into_gift(now)).transpose()
}

pub async fn order_state(db: &mut impl Db, order_id: OrderId) -> StoreResult<(String, bool)> {
    let status: String = sqlx::query_scalar("SELECT status FROM orders WHERE id = $1 FOR SHARE")
        .bind(order_id.0)
        .fetch_one(db.conn())
        .await?;
    let pending = db.orders().refund_pending(order_id).await?;
    Ok((status, pending))
}

pub async fn mark_redeemed(
    db: &mut impl Db,
    id: i64,
    client_id: ClientId,
    grant_order_id: OrderId,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE gifts SET status = 'redeemed', redeemed_by = $2, redeemed_at = now(), grant_order_id = $3 \
         WHERE id = $1 AND status = 'active'",
    )
    .bind(id)
    .bind(client_id.0)
    .bind(grant_order_id.0)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn cancel_refunded(db: &mut impl Db, order_id: OrderId) -> StoreResult<bool> {
    let done = sqlx::query(
        "UPDATE gifts g SET status = 'canceled', canceled_at = now() \
         WHERE g.order_id = $1 AND g.status = 'active' \
           AND EXISTS (SELECT 1 FROM orders o WHERE o.id = g.order_id AND o.status = 'refunded')",
    )
    .bind(order_id.0)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn by_giver(
    db: &mut impl Db,
    giver: ClientId,
    limit: i64,
    now: DateTime<Utc>,
) -> StoreResult<Vec<Gift>> {
    let rows: Vec<GiftRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM gifts g WHERE g.giver_id = $1 ORDER BY g.id DESC LIMIT $2"
    ))
    .bind(giver.0)
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    rows.into_iter().map(|r| r.into_gift(now)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFilter {
    All,
    Active,
    Redeemed,
    Expired,
    Canceled,
}

impl ListFilter {
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        Some(match raw.map(str::trim).unwrap_or("all") {
            "" | "all" => ListFilter::All,
            "active" => ListFilter::Active,
            "redeemed" => ListFilter::Redeemed,
            "expired" => ListFilter::Expired,
            "canceled" => ListFilter::Canceled,
            _ => return None,
        })
    }

    fn condition(self) -> &'static str {
        match self {
            ListFilter::All => "true",
            ListFilter::Active => {
                "g.status = 'active' AND (g.expires_at IS NULL OR g.expires_at > $4)"
            }
            ListFilter::Redeemed => "g.status = 'redeemed'",
            ListFilter::Expired => "g.status = 'active' AND g.expires_at <= $4",
            ListFilter::Canceled => "g.status = 'canceled'",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AdminGift {
    #[serde(flatten)]
    pub gift: Gift,
    pub giver: Option<ClientPerson>,
    pub recipient: Option<ClientPerson>,
    pub paid: Option<sn_domain::money::Amount>,
    pub refunded: bool,
}

#[derive(sqlx::FromRow)]
struct AdminRow {
    #[sqlx(flatten)]
    gift: GiftRow,
    paid_minor: Option<i64>,
    paid_currency: Option<String>,
    order_status: String,
}

pub async fn list(
    db: &mut impl Db,
    filter: ListFilter,
    search: Option<&str>,
    page: PageRequest,
    now: DateTime<Utc>,
) -> StoreResult<Page<AdminGift>> {
    let pattern = search
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s.trim_start_matches('@').replace(['%', '_'], "")));
    let rows: Vec<AdminRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS}, p.amount_minor AS paid_minor, p.currency AS paid_currency, o.status AS order_status \
         FROM gifts g JOIN orders o ON o.id = g.order_id \
         LEFT JOIN LATERAL (SELECT amount_minor, currency FROM payments WHERE order_id = g.order_id \
             AND status IN ('paid', 'refunded', 'partially_refunded') ORDER BY id DESC LIMIT 1) p ON true \
         WHERE {} AND ($1::bigint IS NULL OR g.id < $1) \
           AND ($3::text IS NULL OR g.code ILIKE $3 OR g.tariff_name ILIKE $3 OR EXISTS ( \
               SELECT 1 FROM client_identities i WHERE i.client_id IN (g.giver_id, g.redeemed_by) \
               AND (i.value ILIKE $3 OR i.meta ->> 'username' ILIKE $3 \
                    OR concat_ws(' ', i.meta ->> 'first_name', i.meta ->> 'last_name') ILIKE $3))) \
         ORDER BY g.id DESC LIMIT $2",
        filter.condition()
    ))
    .bind(page.after)
    .bind(page.fetch_limit())
    .bind(pattern)
    .bind(now)
    .fetch_all(db.conn())
    .await?;
    let mut ids: Vec<ClientId> = Vec::new();
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let paid = match (row.paid_minor, row.paid_currency.as_deref()) {
            (Some(minor), Some(code)) => Some(sn_domain::money::Amount::new(
                minor,
                sn_domain::money::PayCurrency::parse(code)
                    .map_err(|e| StoreError::Invalid(e.to_string()))?,
            )),
            _ => None,
        };
        let refunded = row.order_status == "refunded";
        let gift = row.gift.into_gift(now)?;
        ids.push(gift.giver_id);
        if let Some(id) = gift.redeemed_by {
            ids.push(id);
        }
        items.push(AdminGift {
            gift,
            giver: None,
            recipient: None,
            paid,
            refunded,
        });
    }
    ids.sort_by_key(|id| id.0);
    ids.dedup();
    let people = db.clients().people(&ids).await?;
    for item in &mut items {
        item.giver = people.get(&item.gift.giver_id).cloned();
        item.recipient = item
            .gift
            .redeemed_by
            .and_then(|id| people.get(&id).cloned());
    }
    Ok(Page::from_overfetch(items, page, |item| item.gift.id))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    pub active: i64,
    pub redeemed: i64,
    pub expired: i64,
    pub canceled: i64,
}

pub async fn counts(db: &mut impl Db, now: DateTime<Utc>) -> StoreResult<Counts> {
    let row: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'active' AND (expires_at IS NULL OR expires_at > $1)), \
                count(*) FILTER (WHERE status = 'redeemed'), \
                count(*) FILTER (WHERE status = 'active' AND expires_at <= $1), \
                count(*) FILTER (WHERE status = 'canceled') \
         FROM gifts",
    )
    .bind(now)
    .fetch_one(db.conn())
    .await?;
    Ok(Counts {
        active: row.0,
        redeemed: row.1,
        expired: row.2,
        canceled: row.3,
    })
}
