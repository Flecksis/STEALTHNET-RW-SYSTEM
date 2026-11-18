use super::{NewPromo, PromoCode, PromoKind, PromoListItem, Usage};
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId, OrderId, SubscriptionId, TariffId};
use sn_domain::money::{Currency, Money};
use sn_domain::paging::{Page, PageRequest};
use sn_store::{Db, StoreError, StoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedemptionStatus {
    Reserved,
    Used,
    Released,
}

impl RedemptionStatus {
    const fn as_str(self) -> &'static str {
        match self {
            RedemptionStatus::Reserved => "reserved",
            RedemptionStatus::Used => "used",
            RedemptionStatus::Released => "released",
        }
    }

    fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "reserved" => Ok(RedemptionStatus::Reserved),
            "used" => Ok(RedemptionStatus::Used),
            "released" => Ok(RedemptionStatus::Released),
            _ => Err(decode("status")),
        }
    }
}

fn state(now: &str) -> String {
    format!(
        "CASE \
           WHEN r.status = 'used' THEN 'used' \
           WHEN r.status <> 'reserved' THEN NULL \
           WHEN r.order_id IS NULL THEN CASE WHEN r.reserved_until > {now} THEN 'held' ELSE 'used' END \
           WHEN o.status IN ('paid', 'fulfilled', 'review') THEN 'used' \
           WHEN o.status <> 'pending' THEN NULL \
           WHEN r.reserved_until IS NULL OR r.reserved_until > {now} THEN 'held' \
           WHEN EXISTS (SELECT 1 FROM payments p WHERE p.order_id = r.order_id AND p.status = 'pending' \
                        AND (p.expires_at IS NULL OR p.expires_at > {now})) THEN 'held' \
         END"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFilter {
    Active,
    Disabled,
    All,
}

impl ListFilter {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.unwrap_or("all") {
            "active" => Some(ListFilter::Active),
            "disabled" => Some(ListFilter::Disabled),
            "all" => Some(ListFilter::All),
            _ => None,
        }
    }

    const fn condition(self) -> &'static str {
        match self {
            ListFilter::Active => "p.active",
            ListFilter::Disabled => "NOT p.active",
            ListFilter::All => "true",
        }
    }
}

