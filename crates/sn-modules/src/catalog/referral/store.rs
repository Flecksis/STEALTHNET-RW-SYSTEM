use super::{PayoutMethod, payout_title};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::ids::{AdminId, ClientId, PaymentId};
use sn_domain::ledger::Wallet;
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{PaymentStatus, ProviderId};
use sn_store::{ClientPerson, Db, StoreError, StoreResult};
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

    pub fn title(self) -> &'static str {
        let english = match self {
            WithdrawalStatus::Holding | WithdrawalStatus::Pending => "Under review",
            WithdrawalStatus::Paid => "Paid",
            WithdrawalStatus::Rejected => "Declined",
            WithdrawalStatus::Canceled => "Cancelled",
        };
        sn_domain::lang::pick(self.title_ru(), english)
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
    Decided,
    All,
}

impl WithdrawalFilter {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.unwrap_or("pending") {
            "all" => Some(WithdrawalFilter::All),
            "decided" => Some(WithdrawalFilter::Decided),
            "pending" => Some(WithdrawalFilter::Status(WithdrawalStatus::Pending)),
            "paid" => Some(WithdrawalFilter::Status(WithdrawalStatus::Paid)),
            "rejected" => Some(WithdrawalFilter::Status(WithdrawalStatus::Rejected)),
            "canceled" => Some(WithdrawalFilter::Status(WithdrawalStatus::Canceled)),
            _ => None,
        }
    }

    fn statuses(self) -> Vec<&'static str> {
        match self {
            WithdrawalFilter::Status(status) => vec![status.as_str()],
            WithdrawalFilter::Decided => [
                WithdrawalStatus::Paid,
                WithdrawalStatus::Rejected,
                WithdrawalStatus::Canceled,
            ]
            .map(WithdrawalStatus::as_str)
            .to_vec(),
            WithdrawalFilter::All => Vec::new(),
        }
    }

    const fn oldest_first(self) -> bool {
        matches!(self, WithdrawalFilter::Status(WithdrawalStatus::Pending))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Withdrawal {
    pub id: i64,
    pub public_id: Uuid,
    pub client_id: ClientId,
    pub amount: Money,
    pub method: String,
    pub method_title: String,
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
    method_title: Option<String>,
    details: String,
    status: String,
    comment: Option<String>,
    created_at: DateTime<Utc>,
    decided_at: Option<DateTime<Utc>>,
}

const W_COLUMNS: &str = "id, public_id, client_id, amount_minor, currency, method, method_title, details, \
                         status, comment, created_at, decided_at";

impl TryFrom<WithdrawalRow> for Withdrawal {
    type Error = StoreError;

    fn try_from(row: WithdrawalRow) -> Result<Self, Self::Error> {
        Ok(Withdrawal {
            id: row.id,
            public_id: row.public_id,
            client_id: ClientId(row.client_id),
            amount: Money::new(row.amount_minor, Currency::parse(&row.currency)?),
            method_title: payout_title(&row.method, row.method_title.as_deref()),
            method: row.method,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdminWithdrawal {
    #[serde(flatten)]
    pub withdrawal: Withdrawal,
    pub decided_by: Option<String>,
    pub client: Option<ClientPerson>,
}

#[derive(sqlx::FromRow)]
struct AdminWithdrawalRow {
    #[sqlx(flatten)]
    row: WithdrawalRow,
    decided_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub invited: u32,
    pub referrers: u32,
    pub earned: Money,
    pub paid_out: Money,
    pub pending_count: u32,
    pub pending: Money,
    pub holders: u32,
    pub balance: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Referrer {
    pub client_id: ClientId,
    pub client: Option<ClientPerson>,
    pub code: Option<String>,
    pub personal_percent: Option<u8>,
    pub invited: u32,
    pub earned: Money,
    pub paid_out: Money,
    pub balance: Money,
}

#[derive(sqlx::FromRow)]
struct ReferrerRow {
    client_id: i64,
    code: Option<String>,
    percent: Option<i32>,
    invited: i64,
    earned: i64,
    paid_out: i64,
    balance: i64,
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
    method: &PayoutMethod,
    details: &str,
) -> StoreResult<Withdrawal> {
    let row: WithdrawalRow = sqlx::query_as(&format!(
        "INSERT INTO referral_withdrawals (client_id, amount_minor, currency, method, method_title, details) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING {W_COLUMNS}"
    ))
    .bind(client_id.0)
    .bind(amount.minor)
    .bind(amount.currency.code())
    .bind(&method.id)
    .bind(&method.title)
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
) -> StoreResult<Page<AdminWithdrawal>> {
    let (after, order) = if filter.oldest_first() {
        (">", "ASC")
    } else {
        ("<", "DESC")
    };
    let found: Vec<AdminWithdrawalRow> = sqlx::query_as(&format!(
        "SELECT {W_COLUMNS}, (SELECT a.email FROM admins a WHERE a.id = w.admin_id) AS decided_by \
         FROM referral_withdrawals w WHERE (cardinality($1::text[]) = 0 OR status = ANY($1)) \
         AND ($2::bigint IS NULL OR id {after} $2) ORDER BY id {order} LIMIT $3"
    ))
    .bind(filter.statuses())
    .bind(page.after)
    .bind(page.fetch_limit())
    .fetch_all(db.conn())
    .await?;
    let items = found
        .into_iter()
        .map(|found| {
            Ok(AdminWithdrawal {
                withdrawal: found.row.try_into()?,
                decided_by: found.decided_by,
                client: None,
            })
        })
        .collect::<StoreResult<Vec<AdminWithdrawal>>>()?;
    let mut page = Page::from_overfetch(items, page, |w| w.withdrawal.id);
    let ids: Vec<ClientId> = page.items.iter().map(|w| w.withdrawal.client_id).collect();
    let people = db.clients().people(&ids).await?;
    for item in &mut page.items {
        item.client = people.get(&item.withdrawal.client_id).cloned();
    }
    Ok(page)
}

pub async fn totals(db: &mut impl Db, currency: Currency) -> StoreResult<Totals> {
    let (invited, referrers, earned, paid_out, pending_count, pending): (i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT (SELECT count(*) FROM referral_links), \
               (SELECT count(DISTINCT referrer_id) FROM referral_links), \
               (SELECT COALESCE(sum(amount_minor - reversed_minor), 0)::bigint FROM referral_credits \
                WHERE currency = $1 AND credited_at IS NOT NULL), \
               (SELECT COALESCE(sum(amount_minor), 0)::bigint FROM referral_withdrawals \
                WHERE currency = $1 AND status = 'paid'), \
               (SELECT count(*) FROM referral_withdrawals WHERE currency = $1 AND status = 'pending'), \
               (SELECT COALESCE(sum(amount_minor), 0)::bigint FROM referral_withdrawals \
                WHERE currency = $1 AND status = 'pending')",
        )
        .bind(currency.code())
        .fetch_one(db.conn())
        .await?;
    let (holders, balance) = db.ledger().holders(Wallet::Referral, currency).await?;
    Ok(Totals {
        invited: count(invited),
        referrers: count(referrers),
        earned: Money::new(earned, currency),
        paid_out: Money::new(paid_out, currency),
        pending_count: count(pending_count),
        pending: Money::new(pending, currency),
        holders: count(holders),
        balance,
    })
}

pub async fn referrers(
    db: &mut impl Db,
    currency: Currency,
    page: PageRequest,
) -> StoreResult<Page<Referrer>> {
    let offset = page.after.unwrap_or(0).max(0);
    let found: Vec<ReferrerRow> = sqlx::query_as(
        "WITH ids AS ( \
             SELECT referrer_id AS client_id FROM referral_links \
             UNION SELECT referrer_id FROM referral_credits \
             UNION SELECT client_id FROM referral_codes WHERE percent IS NOT NULL), \
         inv AS (SELECT referrer_id AS client_id, count(*) AS invited FROM referral_links GROUP BY referrer_id), \
         earn AS (SELECT referrer_id AS client_id, sum(amount_minor - reversed_minor)::bigint AS earned \
             FROM referral_credits WHERE credited_at IS NOT NULL AND currency = $1 GROUP BY referrer_id), \
         paid AS (SELECT client_id, sum(amount_minor)::bigint AS paid_out FROM referral_withdrawals \
             WHERE status = 'paid' AND currency = $1 GROUP BY client_id) \
         SELECT ids.client_id, rc.code, rc.percent, COALESCE(inv.invited, 0) AS invited, \
             COALESCE(earn.earned, 0)::bigint AS earned, COALESCE(paid.paid_out, 0)::bigint AS paid_out, \
             COALESCE(b.amount_minor, 0) AS balance \
         FROM ids JOIN clients c ON c.id = ids.client_id AND c.deleted_at IS NULL \
         LEFT JOIN inv ON inv.client_id = ids.client_id \
         LEFT JOIN earn ON earn.client_id = ids.client_id \
         LEFT JOIN paid ON paid.client_id = ids.client_id \
         LEFT JOIN referral_codes rc ON rc.client_id = ids.client_id \
         LEFT JOIN client_balances b ON b.client_id = ids.client_id AND b.wallet = 'referral' AND b.currency = $1 \
         ORDER BY earned DESC, invited DESC, ids.client_id DESC OFFSET $2 LIMIT $3",
    )
    .bind(currency.code())
    .bind(offset)
    .bind(page.fetch_limit())
    .fetch_all(db.conn())
    .await?;
    let more = found.len() > page.limit as usize;
    let mut items: Vec<Referrer> = found
        .into_iter()
        .take(page.limit as usize)
        .map(|row| Referrer {
            client_id: ClientId(row.client_id),
            client: None,
            code: row.code,
            personal_percent: row.percent.and_then(|p| u8::try_from(p).ok()),
            invited: count(row.invited),
            earned: Money::new(row.earned, currency),
            paid_out: Money::new(row.paid_out, currency),
            balance: Money::new(row.balance, currency),
        })
        .collect();
    let ids: Vec<ClientId> = items.iter().map(|r| r.client_id).collect();
    let people = db.clients().people(&ids).await?;
    for item in &mut items {
        item.client = people.get(&item.client_id).cloned();
    }
    let next = more.then(|| offset + i64::from(page.limit));
    Ok(Page { items, next })
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
