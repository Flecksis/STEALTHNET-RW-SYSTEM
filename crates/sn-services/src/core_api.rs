use crate::error::ServiceError;
use crate::services::Services;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{ClientId, OrderId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::NewLedgerEntry;
use sn_domain::money::Money;
use sn_modules::{
    BalanceChange, BotSender, CoreApi, GrantPreview, GrantRefusal, GrantedOrder, Lifted,
    MailSender, ModuleError, Notification, Restricted, SubscriptionGrant, TrafficReading,
    TrafficRestriction,
};
use sn_store::{Db, StoreError, Tx};
use std::sync::Arc;

#[cfg(test)]
mod tests;

#[async_trait]
impl CoreApi for Services {
    async fn change_balance(&self, change: BalanceChange) -> Result<Money, ModuleError> {
        let currency = self.settings().currency;
        if change.amount.currency != currency {
            return Err(ModuleError::Core(format!(
                "баланс ведётся в {currency}, а не в {}",
                change.amount.currency
            )));
        }
        let mut tx = self.store().begin().await?;
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: change.client_id,
                wallet: change.wallet,
                amount: change.amount,
                kind: change.kind,
                order_id: change.order_id,
                payment_id: change.payment_id,
                admin_id: None,
                comment: change.comment.clone(),
                idempotency_key: Some(change.idempotency_key.clone()),
            })
            .await
            .map_err(|err| match err {
                StoreError::InsufficientFunds => {
                    ModuleError::Core("на балансе не хватает средств".into())
                }
                other => ModuleError::Store(other),
            })?;
        if !appended.duplicate {
            tx.events()
                .push(&DomainEvent::BalanceChanged {
                    client_id: change.client_id,
                    wallet: change.wallet,
                    kind: change.kind,
                    delta: change.amount,
                })
                .await?;
        }
        tx.commit().await?;
        Ok(appended.balance_after)
    }

    async fn add_days(
        &self,
        subscription_id: SubscriptionId,
        days: u32,
        reason: &str,
    ) -> Result<(), ModuleError> {
        self.subscriptions()
            .add_days(subscription_id, days, crate::Actor::System)
            .await
            .map(|_| ())
            .map_err(|e| ModuleError::Core(format!("{reason}: {e}")))
    }

    async fn notify(
        &self,
        client_id: ClientId,
        notification: Notification,
    ) -> Result<(), ModuleError> {
        self.enqueue(JobPayload::Notify {
            client_id,
            template: notification.template,
            params: notification.params,
        })
        .await
    }

    async fn enqueue(&self, job: JobPayload) -> Result<(), ModuleError> {
        let mut conn = self.store().conn().await?;
        conn.jobs().enqueue(&job, None).await?;
        Ok(())
    }

    async fn apply_device_addon(
        &self,
        order_id: OrderId,
        subscription_id: SubscriptionId,
        devices_after: u32,
    ) -> Result<(), ModuleError> {
        self.subscriptions()
            .apply_device_addon(order_id, subscription_id, devices_after)
            .await
            .map_err(addon_error)
    }

    async fn apply_traffic_addon(
        &self,
        order_id: OrderId,
        subscription_id: SubscriptionId,
        bytes: u64,
    ) -> Result<(), ModuleError> {
        self.subscriptions()
            .apply_traffic_addon(order_id, subscription_id, bytes)
            .await
            .map_err(addon_error)
    }

    async fn grant_preview(&self, grant: &SubscriptionGrant) -> Result<GrantPreview, ModuleError> {
        self.grants().preview(grant).await.map_err(grant_error)
    }

    async fn grant_subscription(
        &self,
        tx: &mut Tx,
        grant: &SubscriptionGrant,
    ) -> Result<Result<GrantedOrder, GrantRefusal>, ModuleError> {
        self.grants().grant(tx, grant).await.map_err(grant_error)
    }

    async fn notify_in(
        &self,
        tx: &mut Tx,
        client_id: ClientId,
        notification: Notification,
    ) -> Result<(), ModuleError> {
        tx.jobs()
            .enqueue(
                &JobPayload::Notify {
                    client_id,
                    template: notification.template,
                    params: notification.params,
                },
                None,
            )
            .await?;
        Ok(())
    }

    fn now(&self) -> DateTime<Utc> {
        Services::now(self)
    }

    async fn traffic_readings(&self) -> Result<Vec<TrafficReading>, ModuleError> {
        self.subscriptions()
            .traffic_readings()
            .await
            .map_err(traffic_error)
    }

    async fn restrict_traffic(
        &self,
        subscription_id: SubscriptionId,
        restriction: &TrafficRestriction,
    ) -> Result<Restricted, ModuleError> {
        self.subscriptions()
            .restrict_traffic(subscription_id, restriction)
            .await
            .map_err(traffic_error)
    }

    async fn lift_traffic(
        &self,
        subscription_id: SubscriptionId,
        restriction: &TrafficRestriction,
    ) -> Result<Lifted, ModuleError> {
        self.subscriptions()
            .lift_traffic(subscription_id, restriction)
            .await
            .map_err(traffic_error)
    }

    async fn client_devices(&self, client_id: ClientId) -> Result<Vec<String>, ModuleError> {
        match self.subscriptions().client_hwids(client_id).await {
            Ok(devices) => Ok(devices),
            Err(ServiceError::RemnaNotConfigured(_)) => Ok(Vec::new()),
            Err(err) => Err(traffic_error(err)),
        }
    }

    fn bot_sender(&self) -> Option<Arc<dyn BotSender>> {
        Services::bot_sender(self)
    }

    fn mail_sender(&self) -> Option<Arc<dyn MailSender>> {
        Some(Arc::new(crate::mail::MailBridge::new(self.clone())))
    }
}

fn grant_error(err: ServiceError) -> ModuleError {
    match err {
        ServiceError::Module(err) => err,
        ServiceError::Store(err) => ModuleError::Store(err),
        ServiceError::NotFound(what) => ModuleError::NotFound(what),
        ServiceError::Rejected { message, .. } => ModuleError::Invalid(message),
        other => ModuleError::Core(other.to_string()),
    }
}

fn traffic_error(err: ServiceError) -> ModuleError {
    match err {
        ServiceError::NotFound(what) => ModuleError::NotFound(what),
        ServiceError::Store(err) => ModuleError::Store(err),
        other => ModuleError::Core(other.to_string()),
    }
}

fn addon_error(err: ServiceError) -> ModuleError {
    match err {
        ServiceError::Rejected {
            code: "addon.wait",
            message,
        } => ModuleError::Core(message),
        ServiceError::Rejected { message, .. } => ModuleError::Invalid(message),
        ServiceError::NotFound(what) => ModuleError::NotFound(what),
        ServiceError::Domain(err) => ModuleError::Invalid(err.to_string()),
        other => ModuleError::Core(other.to_string()),
    }
}