pub struct NewRedemption {
    pub promo_id: i64,
    pub client_id: ClientId,
    pub order_id: Option<OrderId>,
    pub subscription_id: Option<SubscriptionId>,
    pub status: RedemptionStatus,
    pub reserved_until: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct CodeRow {
    id: i64,
    code: String,
    kind: String,
    percent: Option<i32>,
    amount_minor: Option<i64>,
    currency: Option<String>,
    days: Option<i32>,
    max_uses: Option<i32>,
    per_client: i32,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    first_purchase_only: bool,
    tariff_ids: Vec<i64>,
    active: bool,
    note: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ListRow {
    #[sqlx(flatten)]
    code: CodeRow,
    used: i64,
    reserved: i64,
}

const COLUMNS: &str = "p.id, p.code, p.kind, p.percent, p.amount_minor, p.currency, p.days, p.max_uses, \
                       p.per_client, p.valid_from, p.valid_until, p.first_purchase_only, p.tariff_ids, p.active, \
                       p.note, p.created_at";

fn decode(field: &str) -> StoreError {
    StoreError::Decode(format!("промокод: поле {field}"))
}

fn count(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

impl TryFrom<CodeRow> for PromoCode {
    type Error = StoreError;

    fn try_from(row: CodeRow) -> Result<Self, Self::Error> {
        let kind = match row.kind.as_str() {
            "percent" => PromoKind::Percent {
                percent: row
                    .percent
                    .and_then(|p| u8::try_from(p).ok())
                    .ok_or_else(|| decode("percent"))?,
            },
            "fixed" => PromoKind::Fixed {
                amount: Money::new(
                    row.amount_minor.ok_or_else(|| decode("amount_minor"))?,
                    Currency::parse(row.currency.as_deref().ok_or_else(|| decode("currency"))?)?,
                ),
            },
            "days" => PromoKind::Days {
                days: row
                    .days
                    .and_then(|d| u32::try_from(d).ok())
                    .ok_or_else(|| decode("days"))?,
            },
            _ => return Err(decode("kind")),
        };
        Ok(PromoCode {
            id: row.id,
            code: row.code,
            kind,
            max_uses: row.max_uses.and_then(|m| u32::try_from(m).ok()),
            per_client: u32::try_from(row.per_client).map_err(|_| decode("per_client"))?,
            valid_from: row.valid_from,
            valid_until: row.valid_until,
            first_purchase_only: row.first_purchase_only,
            tariff_ids: row.tariff_ids.into_iter().map(TariffId).collect(),
            active: row.active,
            note: row.note,
            created_at: row.created_at,
        })
    }
}

pub async fn by_code(db: &mut impl Db, code: &str, lock: bool) -> StoreResult<Option<PromoCode>> {
    let suffix = if lock { " FOR UPDATE" } else { "" };
    let row: Option<CodeRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM promo_codes p WHERE p.code = $1{suffix}"
    ))
    .bind(code)
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

pub async fn lock(db: &mut impl Db, id: i64) -> StoreResult<Option<PromoCode>> {
    let row: Option<CodeRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM promo_codes p WHERE p.id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

pub async fn usage(
    db: &mut impl Db,
    promo_id: i64,
    client_id: ClientId,
    now: DateTime<Utc>,
    exclude: &[OrderId],
) -> StoreResult<Usage> {
    let exclude: Vec<i64> = exclude.iter().map(|o| o.0).collect();
    let (total, mine, held): (i64, i64, i64) = sqlx::query_as(&format!(
        "WITH s AS (SELECT r.client_id, {} AS state FROM promo_redemptions r \
           LEFT JOIN orders o ON o.id = r.order_id \
           WHERE r.promo_id = $1 AND (r.order_id IS NULL OR NOT (r.order_id = ANY($4)))) \
         SELECT count(*) FILTER (WHERE state IS NOT NULL), \
           count(*) FILTER (WHERE state IS NOT NULL AND client_id = $2), \
           count(*) FILTER (WHERE state = 'held' AND client_id = $2) \
         FROM s",
        state("$3")
    ))
    .bind(promo_id)
    .bind(client_id.0)
    .bind(now)
    .bind(&exclude)
    .fetch_one(db.conn())
    .await?;
    let purchases: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM orders WHERE client_id = $1 \
           AND kind IN ('new', 'renewal', 'change') \
           AND status IN ('paid', 'fulfilled', 'review', 'refunded') \
           AND NOT (id = ANY($2))) \
         OR EXISTS (SELECT 1 FROM promo_redemptions r \
           JOIN promo_codes c ON c.id = r.promo_id \
           JOIN orders o ON o.id = r.order_id \
           WHERE r.client_id = $1 AND c.first_purchase_only \
           AND NOT (r.order_id = ANY($2)) AND {} = 'held')",
        state("$3")
    ))
    .bind(client_id.0)
    .bind(&exclude)
    .bind(now)
    .fetch_one(db.conn())
    .await?;
    Ok(Usage {
        active_total: count(total),
        client_active: count(mine),
        client_held: count(held),
        client_has_purchases: purchases,
    })
}

