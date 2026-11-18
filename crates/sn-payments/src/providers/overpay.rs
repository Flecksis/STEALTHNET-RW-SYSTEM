use crate::error::{PaymentError, PaymentResult};
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use sn_domain::money::Currency;
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;

pub struct Overpay;

#[async_trait]
impl PaymentProvider for Overpay {
    fn id(&self) -> &'static str {
        providers::OVERPAY
    }

    fn title(&self) -> &'static str {
        "Overpay"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: "Карты и СБП",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::url("api_url", "Адрес API", "Выдаёт Overpay при подключении"),
            SettingField::text("project_id", "ID проекта", "Кабинет Overpay"),
            SettingField::text("login", "Логин", "Кабинет Overpay"),
            SettingField::secret("password", "Пароль", "Кабинет Overpay"),
        ]
    }

    fn is_configured(&self, _config: &ProviderConfig) -> bool {
        false
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if config.get("api_url").is_some_and(util::is_https) {
            Ok(())
        } else {
            Err(PaymentError::NotConfigured(
                "адрес API Overpay должен начинаться с https://".into(),
            ))
        }
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        _config: &ProviderConfig,
        _request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        Err(PaymentError::NotImplemented(providers::OVERPAY))
    }

    async fn parse_webhook(
        &self,
        _config: &ProviderConfig,
        _request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        Err(PaymentError::NotImplemented(providers::OVERPAY))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;

    #[tokio::test]
    async fn overpay_выключен_пока_нечем_проверять_уведомления() {
        let settings = config(&[
            ("api_url", json!("https://api.overpay.io")),
            ("project_id", json!("17")),
            ("login", json!("shop")),
            ("password", json!("secret")),
        ]);
        assert!(Overpay.validate_settings(&settings).is_ok());
        assert!(!Overpay.is_configured(&settings));
        let mut plain = settings.clone();
        plain.set("api_url", json!("http://api.overpay.io"));
        assert!(Overpay.validate_settings(&plain).is_err());
        assert_eq!(
            Overpay.parse_webhook(&settings, &hook(&[], "{}")).await,
            Err(PaymentError::NotImplemented(providers::OVERPAY))
        );
        assert_eq!(
            Overpay
                .create_invoice(&settings, &invoice(rub(49_900), None))
                .await,
            Err(PaymentError::NotImplemented(providers::OVERPAY))
        );
    }
}
