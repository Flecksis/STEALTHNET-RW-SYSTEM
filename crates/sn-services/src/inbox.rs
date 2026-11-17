use crate::error::{ServiceError, ServiceResult};
use crate::limits::{RATE_LIMITED, RateRule, Strike, UNKNOWN_ADDRESS, ip_key};
use crate::services::Services;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::{AdminId, ClientId, LedgerEntryId, OrderId, PaymentId, RefundId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Amount, Money};
use sn_domain::order::{Order, OrderKind, OrderStatus};
use sn_domain::payment::{
    AmountCheck, NewPayment, Payment, PaymentStatus, RefundMethod, RefundSubscriptionAction,
    check_paid_amount, providers,
};
use sn_modules::ModuleError;
use sn_payments::providers::{StarsPayment, StarsPreCheckout};
use sn_payments::{
    PaymentError, PaymentRef, ProviderKind, ProviderStatus, RefundRequest, SavedMethod,
    WebhookOutcome, WebhookReply, WebhookRequest,
};
use sn_store::{
    ActorKind, AuditEntry, Db, InboxOutcome, NewInboundWebhook, NewProviderRefund, NewRefund,
    ProviderRefund, ProviderRefundStatus, RepeatAction, StatusChange, StoreError, Tx, audit_entity,
};
use std::net::IpAddr;
use uuid::Uuid;

mod refund_action;
mod refund_requests;
#[cfg(test)]
mod tests;

pub use refund_action::{
    REFUND_ACTION_ATTEMPTS, REFUND_ACTION_FAILED, RefundActionOutcome, RefundActionReview,
};
pub use refund_requests::{PENDING_REFUNDS_MAX, PendingRefundRequest};

