use crate::error::{PaymentError, PaymentResult};
use crate::provider::*;
use async_trait::async_trait;
use sn_domain::money::Currency;
use sn_domain::payment::providers;

pub struct Balance;

#[async_trait]
impl PaymentProvider for Balance {
    fn id(&self) -> &'static str {
        providers::BALANCE
    }

    fn title(&self) -> &'static str {
        "Баланс"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Balance
    }

    fn currencies(&self) -> &'static [Currency] {
        &Currency::ALL
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "С баланса",
            hint: "Списывается вся сумма сразу",
        }
    }

    fn is_configured(&self, _config: &ProviderConfig) -> bool {
        true
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        _config: &ProviderConfig,
        _request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        Err(PaymentError::Unsupported(
            "оплата с баланса проводится без счёта",
        ))
    }

    async fn parse_webhook(
        &self,
        _config: &ProviderConfig,
        _request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        Err(PaymentError::Unsupported("у баланса нет уведомлений"))
    }
}
