mod aurapay;
mod balance;
mod cloudpayments;
mod cryptomus;
mod cryptopay;
mod gram;
mod heleket;
mod kassaai;
mod lava;
mod merchant;
mod mulenpay;
mod overpay;
mod paypalych;
mod platega;
mod rollypay;
mod stars;
mod yookassa;
mod yoomoney;

pub use aurapay::{AuraPay, signature as aurapay_signature};
pub use balance::Balance;
pub use cloudpayments::{CloudPayments, signature as cloudpayments_signature};
pub use cryptomus::{Cryptomus, signature as cryptomus_signature};
pub use cryptopay::{CryptoPay, signature as cryptopay_signature};
pub use gram::{Gram, GramInvoice, comment_payload, format_grams, friendly_address};
pub use heleket::{Heleket, signature as heleket_signature};
pub use kassaai::{KassaAi, notice_signature as kassaai_notice_signature};
pub use lava::Lava;
pub use mulenpay::MulenPay;
pub use overpay::{Overpay, hook_token as overpay_hook_token};
pub use paypalych::{PayPalych, signature as paypalych_signature};
pub use platega::Platega;
pub use rollypay::{RollyPay, signature as rollypay_signature};
pub use stars::{
    STARS_TOKEN_FIELD, StarDirection, StarTransaction, StarsPayment, StarsPaymentEvent,
    StarsPreCheckout, TelegramStars,
};
pub use yookassa::YooKassa;
pub use yoomoney::YooMoney;

use crate::provider::PaymentProvider;
use std::sync::Arc;