pub async fn insert_redemption(
    db: &mut impl Db,
    redemption: &NewRedemption,
) -> StoreResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "INSERT INTO promo_redemptions (promo_id, client_id, order_id, subscription_id, status, reserved_until, used_at) \
         VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $5 = 'used' THEN now() END) \
         ON CONFLICT (order_id) DO NOTHING RETURNING id",
    )
    .bind(redemption.promo_id)
    .bind(redemption.client_id.0)
    .bind(redemption.order_id.map(|o| o.0))
    .bind(redemption.subscription_id.map(|s| s.0))
    .bind(redemption.status.as_str())
    .bind(redemption.reserved_until)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn unpaid_holds(
    db: &mut impl Db,
    promo_id: i64,
    client_id: ClientId,
    now: DateTime<Utc>,
    except: Option<OrderId>,
) -> StoreResult<Vec<OrderId>> {
    let ids: Vec<i64> = sqlx::query_scalar(&format!(
        "SELECT r.order_id FROM promo_redemptions r JOIN orders o ON o.id = r.order_id \
         WHERE r.promo_id = $1 AND r.client_id = $2 AND o.status = 'pending' \
         AND ($4::bigint IS NULL OR r.order_id <> $4) AND {} = 'held' ORDER BY r.order_id",
        state("$3")
    ))
    .bind(promo_id)
    .bind(client_id.0)
    .bind(now)
    .bind(except.map(|o| o.0))
    .fetch_all(db.conn())
    .await?;
    Ok(ids.into_iter().map(OrderId).collect())
}

pub async fn paid_before(db: &mut impl Db, client_id: ClientId) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM payments WHERE client_id = $1 \
         AND status IN ('paid', 'partially_refunded', 'refunded') \
         AND provider NOT IN ('balance', 'manual') AND amount_minor > 0)",
    )
    .bind(client_id.0)
    .fetch_one(db.conn())
    .await?)
}

pub async fn paid_orders_without_use(db: &mut impl Db, limit: i64) -> StoreResult<Vec<OrderId>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT o.id FROM orders o \
         WHERE o.status IN ('paid', 'fulfilled', 'review') AND o.paid_at > now() - interval '7 days' \
         AND o.quote -> 'lines' @> '[{\"kind\": {\"type\": \"adjustment\", \"source\": {\"type\": \"promo\"}}}]' \
         AND NOT EXISTS (SELECT 1 FROM promo_redemptions r WHERE r.order_id = o.id AND r.status = 'used') \
         AND NOT EXISTS (SELECT 1 FROM payments p WHERE p.order_id = o.id AND p.status = 'refunded') \
         ORDER BY o.id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    Ok(ids.into_iter().map(OrderId).collect())
}

pub async fn redemption_for_order(db: &mut impl Db, order_id: OrderId) -> StoreResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM promo_redemptions WHERE order_id = $1")
            .bind(order_id.0)
            .fetch_optional(db.conn())
            .await?,
    )
}

pub struct RedemptionState {
    pub id: i64,
    pub status: RedemptionStatus,
    pub reserved_until: Option<DateTime<Utc>>,
}

pub async fn redemption_state(
    db: &mut impl Db,
    order_id: OrderId,
) -> StoreResult<Option<RedemptionState>> {
    let row: Option<(i64, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, status, reserved_until FROM promo_redemptions WHERE order_id = $1",
    )
    .bind(order_id.0)
    .fetch_optional(db.conn())
    .await?;
    row.map(|(id, status, reserved_until)| {
        Ok(RedemptionState {
            id,
            status: RedemptionStatus::parse(&status)?,
            reserved_until,
        })
    })
    .transpose()
}

pub async fn set_status(db: &mut impl Db, id: i64, status: RedemptionStatus) -> StoreResult<()> {
    sqlx::query(
        "UPDATE promo_redemptions SET status = $2, \
         used_at = CASE WHEN $2 = 'used' THEN now() ELSE used_at END, \
         released_at = CASE WHEN $2 = 'released' THEN now() ELSE NULL END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(status.as_str())
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn mark_order_used(db: &mut impl Db, order_id: OrderId) -> StoreResult<u64> {
    let done = sqlx::query(
        "UPDATE promo_redemptions SET status = 'used', used_at = now(), released_at = NULL \
         WHERE order_id = $1 AND status <> 'used'",
    )
    .bind(order_id.0)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected())
}

pub async fn release_order(
    db: &mut impl Db,
    order_id: OrderId,
    from: RedemptionStatus,
) -> StoreResult<u64> {
    let done = sqlx::query(
        "UPDATE promo_redemptions SET status = 'released', released_at = now() \
         WHERE order_id = $1 AND status = $2",
    )
    .bind(order_id.0)
    .bind(from.as_str())
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected())
}

