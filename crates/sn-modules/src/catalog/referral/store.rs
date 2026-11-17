use super::PayoutMethod;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::ids::{AdminId, ClientId, PaymentId};
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{PaymentStatus, ProviderId};
use sn_store::{Db, StoreError, StoreResult};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalStatus {
    Holding,
    Pending,
    Paid,
    Rejected,
    Canceled,
}

impl WithdrawalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            WithdrawalStatus::Holding => "holding",
            WithdrawalStatus::Pending => "pending",
            WithdrawalStatus::Paid => "paid",
            WithdrawalStatus::Rejected => "rejected",
            WithdrawalStatus::Canceled => "canceled",
        }
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            WithdrawalStatus::Holding | WithdrawalStatus::Pending => "На рассмотрении",
            WithdrawalStatus::Paid => "Выплачено",
            WithdrawalStatus::Rejected => "Отклонено",
            WithdrawalStatus::Canceled => "Отменено",
        }
    }

    fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "holding" => Ok(WithdrawalStatus::Holding),
            "pending" => Ok(WithdrawalStatus::Pending),
            "paid" => Ok(WithdrawalStatus::Paid),
            "rejected" => Ok(WithdrawalStatus::Rejected),
            "canceled" => Ok(WithdrawalStatus::Canceled),
            other => Err(StoreError::Decode(format!(
                "статус заявки на вывод: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalFilter {
    Status(WithdrawalStatus),
    All,
}

impl WithdrawalFilter {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.unwrap_or("pending") {
            "all" => Some(WithdrawalFilter::All),
            "pending" => Some(WithdrawalFilter::Status(WithdrawalStatus::Pending)),
            "paid" => Some(WithdrawalFilter::Status(WithdrawalStatus::Paid)),
            "rejected" => Some(WithdrawalFilter::Status(WithdrawalStatus::Rejected)),
            "canceled" => Some(WithdrawalFilter::Status(WithdrawalStatus::Canceled)),
            _ => None,
        }
    }

    const fn status(self) -> Option<&'static str> {
        match self {
            WithdrawalFilter::Status(status) => Some(status.as_str()),
            WithdrawalFilter::All => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Withdrawal {
    pub id: i64,
    pub public_id: Uuid,
    pub client_id: ClientId,
    pub amount: Money,
    pub method: PayoutMethod,
    pub details: String,
    pub status: WithdrawalStatus,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct WithdrawalRow {
    id: i64,
    public_id: Uuid,
    client_id: i64,
    amount_minor: i64,
    currency: String,
    method: String,
    details: String,
    status: String,
    comment: Option<String>,
    created_at: DateTime<Utc>,
    decided_at: Option<DateTime<Utc>>,
}

const W_COLUMNS: &str = "id, public_id, client_id, amount_minor, currency, method, details, status, comment, \
                         created_at, decided_at";

impl TryFrom<WithdrawalRow> for Withdrawal {
    type Error = StoreError;

    fn try_from(row: WithdrawalRow) -> Result<Self, Self::Error> {
        Ok(Withdrawal {
            id: row.id,
            public_id: row.public_id,
            client_id: ClientId(row.client_id),
            amount: Money::new(row.amount_minor, Currency::parse(&row.currency)?),
            method: PayoutMethod::parse(&row.method)
                .ok_or_else(|| StoreError::Decode(format!("способ вывода: {}", row.method)))?,
            details: row.details,
            status: WithdrawalStatus::parse(&row.status)?,
            comment: row.comment,
            created_at: row.created_at,
            decided_at: row.decided_at,
        })
    }
}

fn rows(rows: Vec<WithdrawalRow>) -> StoreResult<Vec<Withdrawal>> {
    rows.into_iter().map(TryInto::try_into).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ancestor {
    pub level: i32,
    pub referrer_id: ClientId,
    pub personal_percent: Option<u8>,
    pub blocked: bool,
}

pub struct NewCredit {
    pub payment_id: PaymentId,
    pub level: i32,
    pub referrer_id: ClientId,
    pub referral_id: ClientId,
    pub base: Money,
    pub percent: u8,
    pub amount: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    pub id: i64,
    pub payment_id: PaymentId,
    pub level: i32,
    pub referrer_id: ClientId,
    pub base_minor: i64,
    pub amount: Money,
    pub credited_at: Option<DateTime<Utc>>,
    pub reversal_target_minor: i64,
    pub reversed_minor: i64,
}

#[derive(sqlx::FromRow)]
struct CreditRow {
    id: i64,
    payment_id: i64,
    level: i32,
    referrer_id: i64,
    base_minor: i64,
    amount_minor: i64,
    currency: String,
    credited_at: Option<DateTime<Utc>>,
    reversal_target_minor: i64,
    reversed_minor: i64,
}

const C_COLUMNS: &str = "id, payment_id, level, referrer_id, base_minor, amount_minor, currency, credited_at, \
                         reversal_target_minor, reversed_minor";

impl TryFrom<CreditRow> for Credit {
    type Error = StoreError;

    fn try_from(row: CreditRow) -> Result<Self, Self::Error> {
        Ok(Credit {
            id: row.id,
            payment_id: PaymentId(row.payment_id),
            level: row.level,
            referrer_id: ClientId(row.referrer_id),
            base_minor: row.base_minor,
            amount: Money::new(row.amount_minor, Currency::parse(&row.currency)?),
            credited_at: row.credited_at,
            reversal_target_minor: row.reversal_target_minor,
            reversed_minor: row.reversed_minor,
        })
    }
}

fn count(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

pub async fn code_of(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT code FROM referral_codes WHERE client_id = $1")
            .bind(client_id.0)
            .fetch_optional(db.conn())
            .await?,
    )
}

pub async fn insert_code(
    db: &mut impl Db,
    client_id: ClientId,
    code: &str,
) -> StoreResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "INSERT INTO referral_codes (client_id, code) VALUES ($1, $2) ON CONFLICT DO NOTHING RETURNING code",
    )
    .bind(client_id.0)
    .bind(code)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn owner_of_code(db: &mut impl Db, code: &str) -> StoreResult<Option<ClientId>> {
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT r.client_id FROM referral_codes r JOIN clients c ON c.id = r.client_id \
         WHERE r.code = $1 AND c.deleted_at IS NULL",
    )
    .bind(code)
    .fetch_optional(db.conn())
    .await?;
    Ok(id.map(ClientId))
}

pub async fn personal_percent(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<u8>> {
    let percent: Option<Option<i32>> =
        sqlx::query_scalar("SELECT percent FROM referral_codes WHERE client_id = $1")
            .bind(client_id.0)
            .fetch_optional(db.conn())
            .await?;
    Ok(percent.flatten().and_then(|p| u8::try_from(p).ok()))
}

pub async fn set_personal_percent(
    db: &mut impl Db,
    client_id: ClientId,
    percent: Option<u8>,
) -> StoreResult<()> {
    sqlx::query("UPDATE referral_codes SET percent = $2 WHERE client_id = $1")
        .bind(client_id.0)
        .bind(percent.map(i32::from))
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn ancestors(
    db: &mut impl Db,
    client_id: ClientId,
    depth: i32,
) -> StoreResult<Vec<Ancestor>> {
    let rows: Vec<(i32, i64, Option<i32>, bool)> = sqlx::query_as(
        "WITH RECURSIVE chain (level, referrer_id) AS ( \
            SELECT 1, l.referrer_id FROM referral_links l WHERE l.client_id = $1 \
            UNION ALL \
            SELECT c.level + 1, l.referrer_id FROM chain c JOIN referral_links l ON l.client_id = c.referrer_id \
            WHERE c.level < $2 \
         ) \
         SELECT c.level, c.referrer_id, rc.percent, (cl.status = 'blocked' OR cl.deleted_at IS NOT NULL) \
         FROM chain c JOIN clients cl ON cl.id = c.referrer_id \
         LEFT JOIN referral_codes rc ON rc.client_id = c.referrer_id ORDER BY c.level",
    )
    .bind(client_id.0)
    .bind(depth)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(level, referrer, percent, blocked)| Ancestor {
            level,
            referrer_id: ClientId(referrer),
            personal_percent: percent.and_then(|p| u8::try_from(p).ok()),
            blocked,
        })
        .collect())
}

pub async fn link(
    db: &mut impl Db,
    client_id: ClientId,
    referrer_id: ClientId,
) -> StoreResult<bool> {
    let done = sqlx::query(
        "INSERT INTO referral_links (client_id, referrer_id) VALUES ($1, $2) ON CONFLICT (client_id) DO NOTHING",
    )
    .bind(client_id.0)
    .bind(referrer_id.0)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn referrer_of(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<ClientId>> {
    let id: Option<i64> =
        sqlx::query_scalar("SELECT referrer_id FROM referral_links WHERE client_id = $1")
            .bind(client_id.0)
            .fetch_optional(db.conn())
            .await?;
    Ok(id.map(ClientId))
}

pub async fn invited_count(db: &mut impl Db, client_id: ClientId) -> StoreResult<u32> {
    let invited: i64 =
        sqlx::query_scalar("SELECT count(*) FROM referral_links WHERE referrer_id = $1")
            .bind(client_id.0)
            .fetch_one(db.conn())
            .await?;
    Ok(count(invited))
}

pub async fn earned(
    db: &mut impl Db,
    client_id: ClientId,
    currency: Currency,
) -> StoreResult<Money> {
    let total: Option<i64> = sqlx::query_scalar(
        "SELECT sum(amount_minor - reversed_minor)::bigint FROM referral_credits \
         WHERE referrer_id = $1 AND currency = $2 AND credited_at IS NOT NULL",
    )
    .bind(client_id.0)
    .bind(currency.code())
    .fetch_one(db.conn())
    .await?;
    Ok(Money::new(total.unwrap_or(0), currency))
}

pub async fn mark_processed(db: &mut impl Db, payment_id: PaymentId) -> StoreResult<bool> {
    let done = sqlx::query(
        "INSERT INTO referral_payments (payment_id) VALUES ($1) ON CONFLICT (payment_id) DO NOTHING",
    )
    .bind(payment_id.0)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnprocessedPayment {
    pub payment_id: PaymentId,
    pub client_id: ClientId,
    pub amount: Amount,
    pub provider: ProviderId,
    pub status: PaymentStatus,
}

pub async fn unprocessed_payments(
    db: &mut impl Db,
    limit: i64,
) -> StoreResult<Vec<UnprocessedPayment>> {
    let rows: Vec<(i64, i64, i64, String, String, String)> = sqlx::query_as(
        "SELECT p.id, p.client_id, p.amount_minor, p.currency, p.provider, p.status FROM payments p \
         WHERE p.status IN ('paid', 'partially_refunded', 'refunded') AND p.paid_at > now() - interval '7 days' \
         AND p.provider NOT IN ('balance', 'manual', 'stars') AND p.currency IN ('USD', 'RUB') \
         AND EXISTS (SELECT 1 FROM referral_links l WHERE l.client_id = p.client_id) \
         AND NOT EXISTS (SELECT 1 FROM referral_payments r WHERE r.payment_id = p.id) \
         ORDER BY p.id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    rows.into_iter()
        .map(|(id, client, minor, currency, provider, status)| {
            Ok(UnprocessedPayment {
                payment_id: PaymentId(id),
                client_id: ClientId(client),
                amount: Amount::new(minor, PayCurrency::parse(&currency)?),
                provider: ProviderId(provider),
                status: PaymentStatus::parse(&status)?,
            })
        })
        .collect()
}

pub async fn insert_credit(db: &mut impl Db, credit: &NewCredit) -> StoreResult<bool> {
    let done = sqlx::query(
        "INSERT INTO referral_credits (payment_id, level, referrer_id, referral_id, base_minor, percent, amount_minor, currency) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (payment_id, level) DO NOTHING",
    )
    .bind(credit.payment_id.0)
    .bind(credit.level)
    .bind(credit.referrer_id.0)
    .bind(credit.referral_id.0)
    .bind(credit.base.minor)
    .bind(i32::from(credit.percent))
    .bind(credit.amount.minor)
    .bind(credit.amount.currency.code())
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn credits_for_payment(
    db: &mut impl Db,
    payment_id: PaymentId,
) -> StoreResult<Vec<Credit>> {
    let rows: Vec<CreditRow> = sqlx::query_as(&format!(
        "SELECT {C_COLUMNS} FROM referral_credits WHERE payment_id = $1 ORDER BY level"
    ))
    .bind(payment_id.0)
    .fetch_all(db.conn())
    .await?;
    rows.into_iter().map(TryInto::try_into).collect()
}

pub async fn set_reversal_target(db: &mut impl Db, id: i64, target: i64) -> StoreResult<()> {
    sqlx::query(
        "UPDATE referral_credits SET reversal_target_minor = GREATEST(reversal_target_minor, LEAST($2, amount_minor)) \
         WHERE id = $1",
    )
    .bind(id)
    .bind(target)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn unsettled_for_payment(
    db: &mut impl Db,
    payment_id: PaymentId,
) -> StoreResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM referral_credits WHERE payment_id = $1 \
         AND (credited_at IS NULL OR reversed_minor < reversal_target_minor) ORDER BY level",
    )
    .bind(payment_id.0)
    .fetch_all(db.conn())
    .await?)
}

pub async fn unsettled_credits(db: &mut impl Db, limit: i64) -> StoreResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM referral_credits WHERE credited_at IS NULL OR reversed_minor < reversal_target_minor \
         ORDER BY id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db.conn())
    .await?)
}

pub async fn credit(db: &mut impl Db, id: i64) -> StoreResult<Option<Credit>> {
    let row: Option<CreditRow> = sqlx::query_as(&format!(
        "SELECT {C_COLUMNS} FROM referral_credits WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

pub async fn mark_credited(db: &mut impl Db, id: i64) -> StoreResult<bool> {
    let done = sqlx::query(
        "UPDATE referral_credits SET credited_at = now() WHERE id = $1 AND credited_at IS NULL",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn advance_reversed(db: &mut impl Db, id: i64, from: i64, to: i64) -> StoreResult<bool> {
    let done = sqlx::query(
        "UPDATE referral_credits SET reversed_minor = $3 WHERE id = $1 AND reversed_minor = $2",
    )
    .bind(id)
    .bind(from)
    .bind(to)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn reversal_applied(db: &mut impl Db, key: &str) -> StoreResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT -delta_minor FROM balance_entries WHERE idempotency_key = $1 AND delta_minor < 0",
    )
    .bind(key)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn refunded_minor(db: &mut impl Db, payment_id: PaymentId) -> StoreResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT refunded_minor FROM payments WHERE id = $1")
            .bind(payment_id.0)
            .fetch_optional(db.conn())
            .await?
            .unwrap_or(0),
    )
}

pub async fn insert_withdrawal(
    db: &mut impl Db,
    client_id: ClientId,
    amount: Money,
    method: PayoutMethod,
    details: &str,
) -> StoreResult<Withdrawal> {
    let row: WithdrawalRow = sqlx::query_as(&format!(
        "INSERT INTO referral_withdrawals (client_id, amount_minor, currency, method, details) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {W_COLUMNS}"
    ))
    .bind(client_id.0)
    .bind(amount.minor)
    .bind(amount.currency.code())
    .bind(method.as_str())
    .bind(details)
    .fetch_one(db.conn())
    .await?;
    row.try_into()
}

pub async fn mark_held(db: &mut impl Db, id: i64) -> StoreResult<Withdrawal> {
    let row: Option<WithdrawalRow> = sqlx::query_as(&format!(
        "UPDATE referral_withdrawals SET \
         status = CASE WHEN held_at IS NULL AND status IN ('holding', 'canceled') THEN 'pending' ELSE status END, \
         comment = CASE WHEN held_at IS NULL AND status = 'canceled' THEN NULL ELSE comment END, \
         decided_at = CASE WHEN held_at IS NULL AND status = 'canceled' THEN NULL ELSE decided_at END, \
         held_at = COALESCE(held_at, now()) WHERE id = $1 RETURNING {W_COLUMNS}"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    row.ok_or(StoreError::NotFound("заявка на вывод"))?
        .try_into()
}

pub async fn cancel_unheld(db: &mut impl Db, id: i64, reason: &str) -> StoreResult<()> {
    sqlx::query(
        "UPDATE referral_withdrawals SET status = 'canceled', comment = $2, decided_at = now() \
         WHERE id = $1 AND status = 'holding' AND held_at IS NULL",
    )
    .bind(id)
    .bind(reason)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn decide(
    db: &mut impl Db,
    id: i64,
    owner: Option<ClientId>,
    status: WithdrawalStatus,
    comment: Option<&str>,
    admin: Option<AdminId>,
) -> StoreResult<Option<Withdrawal>> {
    let row: Option<WithdrawalRow> = sqlx::query_as(&format!(
        "UPDATE referral_withdrawals SET status = $3, comment = COALESCE($4, comment), admin_id = $5, decided_at = now() \
         WHERE id = $1 AND status = 'pending' AND ($2::bigint IS NULL OR client_id = $2) RETURNING {W_COLUMNS}"
    ))
    .bind(id)
    .bind(owner.map(|c| c.0))
    .bind(status.as_str())
    .bind(comment)
    .bind(admin.map(|a| a.0))
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

pub async fn withdrawal(db: &mut impl Db, id: i64) -> StoreResult<Option<Withdrawal>> {
    let row: Option<WithdrawalRow> = sqlx::query_as(&format!(
        "SELECT {W_COLUMNS} FROM referral_withdrawals WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    row.map(TryInto::try_into).transpose()
}

pub async fn client_withdrawals(
    db: &mut impl Db,
    client_id: ClientId,
    limit: i64,
) -> StoreResult<Vec<Withdrawal>> {
    let found: Vec<WithdrawalRow> = sqlx::query_as(&format!(
        "SELECT {W_COLUMNS} FROM referral_withdrawals WHERE client_id = $1 ORDER BY id DESC LIMIT $2"
    ))
    .bind(client_id.0)
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    rows(found)
}

pub async fn list_withdrawals(
    db: &mut impl Db,
    filter: WithdrawalFilter,
    page: PageRequest,
) -> StoreResult<Page<Withdrawal>> {
    let found: Vec<WithdrawalRow> = sqlx::query_as(&format!(
        "SELECT {W_COLUMNS} FROM referral_withdrawals WHERE ($1::text IS NULL OR status = $1) \
         AND ($2::bigint IS NULL OR id < $2) ORDER BY id DESC LIMIT $3"
    ))
    .bind(filter.status())
    .bind(page.after)
    .bind(page.fetch_limit())
    .fetch_all(db.conn())
    .await?;
    Ok(Page::from_overfetch(rows(found)?, page, |w| w.id))
}

pub async fn stale_holding(db: &mut impl Db, limit: i64) -> StoreResult<Vec<Withdrawal>> {
    let found: Vec<WithdrawalRow> = sqlx::query_as(&format!(
        "SELECT {W_COLUMNS} FROM referral_withdrawals WHERE status = 'holding' \
         AND created_at < now() - interval '1 minute' ORDER BY id LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    rows(found)
}

pub async fn unreleased(db: &mut impl Db, limit: i64) -> StoreResult<Vec<Withdrawal>> {
    let found: Vec<WithdrawalRow> = sqlx::query_as(&format!(
        "SELECT {W_COLUMNS} FROM referral_withdrawals WHERE status IN ('rejected', 'canceled') \
         AND held_at IS NOT NULL AND released_at IS NULL ORDER BY id LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    rows(found)
}

pub async fn mark_released(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query(
        "UPDATE referral_withdrawals SET released_at = COALESCE(released_at, now()) \
         WHERE id = $1 AND held_at IS NOT NULL",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(())
}
