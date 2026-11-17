use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId, OrderId, PaymentId, RefundId};
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{
    NewPayment, Payment, PaymentStatus, ProviderId, RefundSubscriptionAction,
};
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatAction {
    Balance,
    Review,
}

impl RepeatAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            RepeatAction::Balance => "balance",
            RepeatAction::Review => "review",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "balance" => Ok(RepeatAction::Balance),
            "review" => Ok(RepeatAction::Review),
            other => Err(StoreError::Decode(format!("повторная оплата: {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRefundStatus {
    Pending,
    Succeeded,
    Declined,
}

impl ProviderRefundStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderRefundStatus::Pending => "pending",
            ProviderRefundStatus::Succeeded => "succeeded",
            ProviderRefundStatus::Declined => "declined",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "pending" => Ok(ProviderRefundStatus::Pending),
            "succeeded" => Ok(ProviderRefundStatus::Succeeded),
            "declined" => Ok(ProviderRefundStatus::Declined),
            other => Err(StoreError::Decode(format!("статус возврата: {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProviderRefund {
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub subscription_action: RefundSubscriptionAction,
    pub reason: Option<String>,
    pub admin_id: Option<AdminId>,
    pub provider_refund_id: Option<String>,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRefund {
    pub id: i64,
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub subscription_action: RefundSubscriptionAction,
    pub reason: Option<String>,
    pub admin_id: Option<AdminId>,
    pub provider_refund_id: Option<String>,
    pub status: ProviderRefundStatus,
    pub refund_id: Option<RefundId>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct ProviderRefundRow {
    id: i64,
    payment_id: i64,
    amount_minor: i64,
    currency: String,
    subscription_action: String,
    reason: Option<String>,
    admin_id: Option<i64>,
    provider_refund_id: Option<String>,
    status: String,
    refund_id: Option<i64>,
    failure_reason: Option<String>,
    created_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
}

const REFUND_COLUMNS: &str = "id, payment_id, amount_minor, currency, subscription_action, reason, admin_id, \
                              provider_refund_id, status, refund_id, failure_reason, created_at, resolved_at";

const fn action_code(action: RefundSubscriptionAction) -> &'static str {
    match action {
        RefundSubscriptionAction::Keep => "keep",
        RefundSubscriptionAction::Cut => "cut",
        RefundSubscriptionAction::Disable => "disable",
    }
}

fn parse_action(value: &str) -> StoreResult<RefundSubscriptionAction> {
    match value {
        "keep" => Ok(RefundSubscriptionAction::Keep),
        "cut" => Ok(RefundSubscriptionAction::Cut),
        "disable" => Ok(RefundSubscriptionAction::Disable),
        other => Err(StoreError::Decode(format!("действие с подпиской: {other}"))),
    }
}

impl TryFrom<ProviderRefundRow> for ProviderRefund {
    type Error = StoreError;

    fn try_from(row: ProviderRefundRow) -> Result<Self, Self::Error> {
        Ok(ProviderRefund {
            id: row.id,
            payment_id: PaymentId(row.payment_id),
            amount: Amount::new(row.amount_minor, PayCurrency::parse(&row.currency)?),
            subscription_action: parse_action(&row.subscription_action)?,
            reason: row.reason,
            admin_id: row.admin_id.map(AdminId),
            provider_refund_id: row.provider_refund_id,
            status: ProviderRefundStatus::parse(&row.status)?,
            refund_id: row.refund_id.map(RefundId),
            failure_reason: row.failure_reason,
            created_at: row.created_at,
            resolved_at: row.resolved_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvoiceUpdate {
    pub pay_url: Option<String>,
    pub provider_txid: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusChange {
    Applied(Payment),
    Unchanged(Payment),
}

impl StatusChange {
    pub fn payment(&self) -> &Payment {
        match self {
            StatusChange::Applied(p) | StatusChange::Unchanged(p) => p,
        }
    }

    pub const fn applied(&self) -> bool {
        matches!(self, StatusChange::Applied(_))
    }
}

#[derive(sqlx::FromRow)]
struct PaymentRow {
    id: i64,
    public_id: Uuid,
    order_id: i64,
    client_id: i64,
    provider: String,
    method: Option<String>,
    amount_minor: i64,
    currency: String,
    status: String,
    provider_txid: Option<String>,
    pay_url: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    paid_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, public_id, order_id, client_id, provider, method, amount_minor, currency, status, \
                       provider_txid, pay_url, expires_at, paid_at, created_at";

pub(crate) const TRANSITION_ATTEMPTS: usize = 4;

pub const TXID_CONSTRAINT: &str = "payments_provider_txid_key";

impl TryFrom<PaymentRow> for Payment {
    type Error = StoreError;

    fn try_from(row: PaymentRow) -> Result<Self, Self::Error> {
        Ok(Payment {
            id: PaymentId(row.id),
            public_id: row.public_id,
            order_id: OrderId(row.order_id),
            client_id: ClientId(row.client_id),
            provider: ProviderId(row.provider),
            method: row.method,
            amount: Amount::new(row.amount_minor, PayCurrency::parse(&row.currency)?),
            status: PaymentStatus::parse(&row.status)?,
            provider_txid: row.provider_txid,
            pay_url: row.pay_url,
            expires_at: row.expires_at,
            paid_at: row.paid_at,
            created_at: row.created_at,
        })
    }
}

pub struct PaymentsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> PaymentsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        PaymentsRepo { conn }
    }

    pub async fn create(&mut self, payment: &NewPayment) -> StoreResult<Payment> {
        if payment.amount.minor < 0 {
            return Err(StoreError::Invalid("сумма платежа отрицательная".into()));
        }
        let row: PaymentRow = sqlx::query_as(&format!(
            "INSERT INTO payments (order_id, client_id, provider, method, amount_minor, currency, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {COLUMNS}"
        ))
        .bind(payment.order_id.0)
        .bind(payment.client_id.0)
        .bind(payment.provider.as_str())
        .bind(&payment.method)
        .bind(payment.amount.minor)
        .bind(payment.amount.currency.code())
        .bind(payment.expires_at)
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    async fn find_one(&mut self, condition: &str, id: i64) -> StoreResult<Payment> {
        let row: PaymentRow =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM payments WHERE {condition}"))
                .bind(id)
                .fetch_optional(&mut *self.conn)
                .await?
                .ok_or(StoreError::NotFound("платёж"))?;
        row.try_into()
    }

    pub async fn get(&mut self, id: PaymentId) -> StoreResult<Payment> {
        self.find_one("id = $1", id.0).await
    }

    pub async fn lock(&mut self, id: PaymentId) -> StoreResult<Payment> {
        self.find_one("id = $1 FOR UPDATE", id.0).await
    }

    pub async fn by_public_id(&mut self, public_id: Uuid) -> StoreResult<Payment> {
        let row: PaymentRow = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE public_id = $1"
        ))
        .bind(public_id)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("платёж"))?;
        row.try_into()
    }

    pub async fn by_provider_txid(
        &mut self,
        provider: &str,
        txid: &str,
    ) -> StoreResult<Option<Payment>> {
        let row: Option<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE provider = $1 AND provider_txid = $2"
        ))
        .bind(provider)
        .bind(txid)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn attach_invoice(
        &mut self,
        id: PaymentId,
        invoice: &InvoiceUpdate,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE payments SET pay_url = $2, provider_txid = COALESCE($3, provider_txid), \
             expires_at = COALESCE($4, expires_at), provider_payload = $5 WHERE id = $1",
        )
        .bind(id.0)
        .bind(&invoice.pay_url)
        .bind(&invoice.provider_txid)
        .bind(invoice.expires_at)
        .bind(&invoice.payload)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn transition(
        &mut self,
        id: PaymentId,
        to: PaymentStatus,
        provider_txid: Option<&str>,
        reason: Option<&str>,
    ) -> StoreResult<StatusChange> {
        let mut current = self.lock(id).await?;
        for _ in 0..TRANSITION_ATTEMPTS {
            if let (Some(known), Some(incoming)) = (current.provider_txid.as_deref(), provider_txid)
                && known != incoming
            {
                return Err(StoreError::Conflict {
                    constraint: Some(TXID_CONSTRAINT.into()),
                    message: format!("у платежа уже другая транзакция провайдера: {known}"),
                });
            }
            if !current.status.transition(to)? {
                return Ok(StatusChange::Unchanged(current));
            }
            let row: Option<PaymentRow> = sqlx::query_as(&format!(
                "UPDATE payments SET status = $2, provider_txid = COALESCE(provider_txid, $3), failure_reason = $4, \
                 paid_at = CASE WHEN $2 = 'paid' THEN COALESCE(paid_at, now()) ELSE paid_at END \
                 WHERE id = $1 AND status = $5 RETURNING {COLUMNS}"
            ))
            .bind(id.0)
            .bind(to.as_str())
            .bind(provider_txid)
            .bind(reason)
            .bind(current.status.as_str())
            .fetch_optional(&mut *self.conn)
            .await?;
            match row {
                Some(row) => return Ok(StatusChange::Applied(row.try_into()?)),
                None => current = self.lock(id).await?,
            }
        }
        Err(StoreError::Conflict {
            constraint: None,
            message: "статус платежа меняется параллельно, повторите".into(),
        })
    }

    pub async fn by_ids(&mut self, ids: &[PaymentId]) -> StoreResult<Vec<Payment>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE id = ANY($1) ORDER BY id"
        ))
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn for_order(&mut self, order_id: OrderId) -> StoreResult<Vec<Payment>> {
        let rows: Vec<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE order_id = $1 ORDER BY id"
        ))
        .bind(order_id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn for_client(
        &mut self,
        client_id: ClientId,
        page: PageRequest,
    ) -> StoreResult<Page<Payment>> {
        let rows: Vec<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE client_id = $1 AND ($2::bigint IS NULL OR id < $2) ORDER BY id DESC LIMIT $3"
        ))
        .bind(client_id.0)
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        let items = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<StoreResult<Vec<Payment>>>()?;
        Ok(Page::from_overfetch(items, page, |p| p.id.0))
    }

    pub async fn expire_stale(
        &mut self,
        now: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<PaymentId>> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "UPDATE payments SET status = 'canceled', failure_reason = 'expired' WHERE id IN ( \
             SELECT id FROM payments WHERE status = 'pending' AND expires_at < $1 ORDER BY expires_at LIMIT $2 \
             FOR UPDATE SKIP LOCKED) RETURNING id",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(ids.into_iter().map(PaymentId).collect())
    }

    pub async fn remember_txid(&mut self, id: PaymentId, provider_txid: &str) -> StoreResult<bool> {
        let updated: Option<i64> = sqlx::query_scalar(
            "UPDATE payments SET provider_txid = $2 WHERE id = $1 \
             AND status IN ('pending', 'failed', 'canceled') \
             AND (provider_txid IS NULL OR provider_txid = $2) RETURNING id",
        )
        .bind(id.0)
        .bind(provider_txid)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(updated.is_some())
    }

    pub async fn mark_repeat(
        &mut self,
        id: PaymentId,
        original: Option<PaymentId>,
        action: RepeatAction,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "INSERT INTO payment_repeats (payment_id, original_payment_id, action) VALUES ($1, $2, $3) \
             ON CONFLICT (payment_id) DO NOTHING",
        )
        .bind(id.0)
        .bind(original.map(|p| p.0))
        .bind(action.as_str())
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn repeat_action(&mut self, id: PaymentId) -> StoreResult<Option<RepeatAction>> {
        let action: Option<String> =
            sqlx::query_scalar("SELECT action FROM payment_repeats WHERE payment_id = $1")
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?;
        action.as_deref().map(RepeatAction::parse).transpose()
    }

    pub async fn create_provider_refund(
        &mut self,
        refund: &NewProviderRefund,
    ) -> StoreResult<ProviderRefund> {
        if refund.amount.minor <= 0 {
            return Err(StoreError::Invalid(
                "сумма возврата должна быть больше нуля".into(),
            ));
        }
        let row: ProviderRefundRow = sqlx::query_as(&format!(
            "INSERT INTO provider_refunds (payment_id, amount_minor, currency, subscription_action, reason, \
             admin_id, provider_refund_id, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             RETURNING {REFUND_COLUMNS}"
        ))
        .bind(refund.payment_id.0)
        .bind(refund.amount.minor)
        .bind(refund.amount.currency.code())
        .bind(action_code(refund.subscription_action))
        .bind(&refund.reason)
        .bind(refund.admin_id.map(|a| a.0))
        .bind(&refund.provider_refund_id)
        .bind(refund.requested_at)
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    pub async fn provider_refunds(
        &mut self,
        payment_id: PaymentId,
    ) -> StoreResult<Vec<ProviderRefund>> {
        let rows: Vec<ProviderRefundRow> = sqlx::query_as(&format!(
            "SELECT {REFUND_COLUMNS} FROM provider_refunds WHERE payment_id = $1 ORDER BY id"
        ))
        .bind(payment_id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn pending_provider_refunds(
        &mut self,
        requested_before: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<ProviderRefund>> {
        let rows: Vec<ProviderRefundRow> = sqlx::query_as(&format!(
            "SELECT {REFUND_COLUMNS} FROM provider_refunds WHERE status = 'pending' AND created_at <= $1 \
             ORDER BY created_at, id LIMIT $2"
        ))
        .bind(requested_before)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn resolve_provider_refund(
        &mut self,
        id: i64,
        status: ProviderRefundStatus,
        refund_id: Option<RefundId>,
        failure_reason: Option<&str>,
        at: DateTime<Utc>,
    ) -> StoreResult<bool> {
        if status == ProviderRefundStatus::Pending {
            return Err(StoreError::Invalid(
                "ожидающий возврат завершается успехом или отказом".into(),
            ));
        }
        let done = sqlx::query(
            "UPDATE provider_refunds SET status = $2, refund_id = $3, failure_reason = $4, resolved_at = $5 \
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(id)
        .bind(status.as_str())
        .bind(refund_id.map(|r| r.0))
        .bind(failure_reason)
        .bind(at)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::repo::NewClient;
    use crate::testing::test_store;
    use sn_domain::client::{Locale, RegistrationSource};
    use sn_domain::money::{Currency, Money};
    use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
    use sn_domain::pricing::Quote;

    async fn pending(db: &mut impl Db, minor: i64) -> Payment {
        let client = db
            .clients()
            .create(&NewClient {
                locale: Locale::Ru,
                registration_source: RegistrationSource::Bot,
                note: None,
            })
            .await
            .unwrap()
            .id;
        let total = Money::new(minor, Currency::Usd);
        let order = db
            .orders()
            .create(&NewOrder {
                client_id: client,
                subscription_id: None,
                kind: OrderKind::TopUp,
                intent: PurchaseIntent::TopUp { amount: total },
                quote: Quote {
                    currency: Currency::Usd,
                    lines: Vec::new(),
                    subtotal: total,
                    discount: Money::zero(Currency::Usd),
                    total,
                    stars: None,
                    days: 0,
                    extra_devices: 0,
                },
                snapshot: None,
            })
            .await
            .unwrap();
        db.payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id: client,
                provider: ProviderId::from("yoomoney"),
                method: None,
                amount: total.into(),
                expires_at: None,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn другая_транзакция_у_оплаченного_платежа_не_выдаётся_за_тот_же_платёж() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let payment = pending(&mut conn, 49_900).await;
        let paid = conn
            .payments()
            .transition(payment.id, PaymentStatus::Paid, Some("op-1"), None)
            .await
            .unwrap();
        assert!(paid.applied());
        let same = conn
            .payments()
            .transition(payment.id, PaymentStatus::Paid, Some("op-1"), None)
            .await
            .unwrap();
        assert!(!same.applied());
        let other = conn
            .payments()
            .transition(payment.id, PaymentStatus::Paid, Some("op-2"), None)
            .await;
        assert!(
            matches!(&other, Err(StoreError::Conflict { constraint: Some(c), .. }) if c == TXID_CONSTRAINT),
            "{other:?}"
        );
        conn.payments()
            .transition(payment.id, PaymentStatus::Refunded, None, None)
            .await
            .unwrap();
        let after_refund = conn
            .payments()
            .transition(payment.id, PaymentStatus::Paid, Some("op-3"), None)
            .await;
        assert!(
            matches!(&after_refund, Err(StoreError::Conflict { constraint: Some(c), .. }) if c == TXID_CONSTRAINT),
            "{after_refund:?}"
        );
        assert_eq!(
            conn.payments()
                .get(payment.id)
                .await
                .unwrap()
                .provider_txid
                .as_deref(),
            Some("op-1")
        );
        drop(conn);
        db.cleanup().await;
    }
}
