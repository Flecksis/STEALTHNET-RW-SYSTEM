use crate::provider::{PaymentProvider, ProviderConfig, ProviderKind};
use serde::Serialize;
use sn_domain::access::ClientChannel;
use sn_domain::money::{Amount, Currency, Money, Stars};
use sn_domain::payment::PaymentChannels;
use sn_domain::pricing::{BalanceCheck, balance_check};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderState {
    pub enabled: bool,
    pub config: ProviderConfig,
    pub channels: PaymentChannels,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RegistrySnapshot {
    pub currency: Currency,
    pub states: HashMap<String, ProviderState>,
    pub order: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodsContext {
    pub currency: Currency,
    pub total: Money,
    pub stars_total: Option<Stars>,
    pub balance: Option<Money>,
    pub channel: Option<ClientChannel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Unavailable {
    InsufficientBalance { balance: Money, missing: Money },
    NoStarsPrice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckoutMethod {
    pub provider: &'static str,
    pub kind: ProviderKind,
    pub title: &'static str,
    pub hint: &'static str,
    pub amount: Option<Amount>,
    pub unavailable: Option<Unavailable>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderView {
    pub id: &'static str,
    pub title: &'static str,
    pub kind: ProviderKind,
    pub currencies: &'static [Currency],
    pub enabled: bool,
    pub configured: bool,
    pub accepts_http_webhooks: bool,
    pub supports_recurring: bool,
    pub channels: PaymentChannels,
    pub settings: serde_json::Map<String, serde_json::Value>,
}

pub struct ProviderRegistry {
    providers: Vec<Arc<dyn PaymentProvider>>,
    snapshot: RwLock<Arc<RegistrySnapshot>>,
}

impl ProviderRegistry {
    pub fn new(providers: Vec<Arc<dyn PaymentProvider>>) -> Self {
        let states = providers
            .iter()
            .map(|p| {
                (
                    p.id().to_string(),
                    ProviderState {
                        enabled: p.enabled_by_default(),
                        config: ProviderConfig::default(),
                        channels: PaymentChannels::ALL,
                    },
                )
            })
            .collect();
        ProviderRegistry {
            providers,
            snapshot: RwLock::new(Arc::new(RegistrySnapshot {
                currency: Currency::Usd,
                states,
                order: Vec::new(),
            })),
        }
    }

    pub fn builtin() -> Self {
        Self::new(crate::providers::builtin())
    }

    pub fn providers(&self) -> &[Arc<dyn PaymentProvider>] {
        &self.providers
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn PaymentProvider>> {
        self.providers.iter().find(|p| p.id() == id).cloned()
    }

    pub fn defaults(&self) -> Vec<(&'static str, bool)> {
        self.providers
            .iter()
            .map(|p| (p.id(), p.enabled_by_default()))
            .collect()
    }

    pub fn snapshot(&self) -> Arc<RegistrySnapshot> {
        match self.snapshot.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn apply(&self, snapshot: RegistrySnapshot) {
        let snapshot = Arc::new(snapshot);
        match self.snapshot.write() {
            Ok(mut guard) => *guard = snapshot,
            Err(poisoned) => *poisoned.into_inner() = snapshot,
        }
    }

    pub fn config(&self, id: &str) -> ProviderConfig {
        self.snapshot()
            .states
            .get(id)
            .map(|s| s.config.clone())
            .unwrap_or_default()
    }

    pub fn is_available(&self, id: &str, currency: Currency) -> bool {
        let Some(provider) = self.get(id) else {
            return false;
        };
        let snapshot = self.snapshot();
        let Some(state) = snapshot.states.get(id) else {
            return false;
        };
        state.enabled
            && provider.currencies().contains(&currency)
            && (provider.kind() == ProviderKind::Balance || provider.is_configured(&state.config))
    }

    pub fn allowed_in(&self, id: &str, channel: ClientChannel) -> bool {
        self.snapshot()
            .states
            .get(id)
            .is_some_and(|state| state.channels.allows(channel))
    }

    pub fn accepts_webhooks(&self, id: &str) -> bool {
        let Some(provider) = self.get(id) else {
            return false;
        };
        if provider.kind() != ProviderKind::External || !provider.accepts_http_webhooks() {
            return false;
        }
        self.snapshot()
            .states
            .get(id)
            .is_some_and(|state| provider.is_configured(&state.config))
    }

    pub fn views(&self) -> Vec<ProviderView> {
        let snapshot = self.snapshot();
        self.providers
            .iter()
            .map(|p| {
                let state = snapshot.states.get(p.id()).cloned().unwrap_or_default();
                ProviderView {
                    id: p.id(),
                    title: p.title(),
                    kind: p.kind(),
                    currencies: p.currencies(),
                    enabled: state.enabled,
                    configured: p.is_configured(&state.config),
                    accepts_http_webhooks: p.accepts_http_webhooks(),
                    supports_recurring: p.supports_recurring(),
                    channels: state.channels,
                    settings: state.config.public_view(&p.settings_schema()),
                }
            })
            .collect()
    }

    pub fn checkout_methods(&self, ctx: &MethodsContext) -> Vec<CheckoutMethod> {
        let snapshot = self.snapshot();
        let mut ordered: Vec<&'static str> = Vec::new();
        for id in &snapshot.order {
            if let Some(provider) = self.get(id)
                && !ordered.contains(&provider.id())
            {
                ordered.push(provider.id());
            }
        }
        for provider in &self.providers {
            if !ordered.contains(&provider.id()) {
                ordered.push(provider.id());
            }
        }
        let mut methods = Vec::new();
        let mut balance_method = None;
        for id in ordered {
            if !self.is_available(id, ctx.currency)
                || ctx
                    .channel
                    .is_some_and(|channel| !self.allowed_in(id, channel))
            {
                continue;
            }
            let Some(provider) = self.get(id) else {
                continue;
            };
            let label = provider.client_label(ctx.currency);
            let (amount, unavailable) = match provider.kind() {
                ProviderKind::External => (Some(ctx.total.into()), None),
                ProviderKind::Stars => match ctx.stars_total {
                    Some(stars) => (Some(stars.into()), None),
                    None => (None, Some(Unavailable::NoStarsPrice)),
                },
                ProviderKind::Balance => {
                    let Some(balance) = ctx.balance else {
                        continue;
                    };
                    let unavailable = match balance_check(balance, ctx.total) {
                        Ok(BalanceCheck::Enough) => None,
                        Ok(BalanceCheck::Short { balance, missing }) => {
                            Some(Unavailable::InsufficientBalance { balance, missing })
                        }
                        Err(_) => continue,
                    };
                    balance_method = Some(CheckoutMethod {
                        provider: provider.id(),
                        kind: ProviderKind::Balance,
                        title: label.title,
                        hint: label.hint,
                        amount: Some(ctx.total.into()),
                        unavailable,
                    });
                    continue;
                }
            };
            methods.push(CheckoutMethod {
                provider: provider.id(),
                kind: provider.kind(),
                title: label.title,
                hint: label.hint,
                amount,
                unavailable,
            });
        }
        methods.extend(balance_method);
        methods
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{PaymentError, PaymentResult};
    use crate::provider::*;
    use async_trait::async_trait;
    use serde_json::json;
    use sn_domain::payment::providers;
    use sn_domain::settings::PaymentOrder;

    struct Fake {
        id: &'static str,
        kind: ProviderKind,
        currencies: &'static [Currency],
        enabled: bool,
    }

    #[async_trait]
    impl PaymentProvider for Fake {
        fn id(&self) -> &'static str {
            self.id
        }

        fn title(&self) -> &'static str {
            self.id
        }

        fn kind(&self) -> ProviderKind {
            self.kind
        }

        fn currencies(&self) -> &'static [Currency] {
            self.currencies
        }

        fn client_label(&self, _currency: Currency) -> ClientLabel {
            ClientLabel {
                title: self.id,
                hint: "",
            }
        }

        fn enabled_by_default(&self) -> bool {
            self.enabled
        }

        async fn create_invoice(
            &self,
            _config: &ProviderConfig,
            _request: &InvoiceRequest,
        ) -> PaymentResult<Invoice> {
            Err(PaymentError::Unsupported("тест"))
        }

        async fn parse_webhook(
            &self,
            _config: &ProviderConfig,
            _request: &WebhookRequest,
        ) -> PaymentResult<WebhookOutcome> {
            Err(PaymentError::Unsupported("тест"))
        }
    }

    fn fake(
        id: &'static str,
        kind: ProviderKind,
        currencies: &'static [Currency],
        enabled: bool,
    ) -> Arc<dyn PaymentProvider> {
        Arc::new(Fake {
            id,
            kind,
            currencies,
            enabled,
        })
    }

    fn registry(currency: Currency) -> ProviderRegistry {
        let registry = ProviderRegistry::new(vec![
            fake(
                providers::BALANCE,
                ProviderKind::Balance,
                &Currency::ALL,
                true,
            ),
            fake(
                providers::CRYPTOPAY,
                ProviderKind::External,
                &[Currency::Usd],
                true,
            ),
            fake(
                providers::HELEKET,
                ProviderKind::External,
                &[Currency::Usd],
                false,
            ),
            fake(
                providers::LAVA,
                ProviderKind::External,
                &[Currency::Usd],
                true,
            ),
            fake(
                providers::PLATEGA,
                ProviderKind::External,
                &[Currency::Rub],
                true,
            ),
            fake(
                providers::YOOKASSA,
                ProviderKind::External,
                &[Currency::Rub],
                true,
            ),
            fake(
                providers::YOOMONEY,
                ProviderKind::External,
                &[Currency::Rub],
                true,
            ),
            fake(
                providers::OVERPAY,
                ProviderKind::External,
                &[Currency::Rub],
                false,
            ),
            fake(providers::STARS, ProviderKind::Stars, &Currency::ALL, true),
        ]);
        let states = registry
            .defaults()
            .into_iter()
            .map(|(id, enabled)| {
                (
                    id.to_string(),
                    ProviderState {
                        enabled,
                        config: ProviderConfig::default(),
                        channels: sn_domain::payment::PaymentChannels::ALL,
                    },
                )
            })
            .collect();
        registry.apply(RegistrySnapshot {
            currency,
            states,
            order: PaymentOrder::default().for_currency(currency).to_vec(),
        });
        registry
    }

    #[test]
    fn умолчания_встроенных_систем_как_в_настройках() {
        let builtin = ProviderRegistry::builtin();
        let on: Vec<&str> = builtin
            .defaults()
            .into_iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            on,
            [
                providers::CRYPTOPAY,
                providers::LAVA,
                providers::YOOKASSA,
                providers::PLATEGA,
                providers::YOOMONEY,
                providers::STARS,
                providers::BALANCE
            ]
        );
    }

    #[test]
    fn способы_оплаты_в_долларах_как_в_макете() {
        let registry = registry(Currency::Usd);
        let methods = registry.checkout_methods(&MethodsContext {
            currency: Currency::Usd,
            total: Money::new(699, Currency::Usd),
            stars_total: Some(Stars(350)),
            balance: Some(Money::new(420, Currency::Usd)),
            channel: None,
        });
        let ids: Vec<&str> = methods.iter().map(|m| m.provider).collect();
        assert_eq!(
            ids,
            [
                providers::CRYPTOPAY,
                providers::LAVA,
                providers::STARS,
                providers::BALANCE
            ]
        );
        assert_eq!(methods[2].amount, Some(Stars(350).into()));
        assert_eq!(
            methods[3].unavailable,
            Some(Unavailable::InsufficientBalance {
                balance: Money::new(420, Currency::Usd),
                missing: Money::new(279, Currency::Usd)
            })
        );
    }

    #[test]
    fn способ_выключенный_в_канале_не_показывается_там() {
        let registry = registry(Currency::Usd);
        let mut snapshot = (*registry.snapshot()).clone();
        if let Some(state) = snapshot.states.get_mut(providers::CRYPTOPAY) {
            state.channels = sn_domain::payment::PaymentChannels {
                cabinet: true,
                bot: false,
                miniapp: false,
            };
        }
        if let Some(state) = snapshot.states.get_mut(providers::STARS) {
            state.channels.cabinet = false;
        }
        registry.apply(snapshot);
        let ids = |channel: Option<ClientChannel>| -> Vec<&'static str> {
            registry
                .checkout_methods(&MethodsContext {
                    currency: Currency::Usd,
                    total: Money::new(699, Currency::Usd),
                    stars_total: Some(Stars(350)),
                    balance: Some(Money::new(9_000, Currency::Usd)),
                    channel,
                })
                .into_iter()
                .map(|m| m.provider)
                .collect()
        };
        assert_eq!(
            ids(Some(ClientChannel::Bot)),
            [providers::LAVA, providers::STARS, providers::BALANCE]
        );
        assert_eq!(
            ids(Some(ClientChannel::Cabinet)),
            [providers::CRYPTOPAY, providers::LAVA, providers::BALANCE]
        );
        assert_eq!(
            ids(None),
            [
                providers::CRYPTOPAY,
                providers::LAVA,
                providers::STARS,
                providers::BALANCE
            ]
        );
        assert!(registry.allowed_in(providers::CRYPTOPAY, ClientChannel::Cabinet));
        assert!(!registry.allowed_in(providers::CRYPTOPAY, ClientChannel::Miniapp));
        assert!(!registry.allowed_in("нет-такого", ClientChannel::Bot));
        let view = registry
            .views()
            .into_iter()
            .find(|v| v.id == providers::CRYPTOPAY)
            .unwrap();
        assert!(!view.channels.bot);
    }

    #[test]
    fn способы_оплаты_в_рублях_и_без_цены_в_звёздах() {
        let registry = registry(Currency::Rub);
        let methods = registry.checkout_methods(&MethodsContext {
            currency: Currency::Rub,
            total: Money::new(49_900, Currency::Rub),
            stars_total: None,
            balance: None,
            channel: None,
        });
        let ids: Vec<&str> = methods.iter().map(|m| m.provider).collect();
        assert_eq!(
            ids,
            [
                providers::PLATEGA,
                providers::YOOKASSA,
                providers::YOOMONEY,
                providers::STARS
            ]
        );
        assert_eq!(methods[3].unavailable, Some(Unavailable::NoStarsPrice));
    }

    #[test]
    fn уведомления_принимаются_от_настроенной_системы_даже_выключенной() {
        let registry = ProviderRegistry::builtin();
        let configured = |id: &str| match id {
            providers::PLATEGA => crate::test_support::config(&[
                ("merchant_id", json!("29ef6fa6-0d2b-466c-9604-0363a30436cc")),
                ("secret", json!("s")),
            ]),
            providers::OVERPAY => crate::test_support::config(&[
                ("api_url", json!("https://api.overpay.io")),
                ("project_id", json!("1")),
                ("login", json!("l")),
                ("password", json!("p")),
            ]),
            providers::STARS => crate::test_support::config(&[("bot_token", json!("1:abc"))]),
            _ => ProviderConfig::default(),
        };
        let apply = |enabled: &[&str]| {
            registry.apply(RegistrySnapshot {
                currency: Currency::Rub,
                states: registry
                    .providers()
                    .iter()
                    .map(|p| {
                        (
                            p.id().to_string(),
                            ProviderState {
                                enabled: enabled.contains(&p.id()),
                                config: configured(p.id()),
                                channels: sn_domain::payment::PaymentChannels::ALL,
                            },
                        )
                    })
                    .collect(),
                order: Vec::new(),
            })
        };
        apply(&[
            providers::PLATEGA,
            providers::YOOKASSA,
            providers::OVERPAY,
            providers::STARS,
            providers::BALANCE,
        ]);
        assert!(registry.accepts_webhooks(providers::PLATEGA));
        assert!(!registry.accepts_webhooks(providers::YOOKASSA));
        assert!(!registry.accepts_webhooks(providers::OVERPAY));
        assert!(!registry.accepts_webhooks(providers::STARS));
        assert!(!registry.accepts_webhooks(providers::BALANCE));
        assert!(!registry.accepts_webhooks("nope"));
        apply(&[providers::YOOKASSA]);
        assert!(registry.accepts_webhooks(providers::PLATEGA));
        assert!(!registry.is_available(providers::PLATEGA, Currency::Rub));
        assert!(!registry.accepts_webhooks(providers::YOOKASSA));
        assert!(!registry.accepts_webhooks(providers::OVERPAY));
        assert!(!registry.accepts_webhooks(providers::STARS));
    }

    #[test]
    fn ненастроенная_система_не_предлагается() {
        let registry = ProviderRegistry::builtin();
        assert!(!registry.is_available(providers::CRYPTOPAY, Currency::Usd));
        let view = registry
            .views()
            .into_iter()
            .find(|v| v.id == providers::CRYPTOPAY)
            .unwrap();
        assert!(view.enabled);
        assert!(!view.configured);
        assert_eq!(view.settings["api_token"], json!({"filled": false}));
    }
}
