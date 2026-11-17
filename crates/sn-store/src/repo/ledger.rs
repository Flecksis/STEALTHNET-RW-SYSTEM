use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId, LedgerEntryId, OrderId, PaymentId};
use sn_domain::ledger::{LedgerEntry, LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Currency, Money};
use sn_domain::paging::{Page, PageRequest};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerAppend {
    pub entry_id: LedgerEntryId,
    pub balance_after: Money,
    pub duplicate: bool,
}

#[derive(sqlx::FromRow)]
struct EntryRow {
    id: i64,
    client_id: i64,
    wallet: String,
    currency: String,
    delta_minor: i64,
    kind: String,
    order_id: Option<i64>,
    payment_id: Option<i64>,
    admin_id: Option<i64>,
    comment: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<EntryRow> for LedgerEntry {
    type Error = StoreError;

    fn try_from(row: EntryRow) -> Result<Self, Self::Error> {
        Ok(LedgerEntry {
            id: LedgerEntryId(row.id),
            client_id: ClientId(row.client_id),
            wallet: Wallet::parse(&row.wallet)?,
            amount: Money::new(row.delta_minor, Currency::parse(&row.currency)?),
            kind: LedgerKind::parse(&row.kind)?,
            order_id: row.order_id.map(OrderId),
            payment_id: row.payment_id.map(PaymentId),
            admin_id: row.admin_id.map(AdminId),
            comment: row.comment,
            created_at: row.created_at,
        })
    }
}

const IDEMPOTENCY_CONSTRAINT: &str = "balance_entries_idempotency_key";

async fn write_entry(conn: &mut PgConnection, entry: &NewLedgerEntry) -> StoreResult<LedgerAppend> {
    let currency = entry.amount.currency;
    sqlx::query(
        "INSERT INTO client_balances (client_id, wallet, currency, amount_minor) VALUES ($1, $2, $3, 0) \
         ON CONFLICT (client_id, wallet, currency) DO NOTHING",
    )
    .bind(entry.client_id.0)
    .bind(entry.wallet.as_str())
    .bind(currency.code())
    .execute(&mut *conn)
    .await?;
    let moved: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "UPDATE client_balances SET amount_minor = amount_minor + $4, updated_at = now() \
         WHERE client_id = $1 AND wallet = $2 AND currency = $3 RETURNING amount_minor",
    )
    .bind(entry.client_id.0)
    .bind(entry.wallet.as_str())
    .bind(currency.code())
    .bind(entry.amount.minor)
    .fetch_one(&mut *conn)
    .await;
    let balance_after = match moved {
        Ok(value) => value,
        Err(sqlx::Error::Database(db)) if db.is_check_violation() => {
            return Err(StoreError::InsufficientFunds);
        }
        Err(err) => return Err(err.into()),
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO balance_entries (client_id, wallet, currency, delta_minor, balance_after_minor, kind, \
         order_id, payment_id, admin_id, comment, idempotency_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
    )
    .bind(entry.client_id.0)
    .bind(entry.wallet.as_str())
    .bind(currency.code())
    .bind(entry.amount.minor)
    .bind(balance_after)
    .bind(entry.kind.as_str())
    .bind(entry.order_id.map(|o| o.0))
    .bind(entry.payment_id.map(|p| p.0))
    .bind(entry.admin_id.map(|a| a.0))
    .bind(&entry.comment)
    .bind(&entry.idempotency_key)
    .fetch_one(&mut *conn)
    .await?;
    Ok(LedgerAppend {
        entry_id: LedgerEntryId(id),
        balance_after: Money::new(balance_after, currency),
        duplicate: false,
    })
}

pub struct LedgerRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> LedgerRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        LedgerRepo { conn }
    }

    pub async fn balance(
        &mut self,
        client_id: ClientId,
        wallet: Wallet,
        currency: Currency,
    ) -> StoreResult<Money> {
        let amount: Option<i64> = sqlx::query_scalar(
            "SELECT amount_minor FROM client_balances WHERE client_id = $1 AND wallet = $2 AND currency = $3",
        )
        .bind(client_id.0)
        .bind(wallet.as_str())
        .bind(currency.code())
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(Money::new(amount.unwrap_or(0), currency))
    }

    pub async fn append(&mut self, entry: &NewLedgerEntry) -> StoreResult<LedgerAppend> {
        entry.validate()?;
        if let Some(found) = self.existing(entry).await? {
            return Ok(found);
        }
        let mut tx = sqlx::Connection::begin(&mut *self.conn).await?;
        match write_entry(&mut tx, entry).await {
            Ok(appended) => {
                tx.commit().await?;
                Ok(appended)
            }
            Err(err) => {
                tx.rollback().await?;
                if err.constraint() == Some(IDEMPOTENCY_CONSTRAINT)
                    && let Some(found) = self.existing(entry).await?
                {
                    return Ok(found);
                }
                Err(err)
            }
        }
    }

    async fn existing(&mut self, entry: &NewLedgerEntry) -> StoreResult<Option<LedgerAppend>> {
        let Some(key) = &entry.idempotency_key else {
            return Ok(None);
        };
        let found: Option<(i64, i64)> = sqlx::query_as(
            "SELECT id, balance_after_minor FROM balance_entries WHERE idempotency_key = $1",
        )
        .bind(key)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(found.map(|(id, balance_after)| LedgerAppend {
            entry_id: LedgerEntryId(id),
            balance_after: Money::new(balance_after, entry.amount.currency),
            duplicate: true,
        }))
    }

    pub async fn entries(
        &mut self,
        client_id: ClientId,
        page: PageRequest,
    ) -> StoreResult<Page<LedgerEntry>> {
        let rows: Vec<EntryRow> = sqlx::query_as(
            "SELECT id, client_id, wallet, currency, delta_minor, kind, order_id, payment_id, admin_id, comment, created_at \
             FROM balance_entries WHERE client_id = $1 AND ($2::bigint IS NULL OR id < $2) ORDER BY id DESC LIMIT $3",
        )
        .bind(client_id.0)
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        let items = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<StoreResult<Vec<LedgerEntry>>>()?;
        Ok(Page::from_overfetch(items, page, |e| e.id.0))
    }
}