pub async fn restore_order(db: &mut impl Db, order_id: OrderId) -> StoreResult<u64> {
    let done = sqlx::query(
        "UPDATE promo_redemptions SET status = 'reserved', released_at = NULL \
         WHERE order_id = $1 AND status = 'released'",
    )
    .bind(order_id.0)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected())
}

pub async fn tariffs_exist(db: &mut impl Db, ids: &[TariffId]) -> StoreResult<bool> {
    if ids.is_empty() {
        return Ok(true);
    }
    let raw: Vec<i64> = ids.iter().map(|t| t.0).collect();
    let found: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT id) FROM tariffs WHERE id = ANY($1) AND deleted_at IS NULL",
    )
    .bind(&raw)
    .fetch_one(db.conn())
    .await?;
    let mut unique = raw.clone();
    unique.sort_unstable();
    unique.dedup();
    Ok(found == unique.len() as i64)
}

pub async fn create(
    db: &mut impl Db,
    code: &str,
    kind: PromoKind,
    new: &NewPromo,
    by: Option<AdminId>,
) -> StoreResult<PromoCode> {
    insert(db, code, kind, new, by, false)
        .await?
        .ok_or(StoreError::NotFound("промокод"))
}

pub async fn create_if_free(
    db: &mut impl Db,
    code: &str,
    kind: PromoKind,
    new: &NewPromo,
) -> StoreResult<Option<PromoCode>> {
    insert(db, code, kind, new, None, true).await
}

async fn insert(
    db: &mut impl Db,
    code: &str,
    kind: PromoKind,
    new: &NewPromo,
    by: Option<AdminId>,
    skip_taken: bool,
) -> StoreResult<Option<PromoCode>> {
    let (kind_name, percent, amount, currency, days) = match kind {
        PromoKind::Percent { percent } => ("percent", Some(i32::from(percent)), None, None, None),
        PromoKind::Fixed { amount } => (
            "fixed",
            None,
            Some(amount.minor),
            Some(amount.currency.code()),
            None,
        ),
        PromoKind::Days { days } => (
            "days",
            None,
            None,
            None,
            Some(
                i32::try_from(days)
                    .map_err(|_| StoreError::Invalid("дней слишком много".into()))?,
            ),
        ),
    };
    let mut tariffs: Vec<i64> = new.tariff_ids.iter().map(|t| t.0).collect();
    tariffs.sort_unstable();
    tariffs.dedup();
    let row: Option<CodeRow> = sqlx::query_as(&format!(
        "INSERT INTO promo_codes AS p (code, kind, percent, amount_minor, currency, days, max_uses, per_client, \
         valid_from, valid_until, first_purchase_only, tariff_ids, note, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) {} RETURNING {COLUMNS}",
        if skip_taken { "ON CONFLICT (code) DO NOTHING" } else { "" }
    ))
    .bind(code)
    .bind(kind_name)
    .bind(percent)
    .bind(amount)
    .bind(currency)
    .bind(days)
    .bind(new.max_uses.map(|m| i32::try_from(m).unwrap_or(i32::MAX)))
    .bind(i32::try_from(new.per_client).unwrap_or(i32::MAX))
    .bind(new.valid_from)
    .bind(new.valid_until)
    .bind(new.first_purchase_only)
    .bind(&tariffs)
    .bind(new.note.as_deref().map(str::trim).filter(|n| !n.is_empty()))
    .bind(by.map(|a| a.0))
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

pub async fn list(
    db: &mut impl Db,
    filter: ListFilter,
    page: PageRequest,
    now: DateTime<Utc>,
) -> StoreResult<Page<PromoListItem>> {
    let state = state("$3");
    let rows: Vec<ListRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS}, \
           (SELECT count(*) FROM promo_redemptions r LEFT JOIN orders o ON o.id = r.order_id \
            WHERE r.promo_id = p.id AND {state} = 'used') AS used, \
           (SELECT count(*) FROM promo_redemptions r LEFT JOIN orders o ON o.id = r.order_id \
            WHERE r.promo_id = p.id AND {state} = 'held') AS reserved \
         FROM promo_codes p WHERE {} AND ($1::bigint IS NULL OR p.id < $1) ORDER BY p.id DESC LIMIT $2",
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
            Ok(PromoListItem {
                code: row.code.try_into()?,
                used: count(row.used),
                reserved: count(row.reserved),
            })
        })
        .collect::<StoreResult<Vec<PromoListItem>>>()?;
    Ok(Page::from_overfetch(items, page, |item| item.code.id))
}

