use super::{PaymentInbox, RefundCommand, RefundStep, amount_text};
use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sn_domain::client::ClientStatus;
use sn_domain::ids::OrderId;
use sn_domain::jobs::JobPayload;
use sn_domain::money::Amount;
use sn_domain::order::{Order, OrderStatus};
use sn_domain::payment::{Payment, PaymentStatus, RefundMethod, RefundSubscriptionAction};
use sn_payments::ProviderKind;
use sn_store::{Db, ProviderRefund, ProviderRefundStatus, Tx, audit_entity};
use uuid::Uuid;

pub const BLOCKED_REFUND_REASON: &str = "клиент заблокирован: оплату вернули автоматически";
pub const BLOCKED_REFUND_AUDIT: &str = "payment.refund_blocked";
pub const BLOCKED_REFUND_REQUESTED_AUDIT: &str = "payment.refund_blocked_requested";
pub const BLOCKED_REFUND_TEMPLATE: &str = "payment.refunded_blocked";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockedRefund {
    Refunded,
    Requested,
    Retry(String),
    Kept(String),
    Unblocked,
    NothingPaid,
}

fn refund_key(payment: &Payment) -> Uuid {
    let digest = Sha256::digest(format!("blocked-refund:{}", payment.public_id).as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

pub(super) fn requested_by_system(request: &ProviderRefund) -> bool {
    request.admin_id.is_none() && request.reason.as_deref() == Some(BLOCKED_REFUND_REASON)
}

pub(super) async fn notify_client(
    tx: &mut Tx,
    order: &Order,
    payment: &Payment,
    amount: Amount,
) -> ServiceResult<()> {
    tx.jobs()
        .enqueue(
            &JobPayload::Notify {
                client_id: payment.client_id,
                template: BLOCKED_REFUND_TEMPLATE.into(),
                params: serde_json::json!({
                    "amount": amount_text(amount),
                    "order_id": order.public_id,
                    "payment_id": payment.id,
                }),
            },
            None,
        )
        .await?;
    Ok(())
}

async fn client_state(tx: &mut Tx, order: &Order) -> ServiceResult<(bool, Option<DateTime<Utc>>)> {
    let row: Option<(String, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT status, blocked_at FROM clients WHERE id = $1 FOR SHARE")
            .bind(order.client_id.0)
            .fetch_optional(tx.conn())
            .await?;
    let (status, blocked_at) = row.ok_or(ServiceError::NotFound("клиент"))?;
    Ok((status == ClientStatus::Blocked.as_str(), blocked_at))
}

async fn paid_payments(tx: &mut Tx, order: &Order) -> ServiceResult<Vec<Payment>> {
    let mut paid = Vec::new();
    for payment in tx.payments().for_order(order.id).await? {
        if payment.status.is_paid_like()
            && payment.amount.minor > 0
            && tx.payments().repeat_action(payment.id).await?.is_none()
        {
            paid.push(payment);
        }
    }
    Ok(paid)
}

impl PaymentInbox<'_> {
    pub async fn refund_blocked(&self, order_id: OrderId) -> ServiceResult<BlockedRefund> {
        let mut tx = self.services.store().begin().await?;
        let order = tx.orders().get(order_id).await?;
        let (blocked, blocked_at) = client_state(&mut tx, &order).await?;
        let paid = paid_payments(&mut tx, &order).await?;
        let Some(first) = paid.first() else {
            return Ok(BlockedRefund::NothingPaid);
        };
        if !blocked {
            return Ok(BlockedRefund::Unblocked);
        }
        if paid.len() > 1 {
            return Ok(BlockedRefund::Kept(
                "по заказу несколько оплат, вернуть нужно вручную".into(),
            ));
        }
        let payment = tx.payments().lock(first.id).await?;
        let order = tx.orders().get(order_id).await?;
        match order.status {
            OrderStatus::Refunded => return Ok(BlockedRefund::Refunded),
            OrderStatus::Paid | OrderStatus::Review => {}
            other => {
                return Ok(BlockedRefund::Kept(format!(
                    "заказ в статусе {}",
                    other.as_str()
                )));
            }
        }
        let requests = tx.payments().provider_refunds(payment.id).await?;
        if requests
            .iter()
            .any(|r| r.status == ProviderRefundStatus::Pending)
        {
            return Ok(BlockedRefund::Requested);
        }
        if let Some(reason) = self
            .kept(&mut tx, &order, &payment, &requests, blocked_at)
            .await?
        {
            return Ok(BlockedRefund::Kept(reason));
        }
        let command = RefundCommand {
            amount: None,
            method: RefundMethod::Provider,
            subscription_action: RefundSubscriptionAction::Keep,
            reason: Some(BLOCKED_REFUND_REASON.into()),
            key: Some(refund_key(&payment)),
        };
        let step = match self.refund_locked(&mut tx, &payment, &command, None).await {
            Ok(step) => step,
            Err(ServiceError::Payment(err)) if crate::autorenew::transient(&err) => {
                tracing::warn!(order = %order.id, payment = %payment.id, error = %err, "возврат оплаты заблокированного клиента отложен");
                return Ok(BlockedRefund::Retry(format!(
                    "клиент заблокирован, возврат оплаты отложен: {err}"
                )));
            }
            Err(ServiceError::Payment(err)) => {
                tracing::warn!(order = %order.id, payment = %payment.id, error = %err, "платёжная система не вернула оплату заблокированного клиента");
                return Ok(BlockedRefund::Kept(format!(
                    "платёжная система не вернула деньги: {err}"
                )));
            }
            Err(ServiceError::Rejected { message, .. }) => {
                return Ok(BlockedRefund::Kept(message));
            }
            Err(err) => return Err(err),
        };
        let (action, details, outcome) = match &step {
            RefundStep::Requested {
                request_id,
                amount,
                provider_refund_id,
                ..
            } => (
                BLOCKED_REFUND_REQUESTED_AUDIT,
                serde_json::json!({
                    "order_id": order.id,
                    "client_id": order.client_id,
                    "request_id": request_id,
                    "amount": amount,
                    "provider": payment.provider,
                    "provider_refund_id": provider_refund_id,
                    "reason": BLOCKED_REFUND_REASON,
                }),
                BlockedRefund::Requested,
            ),
            RefundStep::Recorded { recorded, amount } => (
                BLOCKED_REFUND_AUDIT,
                serde_json::json!({
                    "order_id": order.id,
                    "client_id": order.client_id,
                    "refund_id": recorded.refund_id,
                    "amount": amount,
                    "provider": payment.provider,
                    "reason": BLOCKED_REFUND_REASON,
                }),
                BlockedRefund::Refunded,
            ),
        };
        tx.audit()
            .record(&Actor::System.audit(
                action,
                audit_entity::PAYMENT,
                Some(payment.id.0),
                details,
            ))
            .await?;
        if let RefundStep::Recorded { recorded, amount } = &step {
            notify_client(&mut tx, &recorded.order, &payment, *amount).await?;
        }
        tx.commit().await?;
        tracing::info!(order = %order.id, payment = %payment.id, client = %order.client_id, outcome = ?outcome, "оплата заблокированного клиента возвращается автоматически");
        Ok(outcome)
    }

    async fn kept(
        &self,
        tx: &mut Tx,
        order: &Order,
        payment: &Payment,
        requests: &[ProviderRefund],
        blocked_at: Option<DateTime<Utc>>,
    ) -> ServiceResult<Option<String>> {
        let provider = self.services.payments().get(payment.provider.as_str());
        let reason = match provider {
            Some(provider) if provider.kind() == ProviderKind::Balance => {
                Some("оплата прошла с баланса")
            }
            Some(provider) if provider.supports_refunds() => None,
            _ => Some("платёжная система не умеет возвращать деньги"),
        };
        if let Some(reason) = reason {
            return Ok(Some(reason.into()));
        }
        if payment.provider_txid.is_none() {
            return Ok(Some("у оплаты нет номера в платёжной системе".into()));
        }
        if payment.status != PaymentStatus::Paid
            || !tx.refunds().for_payment(payment.id).await?.is_empty()
        {
            return Ok(Some("по оплате уже был возврат".into()));
        }
        if let Some(closed) = requests.last() {
            return Ok(Some(match closed.failure_reason.as_deref() {
                Some(why) => format!("заявку на возврат закрыли без возврата: {why}"),
                None => "по оплате уже была заявка на возврат".into(),
            }));
        }
        let plan = tx.orders().plan_state(order.id).await?;
        if plan.saved || plan.applied_at.is_some() {
            return Ok(Some(
                "заказ мог уже дойти до Remnawave, проверьте подписку".into(),
            ));
        }
        match (payment.paid_at, blocked_at) {
            (Some(paid), Some(blocked)) if paid >= blocked => Ok(None),
            _ => Ok(Some("оплата пришла до блокировки".into())),
        }
    }
}
