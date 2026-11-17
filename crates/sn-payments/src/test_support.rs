use crate::provider::{ClientRef, InvoiceRequest, ProviderConfig, WebhookRequest};
use bytes::Bytes;
use sn_domain::ids::{ClientId, PaymentId};
use sn_domain::money::{Amount, PayCurrency};
use uuid::Uuid;

pub(crate) const PAYMENT: Uuid = Uuid::from_u128(0x6a1c_0c8e_3f4b_4d2a_9b7e_51c0_2d3e_4f50);
pub(crate) const CLIENT: Uuid = Uuid::from_u128(0x0b9d_77aa_1c2d_4e3f_8a4b_5c6d_7e8f_9012);

pub(crate) fn usd(minor: i64) -> Amount {
    Amount::new(minor, PayCurrency::Usd)
}

pub(crate) fn rub(minor: i64) -> Amount {
    Amount::new(minor, PayCurrency::Rub)
}

pub(crate) fn config(pairs: &[(&str, serde_json::Value)]) -> ProviderConfig {
    let mut config = ProviderConfig::default();
    for (key, value) in pairs {
        config.set(key, value.clone());
    }
    config
}

pub(crate) fn invoice(amount: Amount, email: Option<&str>) -> InvoiceRequest {
    InvoiceRequest {
        payment_id: PaymentId(41),
        payment_public_id: PAYMENT,
        order_public_id: Uuid::from_u128(7),
        amount,
        description: "Стандарт · 3 месяца".into(),
        client: ClientRef {
            client_id: ClientId(7),
            public_id: CLIENT,
            telegram_id: Some(5_093_118_422),
            email: email.map(str::to_string),
        },
        return_url: Some("https://panel.example.com/cabinet/payment-wait".into()),
        webhook_url: "https://panel.example.com/api/webhooks/pay/test".into(),
        expires_in: chrono::Duration::hours(1),
        save_payment_method: false,
    }
}

pub(crate) fn hook(headers: &[(&str, &str)], body: impl Into<Bytes>) -> WebhookRequest {
    WebhookRequest {
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: body.into(),
        remote_ip: None,
        route_suffix: None,
        query: None,
    }
}
