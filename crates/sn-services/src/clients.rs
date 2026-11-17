use crate::Actor;
use crate::autorenew::AUTORENEW_METHOD;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::client::{
    Client, ClientStatus, Identity, IdentityKind, Locale, NewIdentity, RegistrationSource,
    TelegramProfile,
};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{AdminId, ClientId, OrderId, PaymentId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Amount, Money};
use sn_domain::order::{OrderKind, OrderStatus};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{PaymentStatus, ProviderId};
use sn_domain::subscription::Subscription;
use sn_store::{ClientFilter, ClientListItem, Db, LockSpace, NewClient, StoreError, Tx};
use uuid::Uuid;

#[cfg(test)]
mod tests;

pub const MAX_BLOCK_REASON: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistorySource {
    Payment,
    Event,
    Audit,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryEntry {
    pub at: DateTime<Utc>,
    pub source: HistorySource,
    pub kind: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentStatusView {
    pub payment_id: Uuid,
    pub status: PaymentStatus,
    pub amount: Amount,
    pub provider: ProviderId,
    pub order_id: Uuid,
    pub order_kind: OrderKind,
    pub order_status: OrderStatus,
    pub subscription_id: Option<SubscriptionId>,
    pub pay_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub paid_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const MONEY_EVENTS: [&str; 3] = ["payment_paid", "payment_refunded", "balance_changed"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientCard {
    pub client: Client,
    pub identities: Vec<Identity>,
    pub balance: Money,
    pub referral_balance: Money,
    pub subscriptions: Vec<Subscription>,
}

const BLOCKED_REASON: &str = "клиент заблокирован";

struct CanceledInvoices {
    payments: Vec<PaymentId>,
    orders: Vec<OrderId>,
    kept: Vec<PaymentId>,
}

async fn cancel_open_invoices(tx: &mut Tx, client_id: ClientId) -> ServiceResult<CanceledInvoices> {
    let pending: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT id, method IS NOT DISTINCT FROM $2 FROM payments \
         WHERE client_id = $1 AND status = 'pending' ORDER BY id",
    )
    .bind(client_id.0)
    .bind(AUTORENEW_METHOD)
    .fetch_all(tx.conn())
    .await?;
    let mut payments = Vec::new();
    let mut kept = Vec::new();
    for (id, in_flight) in pending {
        let id = PaymentId(id);
        if in_flight {
            kept.push(id);
            continue;
        }
        match tx
            .payments()
            .transition(id, PaymentStatus::Canceled, None, Some(BLOCKED_REASON))
            .await
        {
            Ok(change) if change.applied() => payments.push(id),
            Ok(_) | Err(StoreError::Invalid(_)) => {}
            Err(err) => return Err(err.into()),
        }
    }
    let open: Vec<i64> = sqlx::query_scalar(
        "SELECT o.id FROM orders o WHERE o.client_id = $1 AND o.status = 'pending' \
         AND NOT EXISTS (SELECT 1 FROM payments p WHERE p.order_id = o.id \
         AND p.status = 'pending' AND p.method = $2) ORDER BY o.id",
    )
    .bind(client_id.0)
    .bind(AUTORENEW_METHOD)
    .fetch_all(tx.conn())
    .await?;
    let mut orders = Vec::new();
    for id in open.into_iter().map(OrderId) {
        match tx.orders().transition(id, OrderStatus::Canceled).await {
            Ok((_, true)) => orders.push(id),
            Ok(_) | Err(StoreError::Invalid(_)) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(CanceledInvoices {
        payments,
        orders,
        kept,
    })
}

pub struct ClientOps<'a> {
    services: &'a Services,
}

impl<'a> ClientOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        ClientOps { services }
    }

    pub async fn ensure_telegram(
        &self,
        profile: &TelegramProfile,
        source: RegistrationSource,
        start_param: Option<&str>,
    ) -> ServiceResult<(Client, bool)> {
        let identity = NewIdentity::telegram(profile);
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Telegram, profile.id).await?;
        if let Some(client) = tx
            .clients()
            .find_by_identity(IdentityKind::Telegram, &identity.value)
            .await?
        {
            tx.clients()
                .update_identity_meta(IdentityKind::Telegram, &identity.value, &identity.meta)
                .await?;
            tx.commit().await?;
            return Ok((client, false));
        }
        let client = tx
            .clients()
            .create(&NewClient {
                locale: Locale::from_telegram(profile.language_code.as_deref()),
                registration_source: source,
                note: None,
            })
            .await?;
        tx.clients().add_identity(client.id, &identity).await?;
        tx.events()
            .push(&DomainEvent::ClientRegistered {
                client_id: client.id,
                source,
                start_param: start_param.map(str::to_string),
            })
            .await?;
        tx.commit().await?;
        Ok((client, true))
    }

    pub async fn card(&self, id: ClientId) -> ServiceResult<ClientCard> {
        let currency = self.services.settings().currency;
        let mut conn = self.services.store().conn().await?;
        let client = conn.clients().get(id).await?;
        let identities = conn.clients().identities(id).await?;
        let balance = conn.ledger().balance(id, Wallet::Main, currency).await?;
        let referral_balance = conn
            .ledger()
            .balance(id, Wallet::Referral, currency)
            .await?;
        let subscriptions = conn.subscriptions().by_client(id).await?;
        Ok(ClientCard {
            client,
            identities,
            balance,
            referral_balance,
            subscriptions,
        })
    }

    pub async fn set_note(
        &self,
        id: ClientId,
        note: Option<&str>,
        actor: Actor,
    ) -> ServiceResult<()> {
        let note = note.map(str::trim).filter(|n| !n.is_empty());
        if note.is_some_and(|n| n.chars().count() > 2_000) {
            return Err(ServiceError::rejected(
                "client.note_too_long",
                "заметка длиннее 2000 символов",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        tx.clients().get(id).await?;
        tx.clients().set_note(id, note).await?;
        tx.audit()
            .record(&actor.audit(
                "client.note",
                "client",
                Some(id.0),
                serde_json::json!({ "cleared": note.is_none() }),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn adjust_balance(
        &self,
        id: ClientId,
        delta: Money,
        comment: &str,
        admin: AdminId,
        idempotency_key: &str,
    ) -> ServiceResult<Money> {
        let currency = self.services.settings().currency;
        if delta.currency != currency {
            return Err(ServiceError::rejected(
                "balance.currency",
                format!("баланс ведётся в {currency}"),
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: id,
                wallet: Wallet::Main,
                amount: delta,
                kind: LedgerKind::AdminAdjust,
                order_id: None,
                payment_id: None,
                admin_id: Some(admin),
                comment: Some(comment.to_string()),
                idempotency_key: Some(format!("admin_adjust:{idempotency_key}")),
            })
            .await?;
        if !appended.duplicate {
            tx.events()
                .push(&DomainEvent::BalanceChanged {
                    client_id: id,
                    wallet: Wallet::Main,
                    kind: LedgerKind::AdminAdjust,
                    delta,
                })
                .await?;
        }
        tx.commit().await?;
        Ok(appended.balance_after)
    }

    pub async fn search(
        &self,
        filter: &ClientFilter,
        page: PageRequest,
    ) -> ServiceResult<Page<ClientListItem>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .clients()
            .search_at(
                filter,
                page,
                self.services.now(),
                self.services.settings().currency,
            )
            .await?)
    }

    pub async fn segment_counts(&self) -> ServiceResult<sn_store::SegmentCounts> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.clients().segment_counts(self.services.now()).await?)
    }

    pub async fn tariff_names(
        &self,
    ) -> ServiceResult<std::collections::HashMap<sn_domain::ids::TariffId, String>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .tariffs()
            .list(true)
            .await?
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect())
    }

    pub async fn block(
        &self,
        id: ClientId,
        reason: Option<&str>,
        actor: Actor,
    ) -> ServiceResult<()> {
        let reason = reason.map(str::trim).filter(|r| !r.is_empty());
        if reason.is_some_and(|r| r.chars().count() > MAX_BLOCK_REASON) {
            return Err(ServiceError::rejected(
                "client.block_reason",
                format!("причина длиннее {MAX_BLOCK_REASON} символов"),
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let client = tx.clients().lock(id).await?;
        let changed = !client.is_blocked();
        tx.clients()
            .set_status(id, ClientStatus::Blocked, reason)
            .await?;
        tx.sessions().revoke_all_client(id).await?;
        let invoices = cancel_open_invoices(&mut tx, id).await?;
        let ctx = self.services.module_context();
        for order_id in &invoices.orders {
            let order = tx.orders().get(*order_id).await?;
            self.services
                .modules()
                .order_canceled(&ctx, &mut tx, &order)
                .await?;
        }
        if changed {
            tx.events()
                .push(&DomainEvent::ClientBlocked {
                    client_id: id,
                    reason: reason.map(str::to_string),
                })
                .await?;
        }
        tx.audit()
            .record(&actor.audit(
                "client.block",
                "client",
                Some(id.0),
                serde_json::json!({
                    "reason": reason,
                    "changed": changed,
                    "canceled_payments": invoices.payments,
                    "canceled_orders": invoices.orders,
                    "kept_autorenew_payments": invoices.kept,
                }),
            ))
            .await?;
        tx.commit().await?;
        self.services.subscriptions().block_access(id, reason).await
    }

    pub async fn unblock(&self, id: ClientId, actor: Actor) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        let client = tx.clients().lock(id).await?;
        let changed = client.is_blocked();
        tx.clients()
            .set_status(id, ClientStatus::Active, None)
            .await?;
        if changed {
            tx.events()
                .push(&DomainEvent::ClientUnblocked { client_id: id })
                .await?;
        }
        tx.audit()
            .record(&actor.audit(
                "client.unblock",
                "client",
                Some(id.0),
                serde_json::json!({ "changed": changed }),
            ))
            .await?;
        tx.commit().await?;
        self.services.subscriptions().restore_access(id).await?;
        self.resume_charge_polls(id).await
    }

    async fn resume_charge_polls(&self, id: ClientId) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let waiting: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM payments WHERE client_id = $1 AND status = 'pending' AND method = $2 \
             AND (expires_at IS NULL OR expires_at > $3) ORDER BY id",
        )
        .bind(id.0)
        .bind(AUTORENEW_METHOD)
        .bind(self.services.now())
        .fetch_all(conn.conn())
        .await?;
        for payment_id in waiting.into_iter().map(PaymentId) {
            let queued = conn
                .jobs()
                .ensure_queued(&JobPayload::PollPayment { payment_id })
                .await?;
            if queued.armed() {
                tracing::info!(client = %id, payment = %payment_id, job = %queued.id(), "опрос автосписания возобновлён после разблокировки");
            }
        }
        Ok(())
    }

    pub async fn history(
        &self,
        id: ClientId,
        limit: u32,
        include_money: bool,
    ) -> ServiceResult<Vec<HistoryEntry>> {
        let limit = limit.clamp(1, PageRequest::MAX_LIMIT);
        let mut conn = self.services.store().conn().await?;
        conn.clients().get(id).await?;
        let mut entries = Vec::new();
        if include_money {
            let payments = conn
                .payments()
                .for_client(id, PageRequest::new(Some(limit), None))
                .await?;
            for payment in payments.items {
                entries.push(HistoryEntry {
                    at: payment.paid_at.unwrap_or(payment.created_at),
                    source: HistorySource::Payment,
                    kind: payment.status.as_str().to_string(),
                    data: serde_json::to_value(&payment)
                        .map_err(|e| ServiceError::Internal(e.to_string()))?,
                });
            }
        }
        for stored in conn.events().for_client(id, i64::from(limit)).await? {
            let kind = stored.event.kind();
            if MONEY_EVENTS.contains(&kind) && (!include_money || kind == "payment_paid") {
                continue;
            }
            entries.push(HistoryEntry {
                at: stored.occurred_at,
                source: HistorySource::Event,
                kind: kind.to_string(),
                data: serde_json::to_value(&stored.event)
                    .map_err(|e| ServiceError::Internal(e.to_string()))?,
            });
        }
        for record in conn
            .audit()
            .for_entity("client", id.0, i64::from(limit))
            .await?
        {
            entries.push(HistoryEntry {
                at: record.created_at,
                source: HistorySource::Audit,
                kind: record.action.clone(),
                data: serde_json::json!({
                    "actor_kind": record.actor_kind,
                    "actor_id": record.actor_id,
                    "details": record.details,
                }),
            });
        }
        entries.sort_by(|a, b| b.at.cmp(&a.at));
        entries.truncate(limit as usize);
        Ok(entries)
    }

    pub async fn payment_status(
        &self,
        client_id: ClientId,
        payment: Uuid,
    ) -> ServiceResult<PaymentStatusView> {
        let mut conn = self.services.store().conn().await?;
        let payment = match conn.payments().by_public_id(payment).await {
            Ok(payment) if payment.client_id == client_id => payment,
            Ok(_) | Err(sn_store::StoreError::NotFound(_)) => {
                return Err(ServiceError::NotFound("платёж"));
            }
            Err(err) => return Err(err.into()),
        };
        let order = conn.orders().get(payment.order_id).await?;
        let pay_url =
            (payment.status == PaymentStatus::Pending && payment.pay_url.is_some()).then(|| {
                let base = self
                    .services
                    .settings()
                    .public_url
                    .clone()
                    .unwrap_or_default();
                format!(
                    "{}/api/pay/{}",
                    base.trim_end_matches('/'),
                    payment.public_id
                )
            });
        Ok(PaymentStatusView {
            payment_id: payment.public_id,
            status: payment.status,
            amount: payment.amount,
            provider: payment.provider,
            order_id: order.public_id,
            order_kind: order.kind,
            order_status: order.status,
            subscription_id: order.subscription_id,
            pay_url,
            expires_at: payment.expires_at,
            paid_at: payment.paid_at,
            created_at: payment.created_at,
        })
    }
}
