use crate::error::ModuleError;
use crate::sender::{BotSender, MailSender};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sn_domain::ids::{ClientId, OrderId, PaymentId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, Wallet};
use sn_domain::money::{Currency, Money};
use sn_store::Store;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceChange {
    pub client_id: ClientId,
    pub wallet: Wallet,
    pub amount: Money,
    pub kind: LedgerKind,
    pub comment: Option<String>,
    pub idempotency_key: String,
    pub order_id: Option<OrderId>,
    pub payment_id: Option<PaymentId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub template: String,
    pub params: serde_json::Value,
}

#[async_trait]
pub trait CoreApi: Send + Sync {
    async fn change_balance(&self, change: BalanceChange) -> Result<Money, ModuleError>;

    async fn add_days(
        &self,
        subscription_id: SubscriptionId,
        days: u32,
        reason: &str,
    ) -> Result<(), ModuleError>;

    async fn notify(
        &self,
        client_id: ClientId,
        notification: Notification,
    ) -> Result<(), ModuleError>;

    async fn enqueue(&self, job: JobPayload) -> Result<(), ModuleError>;

    async fn apply_device_addon(
        &self,
        _order_id: OrderId,
        _subscription_id: SubscriptionId,
        _devices_after: u32,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::NotImplemented(
            "ядро не умеет добавлять устройства",
        ))
    }

    async fn apply_traffic_addon(
        &self,
        _order_id: OrderId,
        _subscription_id: SubscriptionId,
        _bytes: u64,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::NotImplemented(
            "ядро не умеет добавлять трафик",
        ))
    }

    fn bot_sender(&self) -> Option<Arc<dyn BotSender>> {
        None
    }

    fn mail_sender(&self) -> Option<Arc<dyn MailSender>> {
        None
    }
}

#[derive(Clone)]
pub struct ModuleContext {
    pub store: Store,
    pub core: Arc<dyn CoreApi>,
    pub currency: Currency,
}