fn digest_hex(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

const STORED_HEADERS: [&str; 4] = ["content-type", "user-agent", "x-forwarded-for", "x-real-ip"];
const STORED_TEXT_LIMIT: usize = 300;

pub const MAX_WEBHOOK_BODY: usize = 64 * 1024;
pub const WEBHOOK_RETENTION_DAYS: i64 = 30;
pub const REJECTED_WEBHOOK_RETENTION_DAYS: i64 = 3;
pub const REPEAT_METHOD: &str = "repeat";
pub const REPEAT_CREDITED_TEMPLATE: &str = "payment.repeat_credited";
pub const REPEAT_REVIEW_TEMPLATE: &str = "payment.repeat_review";

const PURGE_BATCH: i64 = 500;
const PURGE_ROUNDS: usize = 40;
const REFUND_POLL_DELAY_SECONDS: i64 = 60;

const REJECTED_WEBHOOKS: RateRule = RateRule {
    scope: "payment_webhook_rejected",
    limit: 30,
    window_seconds: 600,
};

const WEBHOOK_LOOKUPS: RateRule = RateRule {
    scope: "payment_webhook_lookup",
    limit: 10,
    window_seconds: 60,
};

fn address_key(ip: Option<IpAddr>) -> String {
    ip.map_or_else(|| UNKNOWN_ADDRESS.to_string(), ip_key)
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn rejection_outcome(code: &str) -> InboxOutcome {
    if code == "payment.amount_mismatch" {
        InboxOutcome::AmountMismatch
    } else {
        InboxOutcome::RejectedBusiness
    }
}

fn stored_headers(request: &WebhookRequest) -> serde_json::Value {
    serde_json::Value::Object(
        request
            .headers
            .iter()
            .filter(|(k, _)| STORED_HEADERS.contains(&k.to_ascii_lowercase().as_str()))
            .map(|(k, v)| {
                (
                    k.to_ascii_lowercase(),
                    serde_json::Value::String(clip(v, STORED_TEXT_LIMIT)),
                )
            })
            .collect(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundWebhook {
    pub provider: String,
    pub request: WebhookRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebhookResponse {
    pub reply: WebhookReply,
}

impl WebhookResponse {
    fn status(status: u16) -> Self {
        WebhookResponse {
            reply: WebhookReply::Empty { status },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcceptResult {
    Accepted {
        order_id: OrderId,
        payment_id: PaymentId,
    },
    AlreadyApplied {
        payment_id: PaymentId,
    },
    StatusChanged {
        payment_id: PaymentId,
        status: PaymentStatus,
    },
    Ignored {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PollResult {
    Settled,
    Pending,
    Unsupported,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RefundPollReport {
    pub settled: usize,
    pub waiting: usize,
    pub unsupported: usize,
    pub failed: usize,
    pub actions_done: usize,
    pub actions_retry: usize,
    pub actions_failed: usize,
}

fn keep_subscription() -> RefundSubscriptionAction {
    RefundSubscriptionAction::Keep
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundCommand {
    #[serde(default)]
    pub amount: Option<Amount>,
    pub method: RefundMethod,
    #[serde(default = "keep_subscription")]
    pub subscription_action: RefundSubscriptionAction,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefundResult {
    pub refund_id: Option<RefundId>,
    pub request_id: Option<i64>,
    pub pending: bool,
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub status: PaymentStatus,
    pub subscription_error: Option<String>,
}

struct RecordedRefund {
    refund_id: RefundId,
    status: PaymentStatus,
    order: Order,
}

struct RefundSpec<'r> {
    amount: Amount,
    method: RefundMethod,
    action: RefundSubscriptionAction,
    reason: Option<&'r str>,
    admin: Option<AdminId>,
    provider_refund_id: Option<&'r str>,
    held: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefundPoll {
    Settled,
    Waiting,
    Unsupported,
}

pub(crate) fn saved_method_context(client_id: ClientId, provider: &str) -> String {
    format!("saved_method:{client_id}:{provider}")
}

pub(crate) async fn credit_topup(
    tx: &mut Tx,
    order: &Order,
    paid: &Payment,
) -> ServiceResult<bool> {
    let amount = paid.amount.as_money().ok_or_else(|| {
        ServiceError::rejected("payment.topup_currency", "пополнение только в валюте")
    })?;
    let appended = tx
        .ledger()
        .append(&NewLedgerEntry {
            client_id: paid.client_id,
            wallet: Wallet::Main,
            amount,
            kind: LedgerKind::TopUp,
            order_id: Some(order.id),
            payment_id: Some(paid.id),
            admin_id: None,
            comment: None,
            idempotency_key: Some(format!("topup:payment:{}", paid.id)),
        })
        .await?;
    if !appended.duplicate {
        tx.events()
            .push(&DomainEvent::BalanceChanged {
                client_id: paid.client_id,
                wallet: Wallet::Main,
                kind: LedgerKind::TopUp,
                delta: amount,
            })
            .await?;
    }
    Ok(!appended.duplicate)
}

fn same_refund(stored: &str, event_key: &str) -> bool {
    stored == event_key
        || event_key
            .rsplit_once("refund:")
            .is_some_and(|(_, id)| !id.is_empty() && id == stored)
}

fn pending_requests(requests: &[ProviderRefund]) -> impl Iterator<Item = &ProviderRefund> {
    requests
        .iter()
        .filter(|r| r.status == ProviderRefundStatus::Pending)
}

fn pending_minor(requests: &[ProviderRefund]) -> i64 {
    pending_requests(requests).map(|r| r.amount.minor).sum()
}

fn matching_request<'r>(
    requests: &'r [ProviderRefund],
    event_key: &str,
    amount: Option<Amount>,
) -> Option<&'r ProviderRefund> {
    pending_requests(requests)
        .find(|r| {
            r.provider_refund_id
                .as_deref()
                .is_some_and(|id| same_refund(id, event_key))
        })
        .or_else(|| {
            amount.and_then(|amount| pending_requests(requests).find(|r| r.amount == amount))
        })
}

fn repeat_txid<'t>(payment: &Payment, incoming: Option<&'t str>) -> Option<&'t str> {
    let incoming = incoming?;
    let known = payment.provider_txid.as_deref()?;
    (payment.status.is_paid_like() && known != incoming).then_some(incoming)
}

fn credits_balance(order: &Order, repeat: Option<RepeatAction>) -> bool {
    match repeat {
        Some(RepeatAction::Balance) => true,
        Some(RepeatAction::Review) => false,
        None => order.kind == OrderKind::TopUp,
    }
}

fn amount_text(amount: Amount) -> String {
    match (amount.as_money(), amount.as_stars()) {
        (Some(money), _) => format::money(money),
        (None, Some(stars)) => format::stars(stars),
        (None, None) => amount.to_string(),
    }
}

async fn refunded_minor(tx: &mut Tx, payment: PaymentId) -> ServiceResult<i64> {
    Ok(tx
        .refunds()
        .for_payment(payment)
        .await?
        .iter()
        .map(|r| r.amount.minor)
        .sum())
}

pub struct PaymentInbox<'a> {
    services: &'a Services,
}

async fn find_payment(
    tx: &mut Tx,
    provider: &str,
    reference: &PaymentRef,
) -> ServiceResult<PaymentId> {
    let id = match reference {
        PaymentRef::Payment(id) => *id,
        PaymentRef::PublicId(public_id) => tx.payments().by_public_id(*public_id).await?.id,
        PaymentRef::ProviderTxid(txid) => {
            tx.payments()
                .by_provider_txid(provider, txid)
                .await?
                .ok_or(ServiceError::NotFound("платёж"))?
                .id
        }
    };
    Ok(id)
}

async fn locate(tx: &mut Tx, provider: &str, reference: &PaymentRef) -> ServiceResult<Payment> {
    let id = find_payment(tx, provider, reference).await?;
    let payment = tx.payments().lock(id).await?;
    if payment.provider.as_str() != provider {
        return Err(ServiceError::rejected(
            "payment.provider_mismatch",
            "платёж выставлен другой платёжной системой",
        ));
    }
    Ok(payment)
}

async fn locate_refunded(
    tx: &mut Tx,
    provider: &str,
    reference: &PaymentRef,
    txid: Option<&str>,
) -> ServiceResult<Payment> {
    let payment = locate(tx, provider, reference).await?;
    let Some(txid) = txid.filter(|t| {
        payment
            .provider_txid
            .as_deref()
            .is_some_and(|known| known != *t)
    }) else {
        return Ok(payment);
    };
    match tx.payments().by_provider_txid(provider, txid).await? {
        Some(other) if other.order_id == payment.order_id => {
            Ok(tx.payments().lock(other.id).await?)
        }
        _ => Ok(payment),
    }
}

impl<'a> PaymentInbox<'a> {
    pub fn new(services: &'a Services) -> Self {
        PaymentInbox { services }
    }

    pub async fn handle(&self, hook: InboundWebhook) -> ServiceResult<WebhookResponse> {
        let registry = self.services.payments();
        let Some(provider) = registry.get(&hook.provider) else {
            return Ok(WebhookResponse::status(404));
        };
        if hook.request.body.len() > MAX_WEBHOOK_BODY {
            return Ok(WebhookResponse::status(413));
        }
        let dedupe_key = digest_hex(&hook.request.body);
        if !registry.accepts_webhooks(provider.id()) {
            let reason =
                if provider.kind() == ProviderKind::External && provider.accepts_http_webhooks() {
                    "платёжная система не настроена"
                } else {
                    "платёжная система не принимает уведомления по HTTP"
                };
            self.note_rejection(
                provider.id(),
                &hook.request,
                &dedupe_key,
                InboxOutcome::NotAccepted,
                reason,
            )
            .await;
            return Ok(WebhookResponse::status(404));
        }
        let config = registry.config(provider.id());
        let gate = if provider.verifies_webhooks_by_api() {
            match self.lookup_gate(provider.id(), &hook.request).await? {
                Some(strike) => Some(strike),
                None => return Ok(WebhookResponse::status(429)),
            }
        } else {
            None
        };
        let parsed = match provider.parse_webhook(&config, &hook.request).await {
            Ok(parsed) => {
                if let Some(strike) = &gate
                    && let Err(err) = self.services.limits().forgive(strike).await
                {
                    tracing::warn!(provider = provider.id(), error = %err, "отметка проверки вебхука не снята");
                }
                parsed
            }
            Err(err) => {
                let (outcome, status) = match &err {
                    PaymentError::BadSignature => (InboxOutcome::RejectedSignature, 401),
                    PaymentError::BadPayload(_) => (InboxOutcome::RejectedPayload, 400),
                    _ => (InboxOutcome::Error, 503),
                };
                self.note_rejection(
                    provider.id(),
                    &hook.request,
                    &dedupe_key,
                    outcome,
                    &err.to_string(),
                )
                .await;
                return Ok(WebhookResponse::status(status));
            }
        };
        let record = {
            let mut conn = self.services.store().conn().await?;
            conn.webhooks()
                .record(&NewInboundWebhook {
                    source: provider.id().to_string(),
                    dedupe_key,
                    remote_ip: hook.request.remote_ip.map(|ip| ip.to_string()),
                    headers: stored_headers(&hook.request),
                    body: Some(hook.request.body.to_vec()),
                })
                .await?
        };
        if record.duplicate && record.outcome.is_final() {
            return Ok(WebhookResponse {
                reply: parsed.reply,
            });
        }
        if !record.duplicate
            && let Err(err) = self.purge_batch(PURGE_BATCH).await
        {
            tracing::warn!(error = %err, "старые записи вебхуков не удалены");
        }
        let (outcome, payment_id, error, response) =
            match self.apply_outcome(provider.id(), &parsed).await {
                Ok(AcceptResult::Accepted { payment_id, .. })
                | Ok(AcceptResult::AlreadyApplied { payment_id })
                | Ok(AcceptResult::StatusChanged { payment_id, .. }) => (
                    InboxOutcome::Accepted,
                    Some(payment_id),
                    None,
                    WebhookResponse {
                        reply: parsed.reply,
                    },
                ),
                Ok(AcceptResult::Ignored { reason }) => (
                    InboxOutcome::Ignored,
                    None,
                    Some(reason),
                    WebhookResponse {
                        reply: parsed.reply,
                    },
                ),
                Err(ServiceError::NotFound(_))
                | Err(ServiceError::Store(sn_store::StoreError::NotFound(_))) => (
                    InboxOutcome::PaymentNotFound,
                    None,
                    None,
                    WebhookResponse {
                        reply: parsed.reply,
                    },
                ),
                Err(ServiceError::Rejected { code, message }) => (
                    rejection_outcome(code),
                    None,
                    Some(message),
                    WebhookResponse {
                        reply: parsed.reply,
                    },
                ),
                Err(err) => (
                    InboxOutcome::Error,
                    None,
                    Some(err.to_string()),
                    WebhookResponse::status(503),
                ),
            };
        let mut conn = self.services.store().conn().await?;
        conn.webhooks()
            .finish(record.id, outcome, payment_id, error.as_deref())
            .await?;
        Ok(response)
    }

    async fn lookup_gate(
        &self,
        provider: &str,
        request: &WebhookRequest,
    ) -> ServiceResult<Option<Strike>> {
        let identity = format!("{provider}|{}", address_key(request.remote_ip));
        match self
            .services
            .limits()
            .strike(
                WEBHOOK_LOOKUPS,
                &identity,
                "Слишком много уведомлений, не прошедших проверку.",
            )
            .await
        {
            Ok(strike) => Ok(Some(strike)),
            Err(ServiceError::Rejected { code, .. }) if code == RATE_LIMITED => {
                tracing::debug!(
                    provider,
                    address = %address_key(request.remote_ip),
                    "вебхук отклонён до проверки: слишком много непроверенных уведомлений с этого адреса"
                );
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    async fn note_rejection(
        &self,
        provider: &str,
        request: &WebhookRequest,
        digest: &str,
        outcome: InboxOutcome,
        reason: &str,
    ) {
        let ip = request.remote_ip.map(|ip| ip.to_string());
        let identity = format!("{provider}|{}", address_key(request.remote_ip));
        match self
            .services
            .limits()
            .hit(REJECTED_WEBHOOKS, &identity)
            .await
        {
            Ok(_) => {}
            Err(ServiceError::Rejected { code, .. }) if code == RATE_LIMITED => {
                tracing::debug!(
                    provider,
                    ip = ip.as_deref(),
                    outcome = outcome.as_str(),
                    "отказ вебхука не записан: слишком много отказов с этого адреса"
                );
                return;
            }
            Err(err) => {
                tracing::warn!(provider, error = %err, "отказ вебхука не записан");
                return;
            }
        }
        tracing::warn!(
            provider,
            ip = ip.as_deref(),
            outcome = outcome.as_str(),
            error = reason,
            body_sha256 = digest,
            body_len = request.body.len(),
            "вебхук отклонён"
        );
        let error = clip(reason, STORED_TEXT_LIMIT);
        let record = NewInboundWebhook {
            source: provider.to_string(),
            dedupe_key: format!("rejected:{digest}"),
            remote_ip: ip,
            headers: stored_headers(request),
            body: None,
        };
        let stored = match self.services.store().conn().await {
            Ok(mut conn) => conn
                .webhooks()
                .record_rejected(&record, outcome, Some(&error))
                .await
                .map(|_| ()),
            Err(err) => Err(err),
        };
        if let Err(err) = stored {
            tracing::warn!(provider, error = %err, "отказ вебхука не записан");
        }
    }

    async fn purge_batch(&self, limit: i64) -> ServiceResult<u64> {
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .webhooks()
            .purge(
                now - Duration::days(WEBHOOK_RETENTION_DAYS),
                now - Duration::days(REJECTED_WEBHOOK_RETENTION_DAYS),
                limit,
            )
            .await?)
    }

    pub async fn purge_webhooks(&self) -> ServiceResult<u64> {
        let mut total = 0u64;
        for _ in 0..PURGE_ROUNDS {
            let purged = self.purge_batch(PURGE_BATCH).await?;
            total = total.saturating_add(purged);
            if purged == 0 {
                break;
            }
        }
        Ok(total)
    }

    pub async fn apply_outcome(
        &self,
        provider: &str,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        let Some(reference) = &outcome.payment else {
            return Ok(AcceptResult::Ignored {
                reason: "событие без нашего платежа".into(),
            });
        };
        match outcome.status {
            ProviderStatus::Pending => {
                if let (PaymentRef::Payment(id), Some(txid)) =
                    (reference, outcome.provider_txid.as_deref())
                    && let Err(err) = self.track_pending(*id, txid).await
                {
                    tracing::warn!(payment = %id, error = %err, "номер транзакции ожидающего платежа не сохранён");
                }
                Ok(AcceptResult::Ignored {
                    reason: "платёж ещё не завершён".into(),
                })
            }
            ProviderStatus::Paid => self.accept_paid(provider, reference, outcome).await,
            ProviderStatus::Failed => {
                self.accept_failure(provider, reference, outcome, PaymentStatus::Failed)
                    .await
            }
            ProviderStatus::Canceled => {
                self.accept_failure(provider, reference, outcome, PaymentStatus::Canceled)
                    .await
            }
            ProviderStatus::Refunded => {
                self.accept_refund(provider, reference, outcome, None).await
            }
            ProviderStatus::PartiallyRefunded { refunded } => {
                self.accept_refund(provider, reference, outcome, Some(refunded))
                    .await
            }
            ProviderStatus::RefundDeclined => {
                self.decline_refund(provider, reference, outcome).await
            }
        }
    }

    async fn accept_paid(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate(&mut tx, provider, reference).await?;
        if let Some(txid) = repeat_txid(&payment, outcome.provider_txid.as_deref()) {
            let result = self
                .accept_repeat(&mut tx, &payment, txid, outcome.amount)
                .await?;
            tx.commit().await?;
            return Ok(result);
        }
        let check = check_paid_amount(payment.amount, outcome.amount);
        if !check.accepts() {
            tracing::warn!(payment = %payment.id, ?check, "сумма оплаты не сошлась");
            return Err(ServiceError::rejected(
                "payment.amount_mismatch",
                match check {
                    AmountCheck::Missing => "провайдер не подтвердил сумму".to_string(),
                    other => format!("сумма не сошлась: {other:?}"),
                },
            ));
        }
        let result = self
            .settle_paid(
                &mut tx,
                payment,
                outcome.provider_txid.as_deref(),
                outcome.saved_method.as_ref(),
            )
            .await?;
        tx.commit().await?;
        Ok(result)
    }

    async fn accept_repeat(
        &self,
        tx: &mut Tx,
        original: &Payment,
        txid: &str,
        received: Option<Amount>,
    ) -> ServiceResult<AcceptResult> {
        if let Some(existing) = tx
            .payments()
            .by_provider_txid(original.provider.as_str(), txid)
            .await?
        {
            if existing.order_id != original.order_id {
                return Err(ServiceError::rejected(
                    "payment.txid_mismatch",
                    "транзакция провайдера уже привязана к другому заказу",
                ));
            }
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: existing.id,
            });
        }
        let amount = received.filter(|a| a.minor > 0).ok_or_else(|| {
            ServiceError::rejected(
                "payment.amount_mismatch",
                "провайдер не подтвердил сумму повторной оплаты",
            )
        })?;
        let created = tx
            .payments()
            .create(&NewPayment {
                order_id: original.order_id,
                client_id: original.client_id,
                provider: original.provider.clone(),
                method: Some(REPEAT_METHOD.into()),
                amount,
                expires_at: None,
            })
            .await?;
        let paid = tx
            .payments()
            .transition(created.id, PaymentStatus::Paid, Some(txid), None)
            .await?
            .payment()
            .clone();
        let order = tx.orders().lock(original.order_id).await?;
        tracing::warn!(payment = %paid.id, original = %original.id, order = %order.id, "повторная оплата уже оплаченного счёта");
        self.repeat_payment(tx, &order, &paid, Some(original.id))
            .await?;
        tx.events()
            .push(&DomainEvent::PaymentPaid {
                payment_id: paid.id,
                order_id: order.id,
                client_id: paid.client_id,
                order_kind: order.kind,
                amount: paid.amount,
                provider: paid.provider.clone(),
            })
            .await?;
        Ok(AcceptResult::Accepted {
            order_id: order.id,
            payment_id: paid.id,
        })
    }

    async fn settle_paid(
        &self,
        tx: &mut Tx,
        payment: Payment,
        txid: Option<&str>,
        saved: Option<&SavedMethod>,
    ) -> ServiceResult<AcceptResult> {
        let change = match tx
            .payments()
            .transition(payment.id, PaymentStatus::Paid, txid, None)
            .await
        {
            Ok(change) => change,
            Err(StoreError::Invalid(_)) => {
                return Ok(AcceptResult::Ignored {
                    reason: format!("платёж уже {}", payment.status.as_str()),
                });
            }
            Err(StoreError::Conflict { message, .. }) => {
                return Err(ServiceError::rejected("payment.txid_mismatch", message));
            }
            Err(err) => return Err(err.into()),
        };
        let StatusChange::Applied(paid) = change else {
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: payment.id,
            });
        };
        if let Some(method) = saved {
            self.save_method(tx, &paid, method).await?;
        }
        let order = tx.orders().lock(paid.order_id).await?;
        let order = if matches!(order.status, OrderStatus::Pending | OrderStatus::Canceled) {
            let (order, _) = tx.orders().transition(order.id, OrderStatus::Paid).await?;
            if let Some(reason) = self.modules_reject(tx, &order, &paid).await? {
                self.hold_for_review(tx, &order, &paid, &reason).await?;
                order
            } else if order.kind == OrderKind::TopUp {
                credit_topup(tx, &order, &paid).await?;
                let (fulfilled, changed) = tx
                    .orders()
                    .transition(order.id, OrderStatus::Fulfilled)
                    .await?;
                if changed {
                    tx.events()
                        .push(&DomainEvent::OrderFulfilled {
                            order_id: fulfilled.id,
                            client_id: fulfilled.client_id,
                            order_kind: fulfilled.kind,
                            subscription_id: None,
                        })
                        .await?;
                }
                fulfilled
            } else {
                tx.jobs()
                    .enqueue(&JobPayload::FulfillOrder { order_id: order.id }, None)
                    .await?;
                order
            }
        } else {
            let original = tx
                .payments()
                .for_order(order.id)
                .await?
                .into_iter()
                .find(|p| p.id != paid.id && p.status.is_paid_like())
                .map(|p| p.id);
            self.repeat_payment(tx, &order, &paid, original).await?;
            order
        };
        tx.events()
            .push(&DomainEvent::PaymentPaid {
                payment_id: paid.id,
                order_id: order.id,
                client_id: paid.client_id,
                order_kind: order.kind,
                amount: paid.amount,
                provider: paid.provider.clone(),
            })
            .await?;
        Ok(AcceptResult::Accepted {
            order_id: order.id,
            payment_id: paid.id,
        })
    }

    async fn modules_reject(
        &self,
        tx: &mut Tx,
        order: &Order,
        paid: &Payment,
    ) -> ServiceResult<Option<String>> {
        let ctx = self.services.module_context();
        match self
            .services
            .modules()
            .order_paid(&ctx, tx, order, paid)
            .await
        {
            Ok(()) => Ok(None),
            Err(ModuleError::Invalid(reason)) => Ok(Some(reason)),
            Err(err) => Err(err.into()),
        }
    }

    async fn hold_for_review(
        &self,
        tx: &mut Tx,
        order: &Order,
        paid: &Payment,
        reason: &str,
    ) -> ServiceResult<()> {
        let reason = format!("оплата получена, но модуль не подтвердил заказ: {reason}");
        tracing::warn!(order = %order.id, payment = %paid.id, %reason, "оплаченный заказ ждёт ручной проверки");
        tx.orders().mark_review(order.id, &reason).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: "order.review".into(),
                entity_kind: Some(audit_entity::ORDER.into()),
                entity_id: Some(order.id.0),
                details: serde_json::json!({
                    "payment_id": paid.id,
                    "amount": paid.amount,
                    "reason": reason,
                }),
                ip: None,
            })
            .await?;
        Ok(())
    }

    async fn repeat_payment(
        &self,
        tx: &mut Tx,
        order: &Order,
        paid: &Payment,
        original: Option<PaymentId>,
    ) -> ServiceResult<()> {
        let currency = self.services.settings().currency;
        let credit = paid
            .amount
            .as_money()
            .filter(|m| m.currency == currency && m.minor > 0);
        let action = if credit.is_some() {
            RepeatAction::Balance
        } else {
            RepeatAction::Review
        };
        if !tx.payments().mark_repeat(paid.id, original, action).await? {
            return Ok(());
        }
        let shown = amount_text(paid.amount);
        let (audit_action, template, text) = match credit {
            Some(amount) => {
                let appended = tx
                    .ledger()
                    .append(&NewLedgerEntry {
                        client_id: paid.client_id,
                        wallet: Wallet::Main,
                        amount,
                        kind: LedgerKind::TopUp,
                        order_id: Some(order.id),
                        payment_id: Some(paid.id),
                        admin_id: None,
                        comment: Some(format!(
                            "Повторная оплата заказа {}: зачислено на баланс",
                            order.public_id
                        )),
                        idempotency_key: Some(format!("repeat:payment:{}", paid.id)),
                    })
                    .await?;
                if !appended.duplicate {
                    tx.events()
                        .push(&DomainEvent::BalanceChanged {
                            client_id: paid.client_id,
                            wallet: Wallet::Main,
                            kind: LedgerKind::TopUp,
                            delta: amount,
                        })
                        .await?;
                }
                (
                    "payment.repeat_credited",
                    REPEAT_CREDITED_TEMPLATE,
                    format!(
                        "Повторная оплата {shown} по уже оплаченному счёту зачислена на баланс."
                    ),
                )
            }
            None => {
                tracing::warn!(payment = %paid.id, order = %order.id, "повторная оплата ждёт ручной проверки");
                (
                    "payment.repeat_review",
                    REPEAT_REVIEW_TEMPLATE,
                    format!(
                        "Повторная оплата {shown} по уже оплаченному счёту получена. Проверим её вручную и свяжемся с вами."
                    ),
                )
            }
        };
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: audit_action.into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(paid.id.0),
                details: serde_json::json!({
                    "order_id": order.id,
                    "order_status": order.status.as_str(),
                    "amount": paid.amount,
                    "provider_txid": paid.provider_txid,
                    "original_payment_id": original,
                }),
                ip: None,
            })
            .await?;
        tx.jobs()
            .enqueue(
                &JobPayload::Notify {
                    client_id: paid.client_id,
                    template: template.into(),
                    params: serde_json::json!({
                        "text": text,
                        "amount": paid.amount,
                        "payment_id": paid.id,
                        "order_id": order.id,
                    }),
                },
                None,
            )
            .await?;
        Ok(())
    }

    async fn save_method(
        &self,
        tx: &mut Tx,
        paid: &Payment,
        method: &SavedMethod,
    ) -> ServiceResult<()> {
        let Some(secrets) = self.services.secrets() else {
            tracing::warn!(payment = %paid.id, "карта не сохранена: не задан SN_SECRET_KEY");
            return Ok(());
        };
        let provider = paid.provider.as_str();
        let sealed = secrets.seal_str(
            &saved_method_context(paid.client_id, provider),
            &method.token,
        )?;
        tx.saved_methods()
            .save(paid.client_id, provider, &sealed, method.title.as_deref())
            .await?;
        Ok(())
    }

    async fn accept_failure(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
        target: PaymentStatus,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate(&mut tx, provider, reference).await?;
        let change = match tx
            .payments()
            .transition(
                payment.id,
                target,
                outcome.provider_txid.as_deref(),
                Some("по уведомлению провайдера"),
            )
            .await
        {
            Ok(change) => change,
            Err(StoreError::Invalid(_)) | Err(StoreError::Conflict { .. }) => {
                return Ok(AcceptResult::Ignored {
                    reason: format!("платёж уже {}", payment.status.as_str()),
                });
            }
            Err(err) => return Err(err.into()),
        };
        if change.applied() {
            let order = tx.orders().lock(payment.order_id).await?;
            if order.status == OrderStatus::Pending {
                let (canceled, changed) = tx
                    .orders()
                    .transition(order.id, OrderStatus::Canceled)
                    .await?;
                if changed {
                    let ctx = self.services.module_context();
                    self.services
                        .modules()
                        .order_canceled(&ctx, &mut tx, &canceled)
                        .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(AcceptResult::StatusChanged {
            payment_id: change.payment().id,
            status: change.payment().status,
        })
    }

    async fn accept_refund(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
        partial: Option<Amount>,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate_refunded(
            &mut tx,
            provider,
            reference,
            outcome.provider_txid.as_deref(),
        )
        .await?;
        if !payment.status.is_paid_like() {
            return Ok(AcceptResult::Ignored {
                reason: format!("возврат по платежу в статусе {}", payment.status.as_str()),
            });
        }
        let refunds = tx.refunds().for_payment(payment.id).await?;
        if refunds.iter().any(|r| {
            r.provider_refund_id
                .as_deref()
                .is_some_and(|stored| same_refund(stored, &outcome.event_key))
        }) {
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: payment.id,
            });
        }
        let already: i64 = refunds.iter().map(|r| r.amount.minor).sum();
        let remaining = payment.amount.minor - already;
        let requests = tx.payments().provider_refunds(payment.id).await?;
        if let Some(request) =
            matching_request(&requests, &outcome.event_key, partial.or(outcome.amount))
        {
            return self
                .confirm_request(tx, &payment, request, &outcome.event_key, remaining)
                .await;
        }
        let delta = match partial {
            None => remaining,
            Some(amount) if amount.currency == payment.amount.currency => {
                amount.minor.min(remaining)
            }
            Some(_) => {
                return Err(ServiceError::rejected(
                    "payment.refund_currency",
                    "валюта возврата не совпала с валютой платежа",
                ));
            }
        };
        if delta <= 0 {
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: payment.id,
            });
        }
        let recorded = self
            .record_refund(
                &mut tx,
                &payment,
                &RefundSpec {
                    amount: Amount::new(delta, payment.amount.currency),
                    method: RefundMethod::Provider,
                    action: RefundSubscriptionAction::Keep,
                    reason: Some("возврат по уведомлению платёжной системы"),
                    admin: None,
                    provider_refund_id: Some(&outcome.event_key),
                    held: false,
                },
            )
            .await?;
        tx.commit().await?;
        Ok(AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: recorded.status,
        })
    }

    async fn confirm_request(
        &self,
        mut tx: Tx,
        payment: &Payment,
        request: &ProviderRefund,
        event_key: &str,
        remaining: i64,
    ) -> ServiceResult<AcceptResult> {
        let now = self.services.now();
        let minor = request.amount.minor.min(remaining);
        if minor <= 0 {
            tx.payments()
                .resolve_provider_refund(
                    request.id,
                    ProviderRefundStatus::Succeeded,
                    None,
                    None,
                    now,
                )
                .await?;
            self.release_hold(&mut tx, payment, request, request.amount.minor)
                .await?;
            tx.commit().await?;
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: payment.id,
            });
        }
        let refund_key = match request.provider_refund_id.as_deref() {
            Some(id) if same_refund(id, event_key) => id.to_string(),
            _ => event_key.to_string(),
        };
        let recorded = self
            .record_refund(
                &mut tx,
                payment,
                &RefundSpec {
                    amount: Amount::new(minor, payment.amount.currency),
                    method: RefundMethod::Provider,
                    action: request.subscription_action,
                    reason: request.reason.as_deref(),
                    admin: request.admin_id,
                    provider_refund_id: Some(&refund_key),
                    held: request.hold_entry_id.is_some(),
                },
            )
            .await?;
        tx.payments()
            .resolve_provider_refund(
                request.id,
                ProviderRefundStatus::Succeeded,
                Some(recorded.refund_id),
                None,
                now,
            )
            .await?;
        self.release_hold(&mut tx, payment, request, request.amount.minor - minor)
            .await?;
        let acts = request.subscription_action != RefundSubscriptionAction::Keep
            && recorded.order.subscription_id.is_some()
            && tx.payments().arm_refund_action(request.id, now).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: "payment.refund_confirmed".into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(payment.id.0),
                details: serde_json::json!({
                    "request_id": request.id,
                    "refund_id": recorded.refund_id,
                    "amount": Amount::new(minor, payment.amount.currency),
                    "provider_refund_id": refund_key,
                    "requested_by": request.admin_id,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        if acts && let Err(err) = self.run_refund_action(request.id).await {
            tracing::warn!(payment = %payment.id, request = request.id, error = %err, "действие с подпиской после подтверждения возврата отложено до повтора");
        }
        Ok(AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: recorded.status,
        })
    }

    async fn decline_refund(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate_refunded(
            &mut tx,
            provider,
            reference,
            outcome.provider_txid.as_deref(),
        )
        .await?;
        let requests = tx.payments().provider_refunds(payment.id).await?;
        let Some(request) = matching_request(&requests, &outcome.event_key, outcome.amount) else {
            return Ok(AcceptResult::Ignored {
                reason: "отказ по возврату, которого нет среди ожидающих".into(),
            });
        };
        let reason = "платёжная система отклонила возврат";
        tx.payments()
            .resolve_provider_refund(
                request.id,
                ProviderRefundStatus::Declined,
                None,
                Some(reason),
                self.services.now(),
            )
            .await?;
        self.release_hold(&mut tx, &payment, request, request.amount.minor)
            .await?;
        self.resume_delivery(&mut tx, payment.order_id).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: "payment.refund_declined".into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(payment.id.0),
                details: serde_json::json!({
                    "request_id": request.id,
                    "amount": request.amount,
                    "provider_refund_id": request.provider_refund_id,
                    "event_key": outcome.event_key,
                    "requested_by": request.admin_id,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        tracing::warn!(payment = %payment.id, request = request.id, "платёжная система отклонила возврат");
        Ok(AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: payment.status,
        })
    }

    async fn resume_delivery(&self, tx: &mut Tx, order_id: OrderId) -> ServiceResult<()> {
        let order = tx.orders().get(order_id).await?;
        if order.status == OrderStatus::Paid && !tx.orders().refund_pending(order.id).await? {
            let queued = tx
                .jobs()
                .ensure_queued(&JobPayload::FulfillOrder { order_id: order.id })
                .await?;
            if queued.armed() {
                tracing::info!(order = %order.id, job = %queued.id(), "возврат не состоялся: выдача заказа поставлена в очередь");
            }
        }
        Ok(())
    }

    async fn record_refund(
        &self,
        tx: &mut Tx,
        payment: &Payment,
        spec: &RefundSpec<'_>,
    ) -> ServiceResult<RecordedRefund> {
        let already = refunded_minor(tx, payment.id).await?;
        let total = already
            .checked_add(spec.amount.minor)
            .ok_or(sn_domain::DomainError::AmountOverflow)?;
        let status = if total >= payment.amount.minor {
            PaymentStatus::Refunded
        } else {
            PaymentStatus::PartiallyRefunded
        };
        tx.payments()
            .transition(payment.id, status, None, None)
            .await?;
        let refund_id = tx
            .refunds()
            .create(&NewRefund {
                payment_id: payment.id,
                amount: spec.amount,
                method: spec.method,
                subscription_action: spec.action,
                reason: spec.reason.map(str::to_string),
                admin_id: spec.admin,
                provider_refund_id: spec.provider_refund_id.map(str::to_string),
            })
            .await?;
        let order = tx.orders().lock(payment.order_id).await?;
        let repeat = tx.payments().repeat_action(payment.id).await?;
        match spec.method {
            RefundMethod::Balance => {
                let amount = spec.amount.as_money().ok_or_else(|| {
                    ServiceError::rejected(
                        "refund.balance_currency",
                        "звёзды на баланс не возвращаются",
                    )
                })?;
                let appended = tx
                    .ledger()
                    .append(&NewLedgerEntry {
                        client_id: payment.client_id,
                        wallet: Wallet::Main,
                        amount,
                        kind: LedgerKind::Refund,
                        order_id: Some(order.id),
                        payment_id: Some(payment.id),
                        admin_id: None,
                        comment: spec.reason.map(str::to_string),
                        idempotency_key: Some(format!("refund:{refund_id}")),
                    })
                    .await?;
                if !appended.duplicate {
                    tx.events()
                        .push(&DomainEvent::BalanceChanged {
                            client_id: payment.client_id,
                            wallet: Wallet::Main,
                            kind: LedgerKind::Refund,
                            delta: amount,
                        })
                        .await?;
                }
            }
            RefundMethod::Provider if credits_balance(&order, repeat) && !spec.held => {
                self.reverse_topup(tx, payment, &order, spec.amount, refund_id)
                    .await?;
            }
            RefundMethod::Provider => {}
        }
        let order = if status == PaymentStatus::Refunded
            && repeat.is_none()
            && matches!(
                order.status,
                OrderStatus::Paid | OrderStatus::Review | OrderStatus::Fulfilled
            ) {
            tx.orders()
                .transition(order.id, OrderStatus::Refunded)
                .await?
                .0
        } else {
            order
        };
        tx.events()
            .push(&DomainEvent::PaymentRefunded {
                payment_id: payment.id,
                order_id: order.id,
                client_id: payment.client_id,
                amount: spec.amount,
            })
            .await?;
        Ok(RecordedRefund {
            refund_id,
            status,
            order,
        })
    }

    async fn reverse_topup(
        &self,
        tx: &mut Tx,
        payment: &Payment,
        order: &Order,
        amount: Amount,
        refund_id: RefundId,
    ) -> ServiceResult<()> {
        let Some(refunded) = amount.as_money() else {
            return Ok(());
        };
        let balance = tx
            .ledger()
            .balance(payment.client_id, Wallet::Main, refunded.currency)
            .await?;
        let debit = Money::new(balance.minor.min(refunded.minor), refunded.currency);
        if debit.minor > 0 {
            let delta = debit.checked_neg()?;
            let appended = tx
                .ledger()
                .append(&NewLedgerEntry {
                    client_id: payment.client_id,
                    wallet: Wallet::Main,
                    amount: delta,
                    kind: LedgerKind::Purchase,
                    order_id: Some(order.id),
                    payment_id: Some(payment.id),
                    admin_id: None,
                    comment: Some(format!(
                        "Сторно пополнения: возврат платежа {}",
                        payment.public_id
                    )),
                    idempotency_key: Some(format!("topup_reversal:{refund_id}")),
                })
                .await?;
            if !appended.duplicate {
                tx.events()
                    .push(&DomainEvent::BalanceChanged {
                        client_id: payment.client_id,
                        wallet: Wallet::Main,
                        kind: LedgerKind::Purchase,
                        delta,
                    })
                    .await?;
            }
        }
        if debit.minor < refunded.minor {
            tracing::warn!(payment = %payment.id, "возврат пополнения больше остатка на балансе");
            tx.audit()
                .record(&AuditEntry {
                    actor_kind: ActorKind::System,
                    actor_id: None,
                    action: "refund.topup_shortfall".into(),
                    entity_kind: Some("payment".into()),
                    entity_id: Some(payment.id.0),
                    details: serde_json::json!({
                        "refund_id": refund_id,
                        "refunded": refunded,
                        "debited": debit,
                    }),
                    ip: None,
                })
                .await?;
        }
        Ok(())
    }

    async fn hold_refund(
        &self,
        tx: &mut Tx,
        payment: &Payment,
        order: &Order,
        money: Money,
        admin: AdminId,
    ) -> ServiceResult<LedgerEntryId> {
        let spent = || {
            ServiceError::rejected(
                "refund.topup_spent",
                "Клиент уже потратил пополнение: на балансе меньше суммы возврата",
            )
        };
        let unheld = tx
            .payments()
            .unheld_refunds_minor(payment.client_id, money.currency.into())
            .await?;
        let balance = tx
            .ledger()
            .balance(payment.client_id, Wallet::Main, money.currency)
            .await?;
        if balance.minor - unheld < money.minor {
            return Err(spent());
        }
        let delta = money.checked_neg()?;
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: payment.client_id,
                wallet: Wallet::Main,
                amount: delta,
                kind: LedgerKind::RefundHold,
                order_id: Some(order.id),
                payment_id: Some(payment.id),
                admin_id: Some(admin),
                comment: Some(format!(
                    "Возврат пополнения через платёжную систему: платёж {}",
                    payment.public_id
                )),
                idempotency_key: None,
            })
            .await
            .map_err(|err| match err {
                StoreError::InsufficientFunds => spent(),
                other => other.into(),
            })?;
        tx.events()
            .push(&DomainEvent::BalanceChanged {
                client_id: payment.client_id,
                wallet: Wallet::Main,
                kind: LedgerKind::RefundHold,
                delta,
            })
            .await?;
        Ok(appended.entry_id)
    }

    async fn release_hold(
        &self,
        tx: &mut Tx,
        payment: &Payment,
        request: &ProviderRefund,
        minor: i64,
    ) -> ServiceResult<()> {
        if request.hold_entry_id.is_none() || minor <= 0 {
            return Ok(());
        }
        let Some(currency) = request.amount.currency.as_currency() else {
            return Ok(());
        };
        let amount = Money::new(minor, currency);
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: payment.client_id,
                wallet: Wallet::Main,
                amount,
                kind: LedgerKind::RefundRelease,
                order_id: Some(payment.order_id),
                payment_id: Some(payment.id),
                admin_id: None,
                comment: Some(format!(
                    "Возврат платежа {} через платёжную систему не состоялся",
                    payment.public_id
                )),
                idempotency_key: Some(format!("refund_release:{}", request.id)),
            })
            .await?;
        if !appended.duplicate {
            tx.events()
                .push(&DomainEvent::BalanceChanged {
                    client_id: payment.client_id,
                    wallet: Wallet::Main,
                    kind: LedgerKind::RefundRelease,
                    delta: amount,
                })
                .await?;
        }
        Ok(())
    }

    pub async fn refund(
        &self,
        payment_id: PaymentId,
        command: &RefundCommand,
        admin: AdminId,
    ) -> ServiceResult<RefundResult> {
        let currency = self.services.settings().currency;
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let payment = tx.payments().lock(payment_id).await?;
        if !matches!(
            payment.status,
            PaymentStatus::Paid | PaymentStatus::PartiallyRefunded
        ) {
            return Err(ServiceError::rejected(
                "refund.not_paid",
                "Вернуть можно только оплаченный платёж",
            ));
        }
        let already = refunded_minor(&mut tx, payment.id).await?;
        let waiting = pending_minor(&tx.payments().provider_refunds(payment.id).await?);
        let remaining = payment.amount.minor - already - waiting;
        let minor = match command.amount {
            None => remaining,
            Some(amount) if amount.currency == payment.amount.currency => amount.minor,
            Some(_) => {
                return Err(ServiceError::rejected(
                    "refund.currency",
                    "Валюта возврата не совпадает с валютой платежа",
                ));
            }
        };
        if minor <= 0 || minor > remaining {
            if waiting > 0 {
                return Err(ServiceError::rejected(
                    "refund.pending",
                    "Возврат по этому платежу уже ждёт подтверждения платёжной системы",
                ));
            }
            return Err(ServiceError::rejected(
                "refund.amount",
                "Сумма возврата больше оплаченной или равна нулю",
            ));
        }
        let amount = Amount::new(minor, payment.amount.currency);
        let order = tx.orders().get(payment.order_id).await?;
        let credited = credits_balance(&order, tx.payments().repeat_action(payment.id).await?);
        let (provider_refund_id, completed, hold) = match command.method {
            RefundMethod::Balance => {
                if credited {
                    return Err(ServiceError::rejected(
                        "refund.topup_to_balance",
                        "Пополнение возвращается только через платёжную систему",
                    ));
                }
                if amount.as_money().is_none_or(|m| m.currency != currency) {
                    return Err(ServiceError::rejected(
                        "refund.balance_currency",
                        "На баланс возвращаются только суммы в валюте установки",
                    ));
                }
                (None, true, None)
            }
            RefundMethod::Provider => {
                let provider = self
                    .services
                    .payments()
                    .get(payment.provider.as_str())
                    .filter(|p| p.kind() != ProviderKind::Balance && p.supports_refunds())
                    .ok_or_else(|| {
                        ServiceError::rejected(
                            "refund.provider_unsupported",
                            "Эта платёжная система не умеет возвращать деньги",
                        )
                    })?;
                let txid = payment.provider_txid.clone().ok_or_else(|| {
                    ServiceError::rejected(
                        "refund.no_txid",
                        "У платежа нет номера транзакции в платёжной системе",
                    )
                })?;
                let hold = match amount.as_money() {
                    Some(money) if credited => Some(
                        self.hold_refund(&mut tx, &payment, &order, money, admin)
                            .await?,
                    ),
                    _ => None,
                };
                let config = self.services.payments().config(provider.id());
                let outcome = provider
                    .refund(
                        &config,
                        &RefundRequest {
                            payment_id: payment.id,
                            provider_txid: txid,
                            amount,
                            reason: command.reason.clone(),
                        },
                    )
                    .await?;
                (outcome.provider_refund_id, outcome.completed, hold)
            }
        };
        if !completed {
            let request = tx
                .payments()
                .create_provider_refund(&NewProviderRefund {
                    payment_id: payment.id,
                    amount,
                    subscription_action: command.subscription_action,
                    reason: command.reason.clone(),
                    admin_id: Some(admin),
                    provider_refund_id: provider_refund_id.clone(),
                    hold_entry_id: hold,
                    requested_at: now,
                })
                .await?;
            tx.audit()
                .record(&AuditEntry {
                    actor_kind: ActorKind::Admin,
                    actor_id: Some(admin.0),
                    action: "payment.refund_requested".into(),
                    entity_kind: Some("payment".into()),
                    entity_id: Some(payment.id.0),
                    details: serde_json::json!({
                        "request_id": request.id,
                        "amount": amount,
                        "method": command.method,
                        "subscription_action": command.subscription_action,
                        "provider_refund_id": provider_refund_id,
                        "hold_entry_id": hold,
                    }),
                    ip: None,
                })
                .await?;
            tx.jobs()
                .enqueue(
                    &JobPayload::PollPayment {
                        payment_id: payment.id,
                    },
                    Some(now + Duration::seconds(REFUND_POLL_DELAY_SECONDS)),
                )
                .await?;
            tx.commit().await?;
            return Ok(RefundResult {
                refund_id: None,
                request_id: Some(request.id),
                pending: true,
                payment_id: payment.id,
                amount,
                status: payment.status,
                subscription_error: None,
            });
        }
        let recorded = self
            .record_refund(
                &mut tx,
                &payment,
                &RefundSpec {
                    amount,
                    method: command.method,
                    action: command.subscription_action,
                    reason: command.reason.as_deref(),
                    admin: Some(admin),
                    provider_refund_id: provider_refund_id.as_deref(),
                    held: hold.is_some(),
                },
            )
            .await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::Admin,
                actor_id: Some(admin.0),
                action: "payment.refund".into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(payment.id.0),
                details: serde_json::json!({
                    "refund_id": recorded.refund_id,
                    "amount": amount,
                    "method": command.method,
                    "subscription_action": command.subscription_action,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        let subscription_error = match self
            .refund_subscription_action(&recorded.order, command.subscription_action, Some(admin))
            .await
        {
            Ok(()) => None,
            Err(err) => {
                tracing::warn!(payment = %payment.id, error = %err, "действие с подпиской после возврата не выполнено");
                Some(err.to_string())
            }
        };
        Ok(RefundResult {
            refund_id: Some(recorded.refund_id),
            request_id: None,
            pending: false,
            payment_id: payment.id,
            amount,
            status: recorded.status,
            subscription_error,
        })
    }

    pub async fn mark_paid(
        &self,
        payment_id: PaymentId,
        admin: AdminId,
        note: Option<&str>,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = tx.payments().lock(payment_id).await?;
        match payment.provider.as_str() {
            providers::STARS => {
                return Err(ServiceError::rejected(
                    "payment.manual_stars",
                    "Оплату звёздами подтверждает только Telegram",
                ));
            }
            providers::BALANCE => {
                return Err(ServiceError::rejected(
                    "payment.manual_balance",
                    "Оплата с баланса проводится без ручной отметки",
                ));
            }
            _ => {}
        }
        let result = self.settle_paid(&mut tx, payment, None, None).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::Admin,
                actor_id: Some(admin.0),
                action: "payment.mark_paid".into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(payment_id.0),
                details: serde_json::json!({ "note": note }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn poll_payment(&self, payment_id: PaymentId) -> ServiceResult<PollResult> {
        let payment = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().get(payment_id).await?
        };
        if payment.status.is_paid_like() {
            return self.poll_refunds_of(&payment).await;
        }
        if payment.status != PaymentStatus::Pending {
            return Ok(PollResult::Settled);
        }
        if payment
            .expires_at
            .is_some_and(|at| at < self.services.now())
        {
            return Ok(PollResult::Settled);
        }
        if self
            .services
            .payments()
            .get(payment.provider.as_str())
            .is_none_or(|p| p.kind() != ProviderKind::External)
        {
            return Ok(PollResult::Unsupported);
        }
        let Some(txid) = payment.provider_txid.clone() else {
            return Ok(PollResult::Pending);
        };
        self.confirm_by_txid(payment.id, &txid).await
    }

    pub async fn track_pending(
        &self,
        payment_id: PaymentId,
        provider_txid: &str,
    ) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        if conn
            .payments()
            .remember_txid(payment_id, provider_txid)
            .await?
        {
            return Ok(());
        }
        let payment = conn.payments().get(payment_id).await?;
        match payment.provider_txid.as_deref() {
            Some(known) if known == provider_txid => Ok(()),
            Some(known) => Err(ServiceError::rejected(
                "payment.txid_mismatch",
                format!("у платежа уже другая транзакция провайдера: {known}"),
            )),
            None => Err(ServiceError::rejected(
                "payment.not_pending",
                format!(
                    "номер транзакции запоминается только у неоплаченного платежа, сейчас {}",
                    payment.status.as_str()
                ),
            )),
        }
    }

    pub async fn confirm_by_txid(
        &self,
        payment_id: PaymentId,
        provider_txid: &str,
    ) -> ServiceResult<PollResult> {
        let payment = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().get(payment_id).await?
        };
        if payment.status.is_paid_like() {
            return Ok(PollResult::Settled);
        }
        if let Some(known) = payment.provider_txid.as_deref()
            && known != provider_txid
        {
            return Err(ServiceError::rejected(
                "payment.txid_mismatch",
                format!("у платежа уже другая транзакция провайдера: {known}"),
            ));
        }
        let Some(provider) = self
            .services
            .payments()
            .get(payment.provider.as_str())
            .filter(|p| p.kind() == ProviderKind::External)
        else {
            return Ok(PollResult::Unsupported);
        };
        let config = self.services.payments().config(provider.id());
        let outcome = match provider.fetch_status(&config, provider_txid).await {
            Ok(Some(outcome)) => outcome,
            Ok(None)
            | Err(PaymentError::NotImplemented(_))
            | Err(PaymentError::Unsupported(_))
            | Err(PaymentError::NotConfigured(_)) => return Ok(PollResult::Unsupported),
            Err(err) => return Err(err.into()),
        };
        if outcome.status == ProviderStatus::Pending {
            self.track_pending(payment.id, provider_txid).await?;
            return Ok(PollResult::Pending);
        }
        let outcome = WebhookOutcome {
            payment: Some(PaymentRef::Payment(payment.id)),
            provider_txid: Some(provider_txid.to_string()),
            ..outcome
        };
        self.apply_outcome(provider.id(), &outcome).await?;
        Ok(PollResult::Settled)
    }

    async fn poll_refunds_of(&self, payment: &Payment) -> ServiceResult<PollResult> {
        let requests = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().provider_refunds(payment.id).await?
        };
        let mut waiting = false;
        let mut polled = false;
        for request in pending_requests(&requests) {
            match self.poll_refund(payment, request).await? {
                RefundPoll::Settled => polled = true,
                RefundPoll::Waiting => {
                    polled = true;
                    waiting = true;
                }
                RefundPoll::Unsupported => {}
            }
        }
        Ok(if waiting {
            PollResult::Pending
        } else if polled || pending_requests(&requests).next().is_none() {
            PollResult::Settled
        } else {
            PollResult::Unsupported
        })
    }

    async fn poll_refund(
        &self,
        payment: &Payment,
        request: &ProviderRefund,
    ) -> ServiceResult<RefundPoll> {
        let Some(refund_id) = request.provider_refund_id.as_deref() else {
            return Ok(RefundPoll::Unsupported);
        };
        let Some(provider) = self.services.payments().get(payment.provider.as_str()) else {
            return Ok(RefundPoll::Unsupported);
        };
        let config = self.services.payments().config(provider.id());
        let outcome = match provider.fetch_refund(&config, refund_id).await {
            Ok(Some(outcome)) => outcome,
            Ok(None)
            | Err(PaymentError::NotImplemented(_))
            | Err(PaymentError::Unsupported(_))
            | Err(PaymentError::NotConfigured(_)) => return Ok(RefundPoll::Unsupported),
            Err(err) => return Err(err.into()),
        };
        match outcome.status {
            ProviderStatus::Refunded
            | ProviderStatus::PartiallyRefunded { .. }
            | ProviderStatus::RefundDeclined => {
                let outcome = WebhookOutcome {
                    payment: Some(PaymentRef::Payment(payment.id)),
                    provider_txid: payment.provider_txid.clone(),
                    ..outcome
                };
                self.apply_outcome(provider.id(), &outcome).await?;
                Ok(RefundPoll::Settled)
            }
            _ => Ok(RefundPoll::Waiting),
        }
    }

    pub async fn poll_refunds(&self, limit: i64) -> ServiceResult<RefundPollReport> {
        let now = self.services.now();
        let quiet_since = now - Duration::seconds(REFUND_POLL_DELAY_SECONDS);
        let pollable: Vec<String> = self
            .services
            .payments()
            .providers()
            .iter()
            .filter(|p| p.supports_refund_polling())
            .map(|p| p.id().to_string())
            .collect();
        let requests = {
            let mut conn = self.services.store().conn().await?;
            conn.payments()
                .claim_refunds_to_poll(&pollable, quiet_since, quiet_since, now, limit)
                .await?
        };
        let mut report = RefundPollReport::default();
        for request in &requests {
            let polled = match self.payment(request.payment_id).await {
                Ok(payment) => self.poll_refund(&payment, request).await,
                Err(err) => Err(err),
            };
            match polled {
                Ok(RefundPoll::Settled) => report.settled += 1,
                Ok(RefundPoll::Waiting) => report.waiting += 1,
                Ok(RefundPoll::Unsupported) => report.unsupported += 1,
                Err(err) => {
                    tracing::warn!(request = request.id, payment = %request.payment_id, error = %err, "статус возврата не получен");
                    report.failed += 1;
                }
            }
        }
        let actions = self.retry_refund_actions(limit).await?;
        report.actions_done = actions.done;
        report.actions_retry = actions.retry;
        report.actions_failed = actions.failed;
        Ok(report)
    }

    async fn payment(&self, payment_id: PaymentId) -> ServiceResult<Payment> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.payments().get(payment_id).await?)
    }

    pub async fn stars_pre_checkout(&self, query: &StarsPreCheckout) -> ServiceResult<()> {
        let public_id = Uuid::parse_str(&query.invoice_payload)
            .map_err(|_| ServiceError::rejected("stars.unknown_invoice", "счёт не найден"))?;
        let mut conn = self.services.store().conn().await?;
        let payment = conn.payments().by_public_id(public_id).await?;
        if payment.provider.as_str() != providers::STARS
            || payment.status != PaymentStatus::Pending
            || query.currency != "XTR"
            || payment.amount.minor != query.total_amount
            || payment
                .expires_at
                .is_some_and(|at| at < self.services.now())
        {
            return Err(ServiceError::rejected(
                "stars.invoice_mismatch",
                "счёт устарел, оформите заново",
            ));
        }
        let owner = conn
            .clients()
            .find_by_identity(
                sn_domain::client::IdentityKind::Telegram,
                &query.telegram_user_id.to_string(),
            )
            .await?;
        if owner.as_ref().map(|c| c.id) != Some(payment.client_id) {
            return Err(ServiceError::rejected(
                "stars.foreign_invoice",
                "счёт выставлен другому аккаунту",
            ));
        }
        if owner.is_some_and(|c| c.is_blocked()) {
            return Err(ServiceError::Forbidden);
        }
        Ok(())
    }

    pub async fn stars_payment(&self, payment: &StarsPayment) -> ServiceResult<AcceptResult> {
        let provider = self
            .services
            .payments()
            .get(providers::STARS)
            .ok_or(ServiceError::NotFound("провайдер stars"))?;
        let request = WebhookRequest {
            headers: Vec::new(),
            body: serde_json::to_vec(payment)
                .map_err(|e| ServiceError::Internal(e.to_string()))?
                .into(),
            remote_ip: None,
            route_suffix: None,
            query: None,
        };
        let outcome = provider
            .parse_webhook(&self.services.payments().config(providers::STARS), &request)
            .await?;
        self.apply_outcome(providers::STARS, &outcome).await
    }
}
