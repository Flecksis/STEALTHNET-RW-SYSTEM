use crate::context::ModuleContext;
use crate::error::ModuleError;
use async_trait::async_trait;
use axum::Router;
use serde::{Deserialize, Serialize};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{ClientId, PeriodId, TariffId};
use sn_domain::module::{ModuleId, ModuleInfo};
use sn_domain::money::Currency;
use sn_domain::order::{Order, OrderKind};
use sn_domain::payment::Payment;
use sn_domain::pricing::{PriceAdjustment, Quote};
use sn_domain::settings::{SettingField, missing_required};
use sn_store::Tx;
use std::net::IpAddr;
use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModuleConfig(pub serde_json::Map<String, serde_json::Value>);

impl ModuleConfig {
    pub fn from_value(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Object(map) => ModuleConfig(map.clone()),
            _ => ModuleConfig::default(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.get(key)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::Value::Object(self.0.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteHook {
    pub client_id: ClientId,
    pub tariff_id: TariffId,
    pub period_id: PeriodId,
    pub order_kind: OrderKind,
    pub currency: Currency,
    pub promo_code: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuoteOrigin {
    pub address: Option<IpAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemHook {
    pub client_id: ClientId,
    pub currency: Currency,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledTask {
    pub name: &'static str,
    pub every: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BotEntry {
    pub key: &'static str,
    pub label: &'static str,
    pub order: u8,
}

#[async_trait]
pub trait Module: Send + Sync + 'static {
    fn id(&self) -> ModuleId;

    fn info(&self) -> &'static ModuleInfo {
        self.id().info()
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        Vec::new()
    }

    fn validate_config(&self, config: &ModuleConfig) -> Result<(), ModuleError> {
        let missing = missing_required(&self.settings_schema(), &config.0);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(ModuleError::Invalid(format!(
                "не заполнены поля: {}",
                missing.join(", ")
            )))
        }
    }

    fn admin_routes(&self, _ctx: ModuleContext) -> Option<Router> {
        None
    }

    fn client_routes(&self, _ctx: ModuleContext) -> Option<Router> {
        None
    }

    fn public_routes(&self, _ctx: ModuleContext) -> Option<Router> {
        None
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        Vec::new()
    }

    fn cleanup_tasks(&self) -> Vec<ScheduledTask> {
        Vec::new()
    }

    fn bot_entries(&self) -> Vec<BotEntry> {
        Vec::new()
    }

    async fn on_event(
        &self,
        _ctx: &ModuleContext,
        _event: &DomainEvent,
    ) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn quote_adjustments(
        &self,
        _ctx: &ModuleContext,
        _hook: &QuoteHook,
    ) -> Result<Vec<PriceAdjustment>, ModuleError> {
        Ok(Vec::new())
    }

    async fn quote_adjustments_from(
        &self,
        ctx: &ModuleContext,
        hook: &QuoteHook,
        _origin: &QuoteOrigin,
    ) -> Result<Vec<PriceAdjustment>, ModuleError> {
        self.quote_adjustments(ctx, hook).await
    }

    async fn quote_item(
        &self,
        _ctx: &ModuleContext,
        _hook: &ItemHook,
    ) -> Result<Quote, ModuleError> {
        Err(ModuleError::NotImplemented("у модуля нет своих товаров"))
    }

    async fn fulfill(&self, _ctx: &ModuleContext, _order: &Order) -> Result<(), ModuleError> {
        Err(ModuleError::NotImplemented("у модуля нет своих товаров"))
    }

    async fn run_task(&self, _ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        Err(ModuleError::Invalid(format!("у модуля нет задачи {task}")))
    }

    async fn on_order_created(
        &self,
        _ctx: &ModuleContext,
        _tx: &mut Tx,
        _order: &Order,
        _payment: &Payment,
    ) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn on_order_paid(
        &self,
        _ctx: &ModuleContext,
        _tx: &mut Tx,
        _order: &Order,
        _payment: &Payment,
    ) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn on_order_canceled(
        &self,
        _ctx: &ModuleContext,
        _tx: &mut Tx,
        _order: &Order,
    ) -> Result<(), ModuleError> {
        Ok(())
    }
}