pub fn builtin() -> Vec<Arc<dyn PaymentProvider>> {
    vec![
        Arc::new(CryptoPay::new()),
        Arc::new(Gram::new()),
        Arc::new(Heleket::new()),
        Arc::new(Cryptomus::new()),
        Arc::new(Lava::new()),
        Arc::new(YooKassa::new()),
        Arc::new(Platega::new()),
        Arc::new(YooMoney),
        Arc::new(CloudPayments::new()),
        Arc::new(Overpay::new()),
        Arc::new(RollyPay::new()),
        Arc::new(MulenPay::new()),
        Arc::new(PayPalych::new()),
        Arc::new(KassaAi::new()),
        Arc::new(AuraPay::new()),
        Arc::new(TelegramStars::new()),
        Arc::new(Balance),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{MethodsContext, ProviderRegistry, ProviderState, RegistrySnapshot};
    use crate::test_support::config;
    use serde_json::json;
    use sn_domain::money::{Currency, Money, Stars};
    use sn_domain::payment::providers;
    use sn_domain::settings::PaymentOrder;

    fn filled(id: &str) -> crate::provider::ProviderConfig {
        match id {
            providers::CRYPTOPAY => config(&[("api_token", json!("1:AAA"))]),
            providers::HELEKET => config(&[
                ("merchant_id", json!("8b03432e-385b-4670-8d06-064591096795")),
                ("api_key", json!("k")),
            ]),
            providers::CRYPTOMUS => config(&[
                ("merchant_id", json!("8b03432e-385b-4670-8d06-064591096795")),
                ("api_key", json!("k")),
            ]),
            providers::LAVA => config(&[
                ("api_key", json!("k")),
                ("offer_id", json!("836b9fc5-7ae9-4a27-9642-592bc44072b7")),
            ]),
            providers::YOOKASSA => {
                config(&[("shop_id", json!("504367")), ("secret_key", json!("s"))])
            }
            providers::PLATEGA => config(&[
                ("merchant_id", json!("29ef6fa6-0d2b-466c-9604-0363a30436cc")),
                ("secret", json!("s")),
            ]),
            providers::YOOMONEY => config(&[
                ("wallet", json!("4100118412345678")),
                ("notification_secret", json!("s")),
            ]),
            providers::CLOUDPAYMENTS => {
                config(&[("public_id", json!("pk_1")), ("api_secret", json!("s"))])
            }
            providers::OVERPAY => config(&[
                ("project_id", json!("1")),
                ("login", json!("l")),
                ("password", json!("p")),
                ("certificate", json!(overpay::tests::MERCHANT)),
            ]),
            providers::ROLLYPAY => config(&[
                ("api_key", json!("rpk_live_abc123def456")),
                ("signing_secret", json!("s")),
            ]),
            providers::MULENPAY => config(&[
                ("shop_id", json!("5")),
                ("api_key", json!("k")),
                ("secret_key", json!("s")),
            ]),
            providers::PAL24 => config(&[
                ("api_token", json!("72|abc")),
                ("shop_id", json!("LXZv3R7Q8B")),
            ]),
            providers::KASSA_AI => config(&[
                ("shop_id", json!("777")),
                ("api_key", json!("k")),
                ("secret_word_2", json!("s")),
            ]),
            providers::AURAPAY => config(&[
                ("shop_id", json!("9bee9309-2585-4332-b63d-e1d897f7ce84")),
                ("api_key", json!("k")),
                ("secret_key", json!("s")),
            ]),
            providers::STARS => config(&[(STARS_TOKEN_FIELD, json!("1:abc"))]),
            _ => config(&[]),
        }
    }

    #[test]
    fn настроенные_системы_предлагаются_в_своей_валюте() {
        let registry = ProviderRegistry::builtin();
        let states: std::collections::HashMap<String, ProviderState> = registry
            .providers()
            .iter()
            .map(|p| {
                (
                    p.id().to_string(),
                    ProviderState {
                        enabled: true,
                        config: filled(p.id()),
                        channels: sn_domain::payment::PaymentChannels::ALL,
                        methods: Vec::new(),
                        label: Default::default(),
                    },
                )
            })
            .collect();
        for (currency, expected) in [
            (
                Currency::Usd,
                vec![
                    providers::CRYPTOPAY,
                    providers::HELEKET,
                    providers::LAVA,
                    providers::STARS,
                    providers::CRYPTOMUS,
                    providers::BALANCE,
                ],
            ),
            (
                Currency::Rub,
                vec![
                    providers::PLATEGA,
                    providers::YOOKASSA,
                    providers::YOOMONEY,
                    providers::STARS,
                    providers::CLOUDPAYMENTS,
                    providers::OVERPAY,
                    providers::CRYPTOPAY,
                    providers::CRYPTOMUS,
                    providers::LAVA,
                    providers::ROLLYPAY,
                    providers::MULENPAY,
                    providers::PAL24,
                    providers::KASSA_AI,
                    providers::AURAPAY,
                    providers::BALANCE,
                ],
            ),
        ] {
            registry.apply(RegistrySnapshot {
                currency,
                states: states.clone(),
                order: PaymentOrder::default().for_currency(currency).to_vec(),
            });
            let methods = registry.checkout_methods(&MethodsContext {
                currency,
                total: Money::new(49_900, currency),
                stars_total: Some(Stars(350)),
                balance: Some(Money::new(100_000, currency)),
                channel: None,
            });
            let ids: Vec<&str> = methods.iter().map(|m| m.provider).collect();
            assert_eq!(ids, expected, "{currency}");
            assert!(registry.is_available(providers::OVERPAY, Currency::Rub));
            assert!(!registry.is_available(providers::OVERPAY, Currency::Usd));
            assert!(registry.is_available(providers::ROLLYPAY, Currency::Rub));
            assert!(!registry.is_available(providers::ROLLYPAY, Currency::Usd));
            assert!(registry.accepts_webhooks(providers::ROLLYPAY));
            assert!(registry.is_available(providers::CRYPTOMUS, Currency::Rub));
            assert!(registry.is_available(providers::CRYPTOMUS, Currency::Usd));
            assert!(registry.accepts_webhooks(providers::CRYPTOMUS));
            for id in [
                providers::MULENPAY,
                providers::PAL24,
                providers::KASSA_AI,
                providers::AURAPAY,
            ] {
                assert!(registry.is_available(id, Currency::Rub), "{id}");
                assert!(!registry.is_available(id, Currency::Usd), "{id}");
                assert!(registry.accepts_webhooks(id), "{id}");
            }
        }
    }

    #[test]
    fn у_каждой_системы_свой_неизменный_id() {
        let all = builtin();
        let mut ids: Vec<&str> = all.iter().map(|p| p.id()).collect();
        ids.sort();
        let len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), len);
        for id in [
            providers::CRYPTOPAY,
            providers::HELEKET,
            providers::CRYPTOMUS,
            providers::LAVA,
            providers::YOOKASSA,
            providers::PLATEGA,
            providers::YOOMONEY,
            providers::CLOUDPAYMENTS,
            providers::OVERPAY,
            providers::ROLLYPAY,
            providers::MULENPAY,
            providers::PAL24,
            providers::KASSA_AI,
            providers::AURAPAY,
            providers::STARS,
            providers::BALANCE,
        ] {
            assert!(ids.contains(&id), "{id}");
        }
    }

    #[test]
    fn принимать_http_уведомления_могут_только_внешние_системы() {
        for provider in builtin() {
            if matches!(provider.id(), providers::STARS | providers::BALANCE) {
                assert!(!provider.accepts_http_webhooks(), "{}", provider.id());
            }
        }
    }

    #[test]
    fn без_настроек_ни_одна_внешняя_система_не_считается_настроенной() {
        let empty = crate::provider::ProviderConfig::default();
        for provider in builtin() {
            if provider.id() != providers::BALANCE {
                assert!(!provider.is_configured(&empty), "{}", provider.id());
                assert!(
                    provider.validate_settings(&empty).is_err(),
                    "{}",
                    provider.id()
                );
            }
        }
    }

    #[test]
    fn автосписание_и_возвраты_где_их_умеет_api() {
        let recurring: Vec<&str> = builtin()
            .iter()
            .filter(|p| p.supports_recurring())
            .map(|p| p.id())
            .collect();
        assert_eq!(recurring, [providers::YOOKASSA]);
        let refunds: Vec<&str> = builtin()
            .iter()
            .filter(|p| p.supports_refunds())
            .map(|p| p.id())
            .collect();
        assert_eq!(
            refunds,
            [
                providers::YOOKASSA,
                providers::PLATEGA,
                providers::CLOUDPAYMENTS,
                providers::OVERPAY,
                providers::MULENPAY,
                providers::PAL24,
                providers::KASSA_AI,
                providers::STARS
            ]
        );
    }

    #[tokio::test]
    async fn опрос_возвратов_объявляет_только_система_которая_умеет_спросить_статус() {
        let empty = crate::provider::ProviderConfig::default();
        let mut polling = Vec::new();
        for provider in builtin() {
            if provider.supports_refund_polling() {
                assert!(provider.supports_refunds(), "{}", provider.id());
                polling.push(provider.id());
            } else {
                assert_eq!(
                    provider.fetch_refund(&empty, "refund-1").await,
                    Ok(None),
                    "{}",
                    provider.id()
                );
            }
        }
        assert_eq!(
            polling,
            [
                providers::YOOKASSA,
                providers::PLATEGA,
                providers::CLOUDPAYMENTS,
                providers::OVERPAY,
                providers::PAL24
            ]
        );
    }

    #[test]
    fn у_каждой_системы_с_настройками_есть_секрет() {
        for provider in builtin() {
            let schema = provider.settings_schema();
            if !schema.is_empty() {
                assert!(schema.iter().any(|f| f.is_secret()), "{}", provider.id());
            }
        }
    }
}
