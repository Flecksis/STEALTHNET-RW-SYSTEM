use super::{Channel, Checkout, CheckoutResult};
use crate::error::{ServiceError, ServiceResult};
use serde::Serialize;
use sn_domain::ids::ClientId;
use sn_domain::ledger::Wallet;
use sn_domain::money::{Currency, Money};
use sn_domain::pricing::topup_presets;
use sn_payments::{MethodsContext, ProviderKind};
use sn_store::Db;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopUpMethod {
    pub provider: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopUpOptions {
    pub currency: Currency,
    pub balance: Money,
    pub min: Money,
    pub max: Money,
    pub presets: Vec<Money>,
    pub methods: Vec<TopUpMethod>,
}

impl Checkout<'_> {
    pub async fn topup(
        &self,
        client_id: ClientId,
        amount: Money,
        provider: &str,
        channel: Channel,
    ) -> ServiceResult<CheckoutResult> {
        self.topup_from(client_id, amount, provider, channel, None)
            .await
    }

    pub async fn topup_options(
        &self,
        client_id: ClientId,
        channel: Channel,
    ) -> ServiceResult<TopUpOptions> {
        self.topup_open()?;
        let settings = self.services.settings();
        let currency = settings.currency;
        let balance = {
            let mut conn = self.services.store().conn().await?;
            if conn.clients().get(client_id).await?.is_blocked() {
                return Err(ServiceError::Forbidden);
            }
            conn.ledger()
                .balance(client_id, Wallet::Main, currency)
                .await?
        };
        let limits = settings.topup_limits();
        let methods = self
            .services
            .payments()
            .checkout_methods(&MethodsContext {
                currency,
                total: limits.min,
                stars_total: None,
                balance: None,
                channel: channel.client(),
            })
            .into_iter()
            .filter(|method| method.kind == ProviderKind::External && method.unavailable.is_none())
            .map(|method| TopUpMethod {
                provider: method.provider,
                title: method.title,
                hint: method.hint,
            })
            .collect();
        Ok(TopUpOptions {
            currency,
            balance,
            min: limits.min,
            max: limits.max,
            presets: topup_presets(limits),
            methods,
        })
    }
}
