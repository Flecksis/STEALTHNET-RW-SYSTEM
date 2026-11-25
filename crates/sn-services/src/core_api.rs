use crate::error::ServiceError;
use crate::limits::{RATE_LIMITED, RateRule};
use crate::services::Services;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{ClientId, OrderId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::NewLedgerEntry;
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money};
use sn_domain::settings::CoreSettings;
use sn_modules::context::RateLimit;
use sn_modules::{
    BalanceChange, BotSender, CoreApi, GrantPreview, GrantRefusal, GrantedOrder, HostingPayment,
    Lifted, MailSender, MethodTitle, ModuleConfig, ModuleError, NodeOnline, NodeTraffic,
    Notification, PaymentTitle, Restricted, SubscriptionGrant, TrafficReading, TrafficRestriction,
};
use sn_payments::ProviderConfig;
use sn_store::{Db, StoreError, Tx};
use std::sync::Arc;

#[cfg(test)]
mod tests;

#[async_trait]
impl CoreApi for Services {
    async fn nodes_online(&self) -> Result<Vec<NodeOnline>, ModuleError> {
        let remna = self.remna().map_err(traffic_error)?;
        let nodes = remna
            .nodes()
            .await
            .map_err(|e| ModuleError::Core(e.to_string()))?;
        Ok(nodes
            .into_iter()
            .map(|n| NodeOnline {
                uuid: n.uuid,
                name: n.name,
                online: n.users_online,
                connected: n.is_connected && !n.is_disabled,
            })
            .collect())
    }

    async fn nodes_traffic(
        &self,
        from: chrono::NaiveDate,
        to: chrono::NaiveDate,
    ) -> Result<Vec<NodeTraffic>, ModuleError> {
        let remna = self.remna().map_err(traffic_error)?;
        let usage = remna
            .nodes_usage(&sn_remnawave::models::NodesUsageQuery {
                start: from,
                end: to,
                top_nodes_limit: None,
            })
            .await
            .map_err(|e| ModuleError::Core(e.to_string()))?;
        let mut out: Vec<NodeTraffic> = usage
            .series
            .into_iter()
            .map(|s| NodeTraffic {
                name: s.name,
                bytes: s.total.max(0.0).round() as i64,
            })
            .collect();
        out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        Ok(out)
    }

    async fn hosting_payments(&self) -> Result<Vec<HostingPayment>, ModuleError> {
        const SIZE: u32 = 500;
        let remna = self.remna().map_err(traffic_error)?;
        let mut out = Vec::new();
        for page in 0..20_u32 {
            let found = remna
                .infra_billing_history(&sn_remnawave::models::BillingHistoryQuery {
                    start: Some(page * SIZE),
                    size: Some(SIZE),
                })
                .await
                .map_err(|e| ModuleError::Core(e.to_string()))?;
            let count = found.records.len();
            out.extend(found.records.into_iter().map(|r| HostingPayment {
                provider: r.provider.map(|p| p.name).unwrap_or_default(),
                amount_minor: r.amount.minor(),
                at: r.billed_at,
            }));
            if count < SIZE as usize {
                break;
            }
        }
        Ok(out)
    }
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

    async fn within_rate_limit(&self, limit: RateLimit, key: &str) -> Result<bool, ModuleError> {
        let rule = RateRule {
            scope: limit.scope,
            limit: limit.limit,
            window_seconds: limit.window_seconds,
        };
        match self.limits().hit(rule, key).await {
            Ok(_) => Ok(true),
            Err(err) if err.code() == RATE_LIMITED => Ok(false),
            Err(ServiceError::Store(err)) => Err(ModuleError::Store(err)),
            Err(err) => Err(ModuleError::Core(err.to_string())),
        }
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

    fn payment_titles(&self) -> Vec<PaymentTitle> {
        let registry = self.payments();
        let snapshot = registry.snapshot();
        registry
            .providers()
            .iter()
            .map(|provider| {
                let state = snapshot.states.get(provider.id());
                let choices = state
                    .map(|state| state.methods.as_slice())
                    .unwrap_or_default();
                let empty = ProviderConfig::default();
                let button = state
                    .and_then(|state| state.label.title.clone())
                    .unwrap_or_else(|| {
                        provider
                            .client_label(
                                snapshot.currency,
                                state.map_or(&empty, |state| &state.config),
                            )
                            .title
                            .to_string()
                    });
                let mut methods: Vec<MethodTitle> = Vec::new();
                for currency in Currency::ALL {
                    for method in provider.methods(currency) {
                        if methods.iter().any(|m| m.code == method.code) {
                            continue;
                        }
                        let title = choices
                            .iter()
                            .find(|choice| choice.code == method.code)
                            .and_then(|choice| choice.title.clone())
                            .unwrap_or_else(|| method.title.to_string());
                        methods.push(MethodTitle {
                            code: method.code.to_string(),
                            title,
                        });
                    }
                }
                PaymentTitle {
                    provider: provider.id().to_string(),
                    title: provider.title().to_string(),
                    button,
                    methods,
                }
            })
            .collect()
    }

    fn paused(&self) -> bool {
        self.pause_gate().is_paused()
    }

    fn module_enabled(&self, module: ModuleId) -> bool {
        self.modules().is_enabled(module)
    }

    fn module_config(&self, module: ModuleId) -> ModuleConfig {
        self.modules().config(module)
    }

    fn captcha_endpoints(&self) -> sn_modules::catalog::antibot::captcha::Endpoints {
        Services::captcha_endpoints(self).as_ref().clone()
    }

    fn captcha_http(&self) -> sn_egress::Http {
        Services::captcha_http(self)
    }

    fn core_settings(&self) -> Arc<CoreSettings> {
        self.settings()
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