pub async fn set_active(db: &mut impl Db, id: i64, active: bool) -> StoreResult<Option<PromoCode>> {
    let row: Option<CodeRow> = sqlx::query_as(&format!(
        "UPDATE promo_codes AS p SET active = $2, \
         disabled_at = CASE WHEN $2 THEN NULL ELSE COALESCE(p.disabled_at, now()) END \
         WHERE p.id = $1 RETURNING {COLUMNS}"
    ))
    .bind(id)
    .bind(active)
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

fn attempts(value: i32) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

pub async fn strike_client(
    db: &mut impl Db,
    client_id: ClientId,
    window_start: DateTime<Utc>,
    forget_before: DateTime<Utc>,
) -> StoreResult<u32> {
    let (failures, fresh): (i32, bool) = sqlx::query_as(
        "INSERT INTO promo_attempts (client_id, window_start, failures) VALUES ($1, $2, 1) \
         ON CONFLICT (client_id, window_start) \
         DO UPDATE SET failures = LEAST(promo_attempts.failures + 1, 1000000) \
         RETURNING failures, (xmax = 0)",
    )
    .bind(client_id.0)
    .bind(window_start)
    .fetch_one(db.conn())
    .await?;
    if fresh {
        sqlx::query(
            "DELETE FROM promo_attempts a USING ( \
                 SELECT client_id, window_start FROM promo_attempts \
                 WHERE window_start < $1 FOR UPDATE SKIP LOCKED) old \
             WHERE a.client_id = old.client_id AND a.window_start = old.window_start",
        )
        .bind(forget_before)
        .execute(db.conn())
        .await?;
    }
    Ok(attempts(failures))
}

pub async fn forgive_client(
    db: &mut impl Db,
    client_id: ClientId,
    window_start: DateTime<Utc>,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE promo_attempts SET failures = failures - 1 \
         WHERE client_id = $1 AND window_start = $2 AND failures > 0",
    )
    .bind(client_id.0)
    .bind(window_start)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn strike_address(
    db: &mut impl Db,
    address: &str,
    window_start: DateTime<Utc>,
    forget_before: DateTime<Utc>,
) -> StoreResult<u32> {
    let (failures, fresh): (i32, bool) = sqlx::query_as(
        "INSERT INTO promo_address_attempts (address, window_start, failures) VALUES ($1, $2, 1) \
         ON CONFLICT (address, window_start) \
         DO UPDATE SET failures = LEAST(promo_address_attempts.failures + 1, 1000000) \
         RETURNING failures, (xmax = 0)",
    )
    .bind(address)
    .bind(window_start)
    .fetch_one(db.conn())
    .await?;
    if fresh {
        sqlx::query(
            "DELETE FROM promo_address_attempts a USING ( \
                 SELECT address, window_start FROM promo_address_attempts \
                 WHERE window_start < $1 FOR UPDATE SKIP LOCKED) old \
             WHERE a.address = old.address AND a.window_start = old.window_start",
        )
        .bind(forget_before)
        .execute(db.conn())
        .await?;
    }
    Ok(attempts(failures))
}

pub async fn forgive_address(
    db: &mut impl Db,
    address: &str,
    window_start: DateTime<Utc>,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE promo_address_attempts SET failures = failures - 1 \
         WHERE address = $1 AND window_start = $2 AND failures > 0",
    )
    .bind(address)
    .bind(window_start)
    .execute(db.conn())
    .await?;
    Ok(())
}
