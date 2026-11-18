use crate::error::ModuleError;
use crate::sender::{BotSender, MailSender};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::ids::{ClientId, OrderId, PaymentId, PeriodId, SquadId, SubscriptionId, TariffId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, Wallet};
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money};
use sn_domain::plan::PlanKind;
use sn_domain::settings::CoreSettings;
use sn_domain::subscription::{SubscriptionKind, SubscriptionStatus};
use sn_domain::traffic::TrafficPolicy;
use sn_store::{Store, Tx};
use std::sync::Arc;
use uuid::Uuid;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GrantSpec {
    Tariff {
        tariff_id: TariffId,
        period_id: PeriodId,
        days: u32,
    },
    Custom {
        name: String,
        days: u32,
        devices: u32,
        traffic: TrafficPolicy,
        squads: Vec<SquadId>,
    },
}

impl GrantSpec {
    pub const fn days(&self) -> u32 {
        match self {
            GrantSpec::Tariff { days, .. } | GrantSpec::Custom { days, .. } => *days,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantTarget {
    #[default]
    Auto,
    New,
    Subscription(SubscriptionId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionGrant {
    pub client_id: ClientId,
    pub module: ModuleId,
    pub reference: String,
    pub label: String,
    pub spec: GrantSpec,
    pub target: GrantTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantCurrent {
    pub subscription_id: SubscriptionId,
    pub tariff: Option<String>,
    pub kind: SubscriptionKind,
    pub expire_at: Option<DateTime<Utc>>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantRefusal {
    pub code: &'static str,
    pub message: String,
}

impl GrantRefusal {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        GrantRefusal {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantPreview {
    pub plan: PlanKind,
    pub multi: bool,
    pub current: Option<GrantCurrent>,
    pub choices: Vec<GrantCurrent>,
    pub new_allowed: bool,
    pub refusal: Option<GrantRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantedOrder {
    pub order_id: OrderId,
    pub order_public_id: Uuid,
    pub plan: PlanKind,
    pub subscription_id: Option<SubscriptionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficReading {
    pub subscription_id: SubscriptionId,
    pub lifetime_bytes: u64,
    pub status: SubscriptionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrafficRestriction {
    Disable,
    Squad { squad: Uuid, previous: Vec<Uuid> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restricted {
    Applied { previous_squads: Vec<Uuid> },
    Skipped(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lifted {
    Restored,
    Changed(String),
    Blocked,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MethodTitle {
    pub code: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentTitle {
    pub provider: String,
    pub title: String,
    pub button: String,
    pub methods: Vec<MethodTitle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeOnline {
    pub uuid: Uuid,
    pub name: String,
    pub online: i64,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeTraffic {
    pub name: String,
    pub bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostingPayment {
    pub provider: String,
    pub amount_minor: i64,
    pub at: DateTime<Utc>,
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

    async fn grant_preview(&self, _grant: &SubscriptionGrant) -> Result<GrantPreview, ModuleError> {
        Err(ModuleError::NotImplemented(
            "ядро не умеет выдавать подписки модулям",
        ))
    }

    async fn grant_subscription(
        &self,
        _tx: &mut Tx,
        _grant: &SubscriptionGrant,
    ) -> Result<Result<GrantedOrder, GrantRefusal>, ModuleError> {
        Err(ModuleError::NotImplemented(
            "ядро не умеет выдавать подписки модулям",
        ))
    }

    async fn notify_in(
        &self,
        _tx: &mut Tx,
        client_id: ClientId,
        notification: Notification,
    ) -> Result<(), ModuleError> {
        self.notify(client_id, notification).await
    }

    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    async fn traffic_readings(&self) -> Result<Vec<TrafficReading>, ModuleError> {
        Err(ModuleError::NotImplemented(
            "ядро не умеет читать трафик из Remnawave",
        ))
    }

    async fn restrict_traffic(
        &self,
        _subscription_id: SubscriptionId,
        _restriction: &TrafficRestriction,
    ) -> Result<Restricted, ModuleError> {
        Err(ModuleError::NotImplemented(
            "ядро не умеет ограничивать подписку",
        ))
    }

    async fn lift_traffic(
        &self,
        _subscription_id: SubscriptionId,
        _restriction: &TrafficRestriction,
    ) -> Result<Lifted, ModuleError> {
        Err(ModuleError::NotImplemented(
            "ядро не умеет снимать ограничение подписки",
        ))
    }

    async fn client_devices(&self, _client_id: ClientId) -> Result<Vec<String>, ModuleError> {
        Ok(Vec::new())
    }

    fn bot_sender(&self) -> Option<Arc<dyn BotSender>> {
        None
    }

    fn mail_sender(&self) -> Option<Arc<dyn MailSender>> {
        None
    }

    fn payment_titles(&self) -> Vec<PaymentTitle> {
        Vec::new()
    }

    fn paused(&self) -> bool {
        false
    }

    async fn nodes_online(&self) -> Result<Vec<NodeOnline>, ModuleError> {
        Ok(Vec::new())
    }

    async fn nodes_traffic(
        &self,
        _from: chrono::NaiveDate,
        _to: chrono::NaiveDate,
    ) -> Result<Vec<NodeTraffic>, ModuleError> {
        Ok(Vec::new())
    }

    async fn hosting_payments(&self) -> Result<Vec<HostingPayment>, ModuleError> {
        Ok(Vec::new())
    }

    fn module_enabled(&self, _module: ModuleId) -> bool {
        false
    }

    fn module_config(&self, _module: ModuleId) -> crate::module::ModuleConfig {
        crate::module::ModuleConfig::default()
    }

    fn core_settings(&self) -> Arc<CoreSettings> {
        Arc::new(CoreSettings::default())
    }
}

#[derive(Clone)]
pub struct ModuleContext {
    pub store: Store,
    pub core: Arc<dyn CoreApi>,
    pub currency: Currency,
}
