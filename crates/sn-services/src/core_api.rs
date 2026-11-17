use crate::services::Services;
use async_trait::async_trait;
use sn_domain::events::DomainEvent;
use sn_domain::ids::{ClientId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::NewLedgerEntry;
use sn_domain::money::Money;
use sn_modules::{BalanceChange, BotSender, CoreApi, ModuleError, Notification};
use sn_store::{Db, StoreError};
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

    fn bot_sender(&self) -> Option<Arc<dyn BotSender>> {
        Services::bot_sender(self)
    }
}
