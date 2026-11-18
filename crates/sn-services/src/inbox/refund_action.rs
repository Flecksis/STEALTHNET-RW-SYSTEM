use super::{PaymentInbox, REFUND_REQUESTED_AUDIT, STORED_TEXT_LIMIT, clip};
use crate::error::{ServiceError, ServiceResult};
use crate::subscriptions::{disable_remna_user, with_subscription_lock};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::events::DomainEvent;
use sn_domain::ids::{AdminId, ClientId, OrderId, PaymentId, SubscriptionId};
use sn_domain::money::Amount;
use sn_domain::order::Order;
use sn_domain::payment::RefundSubscriptionAction;
use sn_domain::plan::PlanAction;
use sn_remnawave::{DropConnectionsRequest, UpdateUserRequest};
use sn_store::{
    ActorKind, AuditEntry, Db, FailedRefundAction, ProviderRefund, RefundActionStatus, audit_entity,
};

pub const REFUND_ACTION_ATTEMPTS: u32 = 12;
pub const REFUND_ACTION_FAILED: &str = "subscription.refund_action_failed";

const RETRY_BASE_SECONDS: i64 = 60;
const RETRY_MAX_SECONDS: i64 = 3600;
const ACTION_CONFLICT: &str = "refund.action_conflict";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundActionOutcome {
    Done,
    Retry,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefundActionReview {
    pub request_id: i64,
    pub payment_id: PaymentId,
    pub order_id: OrderId,
    pub client_id: ClientId,
    pub subscription_id: Option<SubscriptionId>,
    pub action: RefundSubscriptionAction,
    pub amount: Amount,
    pub attempts: u32,
    pub error: Option<String>,
    pub requested_by: Option<AdminId>,
    pub confirmed_at: Option<DateTime<Utc>>,
}

impl From<FailedRefundAction> for RefundActionReview {
    fn from(failed: FailedRefundAction) -> Self {
        RefundActionReview {
            request_id: failed.request.id,
            payment_id: failed.request.payment_id,
            order_id: failed.order_id,
            client_id: failed.client_id,
            subscription_id: failed.subscription_id,
            action: failed.request.subscription_action,
            amount: failed.request.amount,
            attempts: failed.request.action.attempts,
            error: failed.request.action.error,
            requested_by: failed.request.admin_id,
            confirmed_at: failed.request.resolved_at,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ActionRound {
    pub done: usize,
    pub retry: usize,
    pub failed: usize,
}

pub(super) fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    Duration::seconds((RETRY_BASE_SECONDS << doublings).min(RETRY_MAX_SECONDS))
}

fn whole_seconds(at: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp(at.timestamp(), 0).unwrap_or(at)
}

fn same_moment(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    a.timestamp_millis() == b.timestamp_millis()
}

pub(super) fn cut_target(current: DateTime<Utc>, days: u32, now: DateTime<Utc>) -> DateTime<Utc> {
    let wanted = (current - Duration::days(i64::from(days))).max(now);
    if wanted >= current {
        current
    } else {
        whole_seconds(wanted)
    }
}

fn bought_meanwhile(action: RefundSubscriptionAction, order: OrderId) -> ServiceError {
    let kept = match action {
        RefundSubscriptionAction::Cut => "срок подписки не укоротили",
        _ => "подписку не отключили",
    };
    ServiceError::rejected(
        ACTION_CONFLICT,
        format!(
            "После запроса возврата по подписке прошла новая оплата (заказ №{order}), \
             поэтому {kept}. Решите вручную и закройте заявку"
        ),
    )
}

fn subject(order: &Order, request: &ProviderRefund) -> (&'static str, i64) {
    match order.subscription_id {
        Some(id) => (audit_entity::SUBSCRIPTION, id.0),
        None => (audit_entity::PAYMENT, request.payment_id.0),
    }
}

impl PaymentInbox<'_> {
    pub(super) async fn refund_subscription_action(
        &self,
        order: &Order,
        action: RefundSubscriptionAction,
        admin: Option<AdminId>,
    ) -> ServiceResult<()> {
        if action == RefundSubscriptionAction::Keep {
            return Ok(());
        }
        let Some(subscription_id) = order.subscription_id else {
            return Ok(());
        };
        with_subscription_lock(
            self.services,
            subscription_id,
            self.apply_refund_action(order, subscription_id, action, admin, None),
        )
        .await
    }

    async fn planned_cut(
        &self,
        request: &ProviderRefund,
        current: DateTime<Utc>,
        days: u32,
    ) -> ServiceResult<Option<DateTime<Utc>>> {
        let (base, target) = match (
            request.action.base_expire_at,
            request.action.target_expire_at,
        ) {
            (Some(base), Some(target)) => (base, target),
            _ => {
                let target = cut_target(current, days, self.services.now());
                let mut conn = self.services.store().conn().await?;
                conn.payments()
                    .plan_refund_action(request.id, current, target)
                    .await?
            }
        };
        if same_moment(current, target) {
            Ok(None)
        } else if same_moment(current, base) {
            Ok(Some(target))
        } else {
            Err(ServiceError::rejected(
                ACTION_CONFLICT,
                "Срок подписки изменился после расчёта: укоротите срок вручную",
            ))
        }
    }

    async fn apply_refund_action(
        &self,
        order: &Order,
        subscription_id: SubscriptionId,
        action: RefundSubscriptionAction,
        admin: Option<AdminId>,
        request: Option<&ProviderRefund>,
    ) -> ServiceResult<()> {
        let subscription = {
            let mut conn = self.services.store().conn().await?;
            conn.subscriptions().get(subscription_id).await?
        };
        let Some(link) = subscription.remna.clone() else {
            return Ok(());
        };
        let remna = self.services.remna()?;
        if let Some(request) = request {
            let newer = {
                let mut conn = self.services.store().conn().await?;
                conn.payments()
                    .paid_after_refund(request.id, REFUND_REQUESTED_AUDIT)
                    .await?
            };
            if let Some(newer) = newer {
                return Err(bought_meanwhile(action, newer));
            }
        }
        let user = match action {
            RefundSubscriptionAction::Cut => {
                let current = remna.get_user(link.user_id).await?;
                if !self.order_delivered(order, current.expire_at).await? {
                    return self
                        .skip_undelivered_cut(order, subscription_id, admin, request)
                        .await;
                }
                let target = match request {
                    Some(request) => {
                        self.planned_cut(request, current.expire_at, order.quote.days)
                            .await?
                    }
                    None => Some(cut_target(
                        current.expire_at,
                        order.quote.days,
                        self.services.now(),
                    ))
                    .filter(|target| !same_moment(*target, current.expire_at)),
                };
                match target {
                    Some(expire_at) => {
                        let mut update = UpdateUserRequest::new(link.user_id);
                        update.expire_at = Some(expire_at);
                        remna.update_user(&update).await?
                    }
                    None => current,
                }
            }
            RefundSubscriptionAction::Disable => {
                let user = disable_remna_user(&*remna, link.user_id).await?;
                remna
                    .drop_connections(&DropConnectionsRequest::users_everywhere(vec![
                        link.user_id,
                    ]))
                    .await?;
                user
            }
            RefundSubscriptionAction::Keep => return Ok(()),
        };
        let mut tx = self.services.store().begin().await?;
        let mirror = user.mirror(subscription.mirror.devices_count, self.services.now());
        tx.subscriptions()
            .apply_mirror(subscription.id, &mirror)
            .await?;
        if let (Some(from), Some(to)) = (subscription.mirror.status, mirror.status)
            && from != to
        {
            tx.events()
                .push(&DomainEvent::SubscriptionStatusChanged {
                    subscription_id: subscription.id,
                    client_id: subscription.client_id,
                    from: Some(from),
                    to,
                })
                .await?;
        }
        tx.audit()
            .record(&AuditEntry {
                actor_kind: if admin.is_some() {
                    ActorKind::Admin
                } else {
                    ActorKind::System
                },
                actor_id: admin.map(|a| a.0),
                action: "subscription.refund_action".into(),
                entity_kind: Some(audit_entity::SUBSCRIPTION.into()),
                entity_id: Some(subscription.id.0),
                details: serde_json::json!({
                    "order_id": order.id,
                    "action": action,
                    "request_id": request.map(|r| r.id),
                    "previous_expire_at": subscription.mirror.expire_at,
                    "expire_at": user.expire_at,
                }),
                ip: None,
            })
            .await?;
        if let Some(request) = request {
            tx.payments().finish_refund_action(request.id).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn order_delivered(&self, order: &Order, current: DateTime<Utc>) -> ServiceResult<bool> {
        let (fresh, state) = {
            let mut conn = self.services.store().conn().await?;
            let fresh = conn.orders().get(order.id).await?;
            let state = conn.orders().plan_state(order.id).await?;
            (fresh, state)
        };
        if fresh.fulfilled_at.is_some() || state.applied_at.is_some() {
            return Ok(true);
        }
        let Some(plan) = fresh.plan else {
            return Ok(false);
        };
        let (PlanAction::Update { .. }, Some(base)) = (&plan.action, plan.base_expire_at) else {
            return Ok(true);
        };
        if same_moment(current, plan.target.expire_at) {
            Ok(true)
        } else if same_moment(current, base) {
            Ok(false)
        } else {
            Err(ServiceError::rejected(
                ACTION_CONFLICT,
                "Не ясно, успел ли заказ продлить подписку: проверьте срок и укоротите его вручную",
            ))
        }
    }

    async fn skip_undelivered_cut(
        &self,
        order: &Order,
        subscription_id: SubscriptionId,
        admin: Option<AdminId>,
        request: Option<&ProviderRefund>,
    ) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: if admin.is_some() {
                    ActorKind::Admin
                } else {
                    ActorKind::System
                },
                actor_id: admin.map(|a| a.0),
                action: "subscription.refund_action".into(),
                entity_kind: Some(audit_entity::SUBSCRIPTION.into()),
                entity_id: Some(subscription_id.0),
                details: serde_json::json!({
                    "order_id": order.id,
                    "action": RefundSubscriptionAction::Cut,
                    "request_id": request.map(|r| r.id),
                    "applied": false,
                    "reason": "order_not_delivered",
                }),
                ip: None,
            })
            .await?;
        if let Some(request) = request {
            tx.payments().finish_refund_action(request.id).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn action_order(&self, request: &ProviderRefund) -> ServiceResult<Order> {
        let mut conn = self.services.store().conn().await?;
        let payment = conn.payments().get(request.payment_id).await?;
        Ok(conn.orders().get(payment.order_id).await?)
    }

    pub(super) async fn run_refund_action(
        &self,
        request_id: i64,
    ) -> ServiceResult<RefundActionOutcome> {
        let request = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().provider_refund(request_id).await?
        };
        if request.action.status != RefundActionStatus::Pending {
            return Ok(RefundActionOutcome::Skipped);
        }
        let order = self.action_order(&request).await?;
        let Some(subscription_id) = order.subscription_id else {
            let mut conn = self.services.store().conn().await?;
            conn.payments().finish_refund_action(request.id).await?;
            return Ok(RefundActionOutcome::Done);
        };
        with_subscription_lock(
            self.services,
            subscription_id,
            self.attempt_refund_action(request_id, &order, subscription_id),
        )
        .await
    }

    async fn attempt_refund_action(
        &self,
        request_id: i64,
        order: &Order,
        subscription_id: SubscriptionId,
    ) -> ServiceResult<RefundActionOutcome> {
        let request = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().provider_refund(request_id).await?
        };
        if request.action.status != RefundActionStatus::Pending {
            return Ok(RefundActionOutcome::Skipped);
        }
        let applied = self
            .apply_refund_action(
                order,
                subscription_id,
                request.subscription_action,
                request.admin_id,
                Some(&request),
            )
            .await;
        match applied {
            Ok(()) => {
                let mut conn = self.services.store().conn().await?;
                conn.payments().finish_refund_action(request.id).await?;
                Ok(RefundActionOutcome::Done)
            }
            Err(err) => self.note_action_failure(&request, order, &err).await,
        }
    }

    async fn note_action_failure(
        &self,
        request: &ProviderRefund,
        order: &Order,
        err: &ServiceError,
    ) -> ServiceResult<RefundActionOutcome> {
        let now = self.services.now();
        let attempts = request.action.attempts.saturating_add(1);
        let conflict =
            matches!(err, ServiceError::Rejected { code, .. } if *code == ACTION_CONFLICT);
        let gave_up = conflict || attempts >= REFUND_ACTION_ATTEMPTS;
        let retry_at = (!gave_up).then(|| now + retry_delay(attempts));
        let error = clip(&err.to_string(), STORED_TEXT_LIMIT);
        let mut tx = self.services.store().begin().await?;
        if !tx
            .payments()
            .fail_refund_action(request.id, &error, retry_at)
            .await?
        {
            return Ok(RefundActionOutcome::Skipped);
        }
        if gave_up {
            let (entity_kind, entity_id) = subject(order, request);
            tx.audit()
                .record(&AuditEntry {
                    actor_kind: ActorKind::System,
                    actor_id: None,
                    action: REFUND_ACTION_FAILED.into(),
                    entity_kind: Some(entity_kind.into()),
                    entity_id: Some(entity_id),
                    details: serde_json::json!({
                        "request_id": request.id,
                        "payment_id": request.payment_id,
                        "order_id": order.id,
                        "action": request.subscription_action,
                        "attempts": attempts,
                        "error": error,
                        "requested_by": request.admin_id,
                    }),
                    ip: None,
                })
                .await?;
        }
        tx.commit().await?;
        if gave_up {
            tracing::error!(
                request = request.id,
                payment = %request.payment_id,
                order = %order.id,
                attempts,
                error = %err,
                "действие с подпиской после возврата не выполнено: нужно ручное действие"
            );
            Ok(RefundActionOutcome::Failed)
        } else {
            tracing::warn!(
                request = request.id,
                payment = %request.payment_id,
                attempts,
                error = %err,
                "действие с подпиской после возврата не выполнено, будет повтор"
            );
            Ok(RefundActionOutcome::Retry)
        }
    }

    pub(super) async fn retry_refund_actions(&self, limit: i64) -> ServiceResult<ActionRound> {
        let due = {
            let mut conn = self.services.store().conn().await?;
            conn.payments()
                .due_refund_actions(self.services.now(), limit)
                .await?
        };
        let mut round = ActionRound::default();
        for request in &due {
            match self.run_refund_action(request.id).await {
                Ok(RefundActionOutcome::Done) => round.done += 1,
                Ok(RefundActionOutcome::Retry) => round.retry += 1,
                Ok(RefundActionOutcome::Failed) => round.failed += 1,
                Ok(RefundActionOutcome::Skipped) => {}
                Err(err) => {
                    tracing::warn!(request = request.id, error = %err, "повтор действия с подпиской после возврата сорвался");
                    round.retry += 1;
                }
            }
        }
        Ok(round)
    }

    pub async fn refund_actions_review(
        &self,
        limit: i64,
    ) -> ServiceResult<Vec<RefundActionReview>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .payments()
            .failed_refund_actions(limit.clamp(1, 500))
            .await?
            .into_iter()
            .map(RefundActionReview::from)
            .collect())
    }

    pub async fn retry_refund_action(
        &self,
        request_id: i64,
        admin: AdminId,
    ) -> ServiceResult<RefundActionOutcome> {
        let mut tx = self.services.store().begin().await?;
        let request = tx.payments().provider_refund(request_id).await?;
        if request.action.status != RefundActionStatus::Failed
            || !tx
                .payments()
                .arm_refund_action(request_id, self.services.now())
                .await?
        {
            return Err(ServiceError::rejected(
                "refund.action_not_failed",
                "Повторить можно только действие, которое ждёт ручного решения",
            ));
        }
        let payment = tx.payments().get(request.payment_id).await?;
        let order = tx.orders().get(payment.order_id).await?;
        let (entity_kind, entity_id) = subject(&order, &request);
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::Admin,
                actor_id: Some(admin.0),
                action: "subscription.refund_action_retry".into(),
                entity_kind: Some(entity_kind.into()),
                entity_id: Some(entity_id),
                details: serde_json::json!({
                    "request_id": request.id,
                    "payment_id": request.payment_id,
                    "action": request.subscription_action,
                    "previous_error": request.action.error,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        self.run_refund_action(request_id).await
    }

    pub async fn close_refund_action(
        &self,
        request_id: i64,
        admin: AdminId,
        note: &str,
    ) -> ServiceResult<()> {
        let note = clip(note.trim(), STORED_TEXT_LIMIT);
        if note.is_empty() {
            return Err(ServiceError::rejected(
                "refund.action_note",
                "Напишите, что сделано с подпиской",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let request = tx.payments().provider_refund(request_id).await?;
        if !tx.payments().finish_refund_action(request_id).await? {
            return Err(ServiceError::rejected(
                "refund.action_closed",
                "У этой заявки нет незавершённого действия с подпиской",
            ));
        }
        let payment = tx.payments().get(request.payment_id).await?;
        let order = tx.orders().get(payment.order_id).await?;
        let (entity_kind, entity_id) = subject(&order, &request);
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::Admin,
                actor_id: Some(admin.0),
                action: "subscription.refund_action_closed".into(),
                entity_kind: Some(entity_kind.into()),
                entity_id: Some(entity_id),
                details: serde_json::json!({
                    "request_id": request.id,
                    "payment_id": request.payment_id,
                    "action": request.subscription_action,
                    "attempts": request.action.attempts,
                    "error": request.action.error,
                    "note": note,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
