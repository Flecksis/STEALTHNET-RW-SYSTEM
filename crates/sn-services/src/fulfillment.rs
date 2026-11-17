use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use crate::subscriptions::{
    actions, disable_remna_user, drop_everywhere, enable_remna_user, with_subscription_lock,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::client::{ClientStatus, IdentityKind};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{CategoryId, ClientId, JobId, OrderId, RemnaUserId, SquadId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::module::ModuleId;
use sn_domain::order::{Order, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::plan::{
    CurrentTariff, FulfillmentPlan, HeldDevices, NewUserProfile, PlanAction, PlanInput, PlanKind,
    classify, plan,
};
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, Subscription, SubscriptionKind, TRIAL_TAG, remna_description,
    remna_username,
};
use sn_domain::tariff::TariffSnapshot;
use sn_modules::ModuleError;
use sn_remnawave::{
    CreateUserRequest, RemnaError, RemnaUser, RemnawaveApi, UpdateUserRequest, UserStatus,
};
use sn_store::{
    Db, NewSubscription, PlanStage, PurchaseApplied, ReviewJob, ReviewOrder, StoreError, Tx,
    audit_entity,
};
use sqlx::types::Json;
use std::collections::HashSet;
use std::time::Duration;

#[cfg(test)]
mod tests;

const MIN_RETRY: Duration = Duration::from_secs(15);
const REFUND_WAIT: Duration = Duration::from_secs(300);
const USERNAME_TAKEN: &str = "A019";
const FINISH_ATTEMPTS: usize = 3;
const REFUND_PENDING_REASON: &str =
    "по платежу заказа ждём решения платёжной системы о возврате: выдача отложена";

pub const MAX_FULFILL_NOTE: usize = 500;
pub const REVIEW_ORDERS_LIMIT: i64 = 50;
pub const REVIEW_ORDERS_MAX: i64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Redeliver,
    Recalculate,
    MarkFulfilled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewItem {
    #[serde(flatten)]
    pub order: ReviewOrder,
    pub actions: Vec<ReviewAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Redelivery {
    pub order_id: OrderId,
    pub job_id: JobId,
    pub state: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Recalculation {
    pub order_id: OrderId,
    pub job_id: JobId,
    pub state: &'static str,
    pub plan: PlanStage,
}

pub fn review_actions(order: &ReviewOrder) -> Vec<ReviewAction> {
    let mut actions = Vec::new();
    if !order.refund_pending {
        if order.plan != PlanStage::Stale {
            actions.push(ReviewAction::Redeliver);
        }
        let running = order.job.as_ref().is_some_and(ReviewJob::active);
        let resettable = match order.plan {
            PlanStage::None => false,
            PlanStage::Stale => true,
            PlanStage::Saved | PlanStage::Applied => !running,
        };
        if order.kind.needs_remnawave() && resettable {
            actions.push(ReviewAction::Recalculate);
        }
    }
    actions.push(ReviewAction::MarkFulfilled);
    actions
}

pub fn abandoned_reason(attempts: u32, error: &str) -> String {
    format!(
        "выдача не завершилась за {attempts} попыток: {error}. План мог быть отправлен в Remnawave: \
         проверьте подписку и выберите «Пересчитать и выдать» или «Отметить выданным»"
    )
}

pub async fn holds_autorenew(db: &mut impl Db, order_id: OrderId) -> ServiceResult<bool> {
    Ok(db
        .orders()
        .holds_renewal(order_id, crate::autorenew::AUTORENEW_METHOD)
        .await?)
}

fn not_deliverable(status: OrderStatus) -> ServiceError {
    let message = match status {
        OrderStatus::Fulfilled => "Заказ уже выдан",
        OrderStatus::Refunded => "Заказ возвращён, выдавать его нельзя",
        OrderStatus::Pending | OrderStatus::Canceled => "Заказ не оплачен",
        OrderStatus::Paid | OrderStatus::Review => "Заказ нельзя выдать повторно",
    };
    ServiceError::rejected("order.redeliver_status", message)
}

fn not_in_review(status: OrderStatus) -> ServiceError {
    let message = match status {
        OrderStatus::Fulfilled => "Заказ уже выдан",
        OrderStatus::Refunded => "Заказ возвращён",
        OrderStatus::Pending | OrderStatus::Canceled => "Заказ не оплачен",
        OrderStatus::Paid => "Заказ не на ручной проверке: его выдаёт очередь",
        OrderStatus::Review => "Действие с заказом сейчас недоступно",
    };
    ServiceError::rejected("order.review_status", message)
}

fn refund_pending() -> ServiceError {
    ServiceError::rejected(
        "order.refund_pending",
        "По платежу этого заказа ждём решения платёжной системы о возврате. \
         Выдать заказ можно после отказа в возврате",
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FulfillmentOutcome {
    Done {
        subscription_id: Option<SubscriptionId>,
    },
    Retry {
        after_seconds: u64,
        reason: String,
    },
    Review {
        reason: String,
    },
}

impl FulfillmentOutcome {
    pub fn retry(after: Duration, reason: impl Into<String>) -> Self {
        FulfillmentOutcome::Retry {
            after_seconds: after.as_secs(),
            reason: reason.into(),
        }
    }
}

enum Halt {
    Retry(String),
    Review(String),
    Stale(String),
    Failed(ServiceError),
}

impl From<ServiceError> for Halt {
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::RemnaNotConfigured(reason) => {
                Halt::Retry(format!("Remnawave не подключена: {reason}"))
            }
            other => Halt::Failed(other),
        }
    }
}

impl From<StoreError> for Halt {
    fn from(err: StoreError) -> Self {
        Halt::Failed(err.into())
    }
}

impl From<sqlx::Error> for Halt {
    fn from(err: sqlx::Error) -> Self {
        Halt::Failed(err.into())
    }
}

fn same_moment(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    a.timestamp_millis() == b.timestamp_millis()
}

async fn client_blocked(tx: &mut Tx, client_id: ClientId) -> Result<bool, Halt> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM clients WHERE id = $1 FOR SHARE")
            .bind(client_id.0)
            .fetch_optional(tx.conn())
            .await?;
    Ok(status.as_deref() == Some(ClientStatus::Blocked.as_str()))
}

fn db_count(value: u32, what: &str) -> Result<i32, Halt> {
    i32::try_from(value).map_err(|_| Halt::Review(format!("{what}: слишком большое значение")))
}

const SUBSCRIPTION_KINDS: &str = "kind NOT IN ('topup', 'module')";

fn stale_reason(saved: Option<String>) -> String {
    let choice =
        "заказ ждёт решения администратора: «Пересчитать и выдать» или «Отметить выданным»";
    match saved.filter(|reason| !reason.trim().is_empty()) {
        Some(reason) => format!("{choice}. Причина: {reason}"),
        None => choice.to_string(),
    }
}

fn blocked_halt(order: &Order) -> Halt {
    Halt::Review(format!(
        "клиент заблокирован: заказ {} не выдаётся, после разблокировки выдайте его повторно",
        order.public_id
    ))
}

fn remna_halt(context: &str, err: RemnaError) -> Halt {
    let reason = format!("{context}: {err}");
    if err.is_retryable()
        || matches!(
            err,
            RemnaError::Unauthorized(_) | RemnaError::NotConfigured(_)
        )
    {
        Halt::Retry(reason)
    } else {
        Halt::Review(reason)
    }
}

fn traffic_bytes(value: u64) -> Result<i64, Halt> {
    i64::try_from(value).map_err(|_| Halt::Review("лимит трафика слишком большой".into()))
}

fn owned_by(user: &RemnaUser, profile: &NewUserProfile) -> bool {
    user.description.as_deref() == Some(profile.description.as_str())
}

struct Context {
    order: Order,
    snapshot: TariffSnapshot,
    subscription: Subscription,
    profile: NewUserProfile,
    converts_trial: bool,
}

pub struct Fulfillment<'a> {
    services: &'a Services,
}

impl<'a> Fulfillment<'a> {
    pub fn new(services: &'a Services) -> Self {
        Fulfillment { services }
    }

    pub async fn run(&self, order_id: OrderId) -> ServiceResult<FulfillmentOutcome> {
        let (order, waits_refund) = {
            let mut conn = self.services.store().conn().await?;
            let order = conn.orders().get(order_id).await?;
            let waits_refund = conn.orders().refund_pending(order_id).await?;
            (order, waits_refund)
        };
        match order.status {
            OrderStatus::Fulfilled => {
                return Ok(FulfillmentOutcome::Done {
                    subscription_id: order.subscription_id,
                });
            }
            OrderStatus::Paid | OrderStatus::Review => {}
            other => {
                return Ok(FulfillmentOutcome::Review {
                    reason: format!("заказ в статусе {}", other.as_str()),
                });
            }
        }
        if waits_refund {
            tracing::info!(order = %order.id, "выдача отложена: по платежу ждём решения о возврате");
            return Ok(FulfillmentOutcome::retry(
                REFUND_WAIT,
                REFUND_PENDING_REASON,
            ));
        }
        let result = match &order.intent {
            PurchaseIntent::TopUp { .. } => self.topup(&order).await,
            PurchaseIntent::Module { module, .. } => match self.ensure_not_blocked(&order).await {
                Ok(()) => self.module(&order, *module).await,
                Err(halt) => Err(halt),
            },
            PurchaseIntent::Subscription { .. }
            | PurchaseIntent::Trial
            | PurchaseIntent::AdminGrant { .. } => self.subscription(order.clone()).await,
        };
        match result {
            Ok(subscription_id) => Ok(FulfillmentOutcome::Done { subscription_id }),
            Err(Halt::Retry(reason)) => {
                tracing::info!(order = %order.id, %reason, "выдача отложена");
                Ok(FulfillmentOutcome::retry(MIN_RETRY, reason))
            }
            Err(Halt::Review(reason)) => {
                tracing::warn!(order = %order.id, %reason, "выдача ждёт ручной проверки");
                let mut conn = self.services.store().conn().await?;
                match conn.orders().mark_review(order.id, &reason).await {
                    Ok(()) | Err(StoreError::Invalid(_)) => {}
                    Err(err) => return Err(err.into()),
                }
                Ok(FulfillmentOutcome::Review { reason })
            }
            Err(Halt::Stale(reason)) => {
                tracing::warn!(order = %order.id, %reason, "заказ ждёт решения администратора");
                Ok(FulfillmentOutcome::Review { reason })
            }
            Err(Halt::Failed(err)) => Err(err),
        }
    }

    pub async fn review_orders(&self, limit: i64) -> ServiceResult<Vec<ReviewItem>> {
        let mut conn = self.services.store().conn().await?;
        let orders = conn
            .orders()
            .in_review(limit.clamp(1, REVIEW_ORDERS_MAX))
            .await?;
        Ok(orders
            .into_iter()
            .map(|order| ReviewItem {
                actions: review_actions(&order),
                order,
            })
            .collect())
    }

    pub async fn redeliver(&self, order_id: OrderId, actor: Actor) -> ServiceResult<Redelivery> {
        let mut tx = self.services.store().begin().await?;
        let order = tx.orders().lock(order_id).await?;
        if !matches!(order.status, OrderStatus::Paid | OrderStatus::Review) {
            return Err(not_deliverable(order.status));
        }
        if tx.orders().refund_pending(order.id).await? {
            return Err(refund_pending());
        }
        if order.status == OrderStatus::Review && tx.orders().plan_state(order.id).await?.stale() {
            return Err(ServiceError::rejected(
                "order.plan_stale",
                "План выдачи этого заказа рассчитан раньше и не применён. \
                 Выберите «Пересчитать и выдать» или «Отметить выданным»",
            ));
        }
        let queued = tx
            .jobs()
            .ensure_queued(&JobPayload::FulfillOrder { order_id: order.id })
            .await?;
        tx.audit()
            .record(&actor.audit(
                "order.redeliver",
                audit_entity::ORDER,
                Some(order.id.0),
                serde_json::json!({
                    "order_status": order.status,
                    "job_id": queued.id(),
                    "state": queued.state(),
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(Redelivery {
            order_id: order.id,
            job_id: queued.id(),
            state: queued.state(),
        })
    }

    pub async fn recalculate(
        &self,
        order_id: OrderId,
        reapply: bool,
        actor: Actor,
    ) -> ServiceResult<Recalculation> {
        let order = {
            let mut conn = self.services.store().conn().await?;
            conn.orders().get(order_id).await?
        };
        match order.subscription_id {
            Some(id) => {
                with_subscription_lock(self.services, id, self.reset_plan(order_id, reapply, actor))
                    .await
            }
            None => self.reset_plan(order_id, reapply, actor).await,
        }
    }

    async fn reset_plan(
        &self,
        order_id: OrderId,
        reapply: bool,
        actor: Actor,
    ) -> ServiceResult<Recalculation> {
        let mut tx = self.services.store().begin().await?;
        let order = tx.orders().lock(order_id).await?;
        if order.status != OrderStatus::Review {
            return Err(not_in_review(order.status));
        }
        if !order.kind.needs_remnawave() {
            return Err(ServiceError::rejected(
                "order.recalculate_kind",
                "У этого заказа нет плана выдачи: используйте «Повторить выдачу»",
            ));
        }
        if tx.orders().refund_pending(order.id).await? {
            return Err(refund_pending());
        }
        let state = tx.orders().plan_state(order.id).await?;
        let running = tx.jobs().active_fulfillment(order.id).await?.is_some();
        if running && matches!(state.stage(), PlanStage::Saved | PlanStage::Applied) {
            return Err(ServiceError::rejected(
                "order.delivery_running",
                "Выдача этого заказа уже идёт: дождитесь результата",
            ));
        }
        if state.applied() && !reapply {
            return Err(ServiceError::rejected(
                "order.plan_applied",
                "План этого заказа уже применён в Remnawave: пересчёт применит заказ ещё раз, \
                 продление прибавит дни повторно. Если так и нужно, подтвердите действие, \
                 иначе выберите «Повторить выдачу» или «Отметить выданным»",
            ));
        }
        let reset = tx.orders().reset_plan(order.id).await?;
        let queued = tx
            .jobs()
            .ensure_queued(&JobPayload::FulfillOrder { order_id: order.id })
            .await?;
        let previous = reset.plan.as_ref().map(|plan| {
            serde_json::json!({
                "kind": plan.kind,
                "base_expire_at": plan.base_expire_at,
                "target_expire_at": plan.target.expire_at,
            })
        });
        tx.audit()
            .record(&actor.audit(
                "order.recalculate",
                audit_entity::ORDER,
                Some(order.id.0),
                serde_json::json!({
                    "job_id": queued.id(),
                    "state": queued.state(),
                    "plan": reset.state.stage(),
                    "reapply": reapply,
                    "previous_plan": previous,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(Recalculation {
            order_id: order.id,
            job_id: queued.id(),
            state: queued.state(),
            plan: reset.state.stage(),
        })
    }

    pub async fn mark_fulfilled(
        &self,
        order_id: OrderId,
        note: &str,
        actor: Actor,
    ) -> ServiceResult<Order> {
        let note = note.trim();
        if note.is_empty() || note.chars().count() > MAX_FULFILL_NOTE {
            return Err(ServiceError::rejected(
                "order.mark_fulfilled_note",
                format!(
                    "Напишите пояснение, почему заказ считается выданным: до {MAX_FULFILL_NOTE} символов"
                ),
            ));
        }
        let order = {
            let mut conn = self.services.store().conn().await?;
            conn.orders().get(order_id).await?
        };
        match order.subscription_id {
            Some(id) => {
                with_subscription_lock(self.services, id, self.close_order(order_id, note, actor))
                    .await
            }
            None => self.close_order(order_id, note, actor).await,
        }
    }

    async fn close_order(
        &self,
        order_id: OrderId,
        note: &str,
        actor: Actor,
    ) -> ServiceResult<Order> {
        let mut tx = self.services.store().begin().await?;
        let order = tx.orders().lock(order_id).await?;
        if order.status != OrderStatus::Review {
            return Err(not_in_review(order.status));
        }
        let state = tx.orders().plan_state(order.id).await?;
        let reason = tx.orders().review_reason(order.id).await?;
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
                    subscription_id: fulfilled.subscription_id,
                })
                .await?;
        }
        tx.audit()
            .record(&actor.audit(
                "order.mark_fulfilled",
                audit_entity::ORDER,
                Some(order.id.0),
                serde_json::json!({
                    "note": note,
                    "review_reason": reason,
                    "plan": state.stage(),
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(fulfilled)
    }

    async fn topup(&self, order: &Order) -> Result<Option<SubscriptionId>, Halt> {
        let mut tx = self.services.store().begin().await?;
        let paid = tx
            .payments()
            .for_order(order.id)
            .await?
            .into_iter()
            .find(|p| p.status.is_paid_like())
            .ok_or_else(|| Halt::Review("у пополнения нет оплаченного платежа".into()))?;
        crate::inbox::credit_topup(&mut tx, order, &paid).await?;
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
        tx.commit().await?;
        Ok(None)
    }

    async fn module(
        &self,
        order: &Order,
        module: ModuleId,
    ) -> Result<Option<SubscriptionId>, Halt> {
        let registry = self.services.modules();
        if !registry.is_enabled(module) {
            return Err(Halt::Review(format!(
                "модуль «{}» выключен, покупку нужно выдать вручную",
                module.info().title
            )));
        }
        let ctx = self.services.module_context();
        registry
            .get(module)
            .fulfill(&ctx, order)
            .await
            .map_err(|err| match err {
                ModuleError::Store(_) | ModuleError::Core(_) => Halt::Retry(err.to_string()),
                other => Halt::Review(other.to_string()),
            })?;
        let mut tx = self.services.store().begin().await?;
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
                    subscription_id: fulfilled.subscription_id,
                })
                .await?;
        }
        tx.commit().await?;
        Ok(order.subscription_id)
    }

    async fn blocked(&self, order: &Order) -> Result<bool, Halt> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.clients().get(order.client_id).await?.is_blocked())
    }

    async fn ensure_not_blocked(&self, order: &Order) -> Result<(), Halt> {
        if self.blocked(order).await? {
            return Err(blocked_halt(order));
        }
        Ok(())
    }

    async fn created_earlier(&self, remna: &dyn RemnawaveApi, order: &Order) -> Result<bool, Halt> {
        match &order.plan {
            Some(plan) if matches!(plan.action, PlanAction::Create { .. }) => {
                let context = self.context(order.clone()).await?;
                Ok(self.current_user(remna, &context).await?.is_some())
            }
            _ => Ok(false),
        }
    }

    async fn subscription(&self, order: Order) -> Result<Option<SubscriptionId>, Halt> {
        let remna = self.services.remna()?;
        if order.subscription_id.is_none() {
            self.ensure_not_blocked(&order).await?;
        }
        let snapshot = order
            .snapshot
            .clone()
            .ok_or_else(|| Halt::Review("в заказе нет снимка тарифа".into()))?;
        let subscription = self.ensure_subscription(&order, &snapshot).await?;
        with_subscription_lock(
            self.services,
            subscription.id,
            self.locked(&*remna, order.id),
        )
        .await
    }

    async fn locked(
        &self,
        remna: &dyn RemnawaveApi,
        order_id: OrderId,
    ) -> Result<Option<SubscriptionId>, Halt> {
        let (order, state) = {
            let mut conn = self.services.store().conn().await?;
            let order = conn.orders().get(order_id).await?;
            let state = conn.orders().plan_state(order_id).await?;
            (order, state)
        };
        match order.status {
            OrderStatus::Fulfilled => return Ok(order.subscription_id),
            OrderStatus::Paid | OrderStatus::Review => {}
            other => {
                return Err(Halt::Review(format!("заказ в статусе {}", other.as_str())));
            }
        }
        if order.status == OrderStatus::Review && state.stale() {
            let mut conn = self.services.store().conn().await?;
            let saved = conn.orders().review_reason(order_id).await?;
            return Err(Halt::Stale(stale_reason(saved)));
        }
        if !state.applied()
            && self.blocked(&order).await?
            && !self.created_earlier(remna, &order).await?
        {
            return Err(blocked_halt(&order));
        }
        let context = self.context(order).await?;
        let (plan, saved) = match context.order.plan.clone() {
            Some(saved) => (saved, true),
            None => (self.prepare(remna, &context).await?, false),
        };
        let user = if state.applied() {
            self.applied_user(remna, &context, &plan).await?
        } else {
            if saved {
                self.check_base(remna, &context, &plan).await?;
            }
            self.ensure_no_refund(context.order.id).await?;
            let user = self.apply(remna, &context, &plan).await?;
            self.mark_applied(context.order.id).await?;
            user
        };
        let reset_done = user
            .last_traffic_reset_at
            .is_some_and(|at| at >= plan.period_started_at);
        let user = if plan.reset_traffic && !reset_done {
            remna
                .reset_user_traffic(user.id)
                .await
                .map_err(|e| remna_halt("сброс трафика", e))?
        } else {
            user
        };
        let fresh = remna
            .get_user(user.id)
            .await
            .map_err(|e| remna_halt("чтение пользователя после выдачи", e))?;
        self.finish(remna, &context, &plan, fresh).await?;
        Ok(Some(context.subscription.id))
    }

    async fn ensure_no_refund(&self, order_id: OrderId) -> Result<(), Halt> {
        let mut conn = self.services.store().conn().await?;
        if conn.orders().refund_pending_settled(order_id).await? {
            return Err(Halt::Retry(REFUND_PENDING_REASON.into()));
        }
        Ok(())
    }

    async fn mark_applied(&self, order_id: OrderId) -> Result<(), Halt> {
        let mut conn = self.services.store().conn().await?;
        conn.orders()
            .mark_plan_applied(order_id, self.services.now())
            .await?;
        Ok(())
    }

    async fn applied_user(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
        plan: &FulfillmentPlan,
    ) -> Result<RemnaUser, Halt> {
        match &plan.action {
            PlanAction::Update { user_id } => remna.get_user(*user_id).await.map_err(|err| {
                if err.is_not_found() {
                    Halt::Review(format!(
                        "пользователь Remnawave {user_id} удалён после выдачи: проверьте вручную"
                    ))
                } else {
                    remna_halt("чтение пользователя после выдачи", err)
                }
            }),
            PlanAction::Create { username, .. } => match self.current_user(remna, context).await? {
                Some(user) => Ok(user),
                None => Err(Halt::Review(format!(
                    "созданный пользователь {username} не найден в Remnawave: проверьте вручную"
                ))),
            },
        }
    }

    async fn check_base(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
        plan: &FulfillmentPlan,
    ) -> Result<(), Halt> {
        let (PlanAction::Update { user_id }, Some(base)) = (&plan.action, plan.base_expire_at)
        else {
            return Ok(());
        };
        let target = plan.target.expire_at;
        let expected = |at: DateTime<Utc>| same_moment(at, base) || same_moment(at, target);
        if context.subscription.mirror.expire_at.is_none_or(expected) {
            return Ok(());
        }
        let current = remna.get_user(*user_id).await.map_err(|err| {
            if err.is_not_found() {
                Halt::Review(format!(
                    "пользователь Remnawave {user_id} удалён до выдачи: выдайте вручную"
                ))
            } else {
                remna_halt("проверка срока перед выдачей", err)
            }
        })?;
        if expected(current.expire_at) {
            return Ok(());
        }
        Err(Halt::Review(format!(
            "срок подписки в Remnawave изменился после расчёта плана: был {base}, сейчас {}, \
             по плану {target}. Проверьте подписку: если срок ещё не учитывает этот заказ, выберите \
             «Пересчитать и выдать», если уже учитывает, выберите «Отметить выданным»",
            current.expire_at
        )))
    }

    async fn context(&self, order: Order) -> Result<Context, Halt> {
        let snapshot = order
            .snapshot
            .clone()
            .ok_or_else(|| Halt::Review("в заказе нет снимка тарифа".into()))?;
        let subscription = self.ensure_subscription(&order, &snapshot).await?;
        let mut conn = self.services.store().conn().await?;
        let client = conn.clients().get(order.client_id).await?;
        let identities = conn.clients().identities(order.client_id).await?;
        let profile = NewUserProfile {
            username: remna_username(subscription.public_id),
            telegram_id: identities
                .iter()
                .find(|i| i.kind == IdentityKind::Telegram)
                .and_then(|i| i.value.parse().ok()),
            email: identities
                .iter()
                .find(|i| i.kind == IdentityKind::Email)
                .map(|i| i.value.clone()),
            description: remna_description(client.public_id),
        };
        let converts_trial = subscription.kind == SubscriptionKind::Trial
            && !matches!(order.intent, PurchaseIntent::Trial);
        Ok(Context {
            order,
            snapshot,
            subscription,
            profile,
            converts_trial,
        })
    }

    async fn ensure_subscription(
        &self,
        order: &Order,
        snapshot: &TariffSnapshot,
    ) -> Result<Subscription, Halt> {
        if let Some(id) = order.subscription_id {
            let mut conn = self.services.store().conn().await?;
            return Ok(conn.subscriptions().get(id).await?);
        }
        let mut tx = self.services.store().begin().await?;
        let locked = tx.orders().lock(order.id).await?;
        if let Some(id) = locked.subscription_id {
            let subscription = tx.subscriptions().get(id).await?;
            tx.commit().await?;
            return Ok(subscription);
        }
        let settings = self.services.settings();
        let (kind, tariff_id, autorenew, extra_devices) = match &order.intent {
            PurchaseIntent::Trial => (SubscriptionKind::Trial, None, AutoRenewState::default(), 0),
            PurchaseIntent::AdminGrant { period_id, .. } => (
                SubscriptionKind::Manual,
                Some(snapshot.tariff_id),
                AutoRenewState {
                    enabled: false,
                    period_id: *period_id,
                },
                0,
            ),
            PurchaseIntent::Subscription {
                period_id,
                extra_devices,
                ..
            } => (
                SubscriptionKind::Paid,
                Some(snapshot.tariff_id),
                AutoRenewState {
                    enabled: settings.autorenew.enable_for_new,
                    period_id: Some(*period_id),
                },
                *extra_devices,
            ),
            PurchaseIntent::TopUp { .. } | PurchaseIntent::Module { .. } => {
                return Err(Halt::Review("заказ не выдаёт подписку".into()));
            }
        };
        let created = tx
            .subscriptions()
            .create(&NewSubscription {
                client_id: order.client_id,
                tariff_id,
                kind,
                extra_devices,
                autorenew,
            })
            .await?;
        tx.subscriptions()
            .reserve_username(created.id, &remna_username(created.public_id))
            .await?;
        tx.orders().set_subscription(order.id, created.id).await?;
        tx.commit().await?;
        Ok(created)
    }

    async fn wait_for_others(&self, context: &Context) -> Result<(), Halt> {
        let mut conn = self.services.store().conn().await?;
        match conn
            .orders()
            .delivery_blocker(context.subscription.id, context.order.id)
            .await?
        {
            Some(public_id) => Err(Halt::Retry(format!("сначала выдаётся заказ {public_id}"))),
            None => Ok(()),
        }
    }

    async fn previous_snapshot(&self, context: &Context) -> Result<Option<TariffSnapshot>, Halt> {
        let mut conn = self.services.store().conn().await?;
        let snapshot: Option<Json<TariffSnapshot>> = sqlx::query_scalar(&format!(
            "SELECT snapshot FROM orders WHERE subscription_id = $1 AND id <> $2 \
             AND fulfilled_at IS NOT NULL AND snapshot IS NOT NULL AND {SUBSCRIPTION_KINDS} \
             ORDER BY fulfilled_at DESC, id DESC LIMIT 1"
        ))
        .bind(context.subscription.id.0)
        .bind(context.order.id.0)
        .fetch_optional(conn.conn())
        .await?;
        Ok(snapshot.map(|s| s.0))
    }

    async fn kept_squads(&self, context: &Context, user: &RemnaUser) -> Result<Vec<SquadId>, Halt> {
        let from_tariffs = match self.previous_snapshot(context).await? {
            Some(previous) => previous.squads,
            None => {
                let mut conn = self.services.store().conn().await?;
                conn.tariffs().squads_in_use().await?
            }
        };
        Ok(user
            .squad_ids()
            .into_iter()
            .filter(|s| !from_tariffs.contains(s))
            .collect())
    }

    async fn held_devices(
        &self,
        context: &Context,
        user: &RemnaUser,
        paid_at: DateTime<Utc>,
    ) -> Result<Option<HeldDevices>, Halt> {
        let mut conn = self.services.store().conn().await?;
        let stored: Option<(i32, DateTime<Utc>, i32)> = sqlx::query_as(
            "SELECT held_limit, held_until, limit_after FROM subscription_device_holds \
             WHERE subscription_id = $1",
        )
        .bind(context.subscription.id.0)
        .fetch_optional(conn.conn())
        .await?;
        let paid_after_hold = match stored {
            Some((limit, until, _)) if until > paid_at => {
                return Ok(u32::try_from(limit)
                    .ok()
                    .map(|limit| HeldDevices { limit, until }));
            }
            Some((_, _, after)) => Some(i64::from(after)),
            None => None,
        };
        if user.status == UserStatus::Expired || user.expire_at <= paid_at {
            return Ok(None);
        }
        Ok(user
            .hwid_device_limit
            .map(|limit| paid_after_hold.map_or(limit, |after| limit.min(after)))
            .and_then(|limit| u32::try_from(limit).ok())
            .map(|limit| HeldDevices {
                limit,
                until: user.expire_at,
            }))
    }

    async fn current_user(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
    ) -> Result<Option<RemnaUser>, Halt> {
        if let Some(link) = &context.subscription.remna {
            match remna.get_user(link.user_id).await {
                Ok(user) => return Ok(Some(user)),
                Err(err) if err.is_not_found() => {}
                Err(err) => return Err(remna_halt("чтение пользователя", err)),
            }
        }
        match remna.get_user_by_username(&context.profile.username).await {
            Ok(user) if owned_by(&user, &context.profile) => Ok(Some(user)),
            Ok(_) => Err(Halt::Review(format!(
                "имя {} в Remnawave занято чужим пользователем",
                context.profile.username
            ))),
            Err(err) if err.is_not_found() => Ok(None),
            Err(err) => Err(remna_halt("поиск пользователя по имени", err)),
        }
    }

    async fn subscription_category(&self, context: &Context) -> Result<Option<CategoryId>, Halt> {
        let Some(tariff_id) = context.subscription.tariff_id else {
            return Ok(None);
        };
        let mut conn = self.services.store().conn().await?;
        match conn.tariffs().get(tariff_id).await {
            Ok(tariff) => Ok(Some(tariff.category_id)),
            Err(StoreError::NotFound(_)) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn plan_kind(
        &self,
        context: &Context,
        current: Option<&RemnaUser>,
    ) -> Result<PlanKind, Halt> {
        Ok(match context.order.kind {
            OrderKind::New | OrderKind::Trial => PlanKind::New,
            OrderKind::Renewal => PlanKind::Renewal,
            OrderKind::Change => PlanKind::Change,
            OrderKind::AdminGrant if current.is_none() => PlanKind::New,
            OrderKind::AdminGrant => classify(
                context.snapshot.category_id,
                Some(CurrentTariff {
                    kind: context.subscription.kind,
                    category_id: self.subscription_category(context).await?,
                }),
            ),
            OrderKind::TopUp | OrderKind::Module => {
                return Err(Halt::Review("заказ не выдаёт подписку".into()));
            }
        })
    }

    async fn prepare(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
    ) -> Result<FulfillmentPlan, Halt> {
        self.wait_for_others(context).await?;
        let current = self.current_user(remna, context).await?;
        let kind = self.plan_kind(context, current.as_ref()).await?;
        let paid_at = context.order.paid_at.unwrap_or_else(|| self.services.now());
        let (keep_squads, held_devices) = match &current {
            Some(user) if kind == PlanKind::Renewal => (
                self.kept_squads(context, user).await?,
                self.held_devices(context, user, paid_at).await?,
            ),
            _ => (Vec::new(), None),
        };
        let extra_devices = match &context.order.intent {
            PurchaseIntent::Subscription { extra_devices, .. } => *extra_devices,
            _ => 0,
        };
        let state = current.as_ref().map(RemnaUser::state);
        let tag = matches!(context.order.intent, PurchaseIntent::Trial).then_some(TRIAL_TAG);
        let computed = plan(&PlanInput {
            kind,
            snapshot: &context.snapshot,
            extra_devices,
            held_devices,
            paid_at,
            current: state.as_ref(),
            keep_squads: &keep_squads,
            profile: &context.profile,
            tag,
        })
        .map_err(|e| Halt::Review(format!("план выдачи не построен: {e}")))?;
        let squads = remna
            .internal_squads()
            .await
            .map_err(|e| remna_halt("список сквадов", e))?;
        let known: HashSet<uuid::Uuid> = squads.internal_squads.iter().map(|s| s.uuid).collect();
        if let Some(missing) = computed
            .target
            .squads
            .iter()
            .find(|s| !known.contains(&s.0))
        {
            return Err(Halt::Review(format!(
                "сквада {missing} нет в Remnawave: проверьте сквады тарифа «{}»",
                context.snapshot.name
            )));
        }
        let mut conn = self.services.store().conn().await?;
        Ok(conn.orders().save_plan(context.order.id, &computed).await?)
    }

    async fn apply(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
        plan: &FulfillmentPlan,
    ) -> Result<RemnaUser, Halt> {
        let target = &plan.target;
        match &plan.action {
            PlanAction::Create {
                username,
                telegram_id,
                email,
                description,
            } => {
                let request = CreateUserRequest {
                    username: username.clone(),
                    status: None,
                    expire_at: target.expire_at,
                    traffic_limit_bytes: traffic_bytes(target.traffic_limit_bytes)?,
                    traffic_limit_strategy: target.traffic_strategy,
                    description: Some(description.clone()),
                    tag: target.tag.clone(),
                    telegram_id: *telegram_id,
                    email: email.clone(),
                    hwid_device_limit: target.hwid_device_limit.map(i64::from),
                    active_internal_squads: target.squads.iter().map(|s| s.0).collect(),
                    external_squad_uuid: target.external_squad.map(|s| s.0),
                };
                match remna.create_user(&request).await {
                    Ok(user) => Ok(user),
                    Err(err) if err.code() == Some(USERNAME_TAKEN) => {
                        let existing = remna
                            .get_user_by_username(username)
                            .await
                            .map_err(|e| remna_halt("поиск занятого имени", e))?;
                        if existing.description.as_deref() != Some(description.as_str()) {
                            return Err(Halt::Review(format!(
                                "имя {username} в Remnawave занято чужим пользователем"
                            )));
                        }
                        self.patch(remna, existing.id, context, plan).await
                    }
                    Err(err) => Err(remna_halt("создание пользователя", err)),
                }
            }
            PlanAction::Update { user_id } => self.patch(remna, *user_id, context, plan).await,
        }
    }

    async fn patch(
        &self,
        remna: &dyn RemnawaveApi,
        user_id: RemnaUserId,
        context: &Context,
        plan: &FulfillmentPlan,
    ) -> Result<RemnaUser, Halt> {
        let target = &plan.target;
        let mut request = UpdateUserRequest::new(user_id);
        request.expire_at = Some(target.expire_at);
        request.traffic_limit_bytes = Some(traffic_bytes(target.traffic_limit_bytes)?);
        request.traffic_limit_strategy = Some(target.traffic_strategy);
        request.hwid_device_limit = Some(target.hwid_device_limit.map(i64::from));
        request.active_internal_squads = Some(target.squads.iter().map(|s| s.0).collect());
        if target.external_squad.is_some() || plan.kind != PlanKind::Renewal {
            request.external_squad_uuid = Some(target.external_squad.map(|s| s.0));
        }
        request.tag = match &target.tag {
            Some(tag) => Some(Some(tag.clone())),
            None if context.converts_trial => Some(None),
            None => None,
        };
        if plan.activate {
            request.status = Some(UserStatus::Active);
        }
        remna.update_user(&request).await.map_err(|err| {
            if err.is_not_found() {
                Halt::Review(format!(
                    "пользователь Remnawave {user_id} удалён до выдачи: выдайте вручную"
                ))
            } else {
                remna_halt("обновление пользователя", err)
            }
        })
    }

    async fn cut_access(
        &self,
        remna: &dyn RemnawaveApi,
        user: &RemnaUser,
    ) -> Result<RemnaUser, Halt> {
        let disabled = disable_remna_user(remna, user.id)
            .await
            .map_err(|e| remna_halt("отключение подписки заблокированного клиента", e))?;
        if let Err(err) = drop_everywhere(remna, vec![user.id]).await {
            tracing::warn!(user = %user.id, error = %err, "подключения заблокированного клиента не сброшены");
        }
        Ok(disabled)
    }

    async fn finish(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
        plan: &FulfillmentPlan,
        user: RemnaUser,
    ) -> Result<(), Halt> {
        let mut user = user;
        let mut cut = false;
        for _ in 0..FINISH_ATTEMPTS {
            let mut tx = self.services.store().begin().await?;
            let blocked = client_blocked(&mut tx, context.order.client_id).await?;
            if blocked && user.status != UserStatus::Disabled {
                tx.rollback().await?;
                user = self.cut_access(remna, &user).await?;
                cut = true;
                continue;
            }
            if !blocked && cut && user.status == UserStatus::Disabled {
                tx.rollback().await?;
                user = enable_remna_user(remna, user.id)
                    .await
                    .map_err(|e| remna_halt("включение подписки после разблокировки", e))?;
                cut = false;
                continue;
            }
            return self.record(tx, context, plan, &user, blocked).await;
        }
        Err(Halt::Retry(
            "статус клиента меняется во время выдачи".into(),
        ))
    }

    async fn record(
        &self,
        mut tx: Tx,
        context: &Context,
        plan: &FulfillmentPlan,
        user: &RemnaUser,
        blocked: bool,
    ) -> Result<(), Halt> {
        let order = &context.order;
        let subscription = &context.subscription;
        let snapshot = &context.snapshot;
        let settings = self.services.settings();
        let trial = matches!(order.intent, PurchaseIntent::Trial);
        match tx
            .subscriptions()
            .link_remna(
                subscription.id,
                &RemnaLink {
                    user_id: user.id,
                    username: user.username.clone(),
                    short_uuid: Some(user.short_uuid.clone()),
                },
            )
            .await
        {
            Ok(()) => {}
            Err(StoreError::Conflict { message, .. }) => {
                return Err(Halt::Review(format!(
                    "пользователь Remnawave {} уже привязан к другой подписке: {message}",
                    user.id
                )));
            }
            Err(err) => return Err(err.into()),
        }
        let mirror = user.mirror(subscription.mirror.devices_count, self.services.now());
        tx.subscriptions()
            .apply_mirror(subscription.id, &mirror)
            .await?;
        let kind = if trial {
            SubscriptionKind::Trial
        } else if matches!(order.intent, PurchaseIntent::AdminGrant { .. }) {
            if subscription.kind == SubscriptionKind::Trial {
                SubscriptionKind::Manual
            } else {
                subscription.kind
            }
        } else {
            SubscriptionKind::Paid
        };
        let extra_devices = match &order.intent {
            PurchaseIntent::Subscription { extra_devices, .. } => *extra_devices,
            _ => 0,
        };
        tx.subscriptions()
            .apply_purchase(
                subscription.id,
                &PurchaseApplied {
                    tariff_id: (!trial).then_some(snapshot.tariff_id),
                    kind,
                    extra_devices,
                    period_started_at: plan.period_started_at,
                    period_days: plan.period_days.max(1),
                },
            )
            .await?;
        match plan.device_hold {
            Some(hold) => {
                sqlx::query(
                    "INSERT INTO subscription_device_holds \
                     (subscription_id, order_id, held_limit, held_until, limit_after) \
                     VALUES ($1, $2, $3, $4, $5) ON CONFLICT (subscription_id) DO UPDATE SET \
                     order_id = EXCLUDED.order_id, held_limit = EXCLUDED.held_limit, \
                     held_until = EXCLUDED.held_until, limit_after = EXCLUDED.limit_after",
                )
                .bind(subscription.id.0)
                .bind(order.id.0)
                .bind(db_count(hold.held_limit, "лимит устройств")?)
                .bind(hold.until)
                .bind(db_count(hold.limit_after, "лимит устройств")?)
                .execute(tx.conn())
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM subscription_device_holds WHERE subscription_id = $1")
                    .bind(subscription.id.0)
                    .execute(tx.conn())
                    .await?;
            }
        }
        if let PurchaseIntent::Subscription { period_id, .. } = &order.intent {
            let enabled = if subscription.kind == SubscriptionKind::Trial {
                settings.autorenew.enable_for_new
            } else {
                subscription.autorenew.enabled
            };
            tx.subscriptions()
                .set_autorenew(
                    subscription.id,
                    AutoRenewState {
                        enabled,
                        period_id: Some(*period_id),
                    },
                )
                .await?;
        }
        let (fulfilled, changed) = tx
            .orders()
            .transition(order.id, OrderStatus::Fulfilled)
            .await?;
        if changed {
            let event = match (order.kind, subscription.remna.is_some()) {
                (OrderKind::New | OrderKind::Trial, _) | (OrderKind::AdminGrant, false) => {
                    DomainEvent::SubscriptionCreated {
                        subscription_id: subscription.id,
                        client_id: order.client_id,
                        kind,
                    }
                }
                (OrderKind::Change, _) => DomainEvent::TariffChanged {
                    subscription_id: subscription.id,
                    client_id: order.client_id,
                    from: subscription.tariff_id,
                    to: snapshot.tariff_id,
                },
                _ => DomainEvent::SubscriptionRenewed {
                    subscription_id: subscription.id,
                    client_id: order.client_id,
                    expire_at: user.expire_at,
                },
            };
            tx.events().push(&event).await?;
            if let (Some(from), Some(to)) = (subscription.mirror.status, mirror.status)
                && from != to
            {
                tx.events()
                    .push(&DomainEvent::SubscriptionStatusChanged {
                        subscription_id: subscription.id,
                        client_id: order.client_id,
                        from: Some(from),
                        to,
                    })
                    .await?;
            }
            tx.events()
                .push(&DomainEvent::OrderFulfilled {
                    order_id: fulfilled.id,
                    client_id: fulfilled.client_id,
                    order_kind: fulfilled.kind,
                    subscription_id: Some(subscription.id),
                })
                .await?;
            if blocked {
                tx.audit()
                    .record(&Actor::System.audit(
                        actions::BLOCKED_DELIVERY,
                        audit_entity::SUBSCRIPTION,
                        Some(subscription.id.0),
                        serde_json::json!({
                            "order_id": fulfilled.id,
                            "client_id": order.client_id,
                            "remna_user_id": user.id,
                            "status": mirror.status,
                            "reason": "клиент заблокирован во время выдачи заказа",
                        }),
                    ))
                    .await?;
            } else {
                tx.jobs()
                    .enqueue(
                        &JobPayload::Notify {
                            client_id: order.client_id,
                            template: "order.fulfilled".into(),
                            params: serde_json::json!({
                                "order_id": fulfilled.public_id,
                                "order_kind": fulfilled.kind,
                                "subscription_id": subscription.id,
                                "tariff": snapshot.name,
                                "expire_at": user.expire_at,
                            }),
                        },
                        None,
                    )
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
