use crate::error::{PaymentError, PaymentResult};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::ids::{ClientId, PaymentId};
use sn_domain::money::{Amount, Currency};
use sn_domain::settings::{SettingField, missing_required};
use std::net::IpAddr;
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderConfig(pub serde_json::Map<String, serde_json::Value>);

impl ProviderConfig {
    pub fn new(map: serde_json::Map<String, serde_json::Value>) -> Self {
        ProviderConfig(map)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn require(&self, key: &'static str) -> PaymentResult<&str> {
        self.get(key)
            .ok_or_else(|| PaymentError::NotConfigured(format!("не задано поле {key}")))
    }

    pub fn flag(&self, key: &str) -> bool {
        match self.0.get(key) {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => matches!(s.trim(), "true" | "1" | "yes"),
            _ => false,
        }
    }

    pub fn set(&mut self, key: &str, value: serde_json::Value) {
        self.0.insert(key.to_string(), value);
    }

    pub fn integer(&self, key: &str) -> Option<i64> {
        match self.0.get(key) {
            Some(serde_json::Value::Number(n)) => n.as_i64(),
            Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn public_view(
        &self,
        schema: &[SettingField],
    ) -> serde_json::Map<String, serde_json::Value> {
        schema
            .iter()
            .map(|field| {
                let value = if field.is_secret() {
                    serde_json::json!({ "filled": self.get(field.key).is_some() })
                } else {
                    self.0
                        .get(field.key)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null)
                };
                (field.key.to_string(), value)
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    External,
    Balance,
    Stars,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClientLabel {
    pub title: &'static str,
    pub hint: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PayMethod {
    pub code: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
    pub min_minor: Option<i64>,
}

impl PayMethod {
    pub const fn new(code: &'static str, title: &'static str, hint: &'static str) -> Self {
        PayMethod {
            code,
            title,
            hint,
            min_minor: None,
        }
    }

    pub const fn min_minor(mut self, minor: i64) -> Self {
        self.min_minor = Some(minor);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderIssue {
    pub at: DateTime<Utc>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRef {
    pub client_id: ClientId,
    pub public_id: Uuid,
    pub telegram_id: Option<i64>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceRequest {
    pub payment_id: PaymentId,
    pub payment_public_id: Uuid,
    pub order_public_id: Uuid,
    pub amount: Amount,
    pub description: String,
    pub client: ClientRef,
    pub return_url: Option<String>,
    pub webhook_url: String,
    pub expires_in: Duration,
    pub save_payment_method: bool,
    #[serde(default)]
    pub method: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invoice {
    pub pay_url: String,
    pub external_id: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookRequest {
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub remote_ip: Option<IpAddr>,
    pub route_suffix: Option<String>,
    pub query: Option<String>,
}

impl WebhookRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PaymentRef {
    Payment(PaymentId),
    PublicId(Uuid),
    ProviderTxid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderStatus {
    Pending,
    Paid,
    Failed,
    Canceled,
    Refunded,
    PartiallyRefunded { refunded: Amount },
    RefundDeclined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMethod {
    pub token: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebhookReply {
    Empty {
        status: u16,
    },
    Text {
        status: u16,
        body: String,
    },
    Json {
        status: u16,
        body: serde_json::Value,
    },
}

impl WebhookReply {
    pub const fn ok() -> Self {
        WebhookReply::Empty { status: 200 }
    }

    pub const fn status(&self) -> u16 {
        match self {
            WebhookReply::Empty { status }
            | WebhookReply::Text { status, .. }
            | WebhookReply::Json { status, .. } => *status,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookOutcome {
    pub event_key: String,
    pub payment: Option<PaymentRef>,
    pub provider_txid: Option<String>,
    pub status: ProviderStatus,
    pub amount: Option<Amount>,
    pub saved_method: Option<SavedMethod>,
    pub reply: WebhookReply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundRequest {
    pub payment_id: PaymentId,
    pub provider_txid: String,
    pub amount: Amount,
    pub reason: Option<String>,
    pub key: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundOutcome {
    pub provider_refund_id: Option<String>,
    pub completed: bool,
}

#[async_trait]
pub trait PaymentProvider: Send + Sync + 'static {
    fn id(&self) -> &'static str;

    fn title(&self) -> &'static str;

    fn kind(&self) -> ProviderKind {
        ProviderKind::External
    }

    fn currencies(&self) -> &'static [Currency];

    fn client_label(&self, currency: Currency) -> ClientLabel;

    fn methods(&self, _currency: Currency) -> Vec<PayMethod> {
        Vec::new()
    }

    fn method(&self, currency: Currency, code: &str) -> Option<PayMethod> {
        self.methods(currency).into_iter().find(|m| m.code == code)
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        Vec::new()
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        missing_required(&self.settings_schema(), &config.0).is_empty()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        let missing = missing_required(&self.settings_schema(), &config.0);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(PaymentError::NotConfigured(format!(
                "не заполнены поля: {}",
                missing.join(", ")
            )))
        }
    }

    fn enabled_by_default(&self) -> bool {
        false
    }

    fn accepts_http_webhooks(&self) -> bool {
        false
    }

    fn verifies_webhooks_by_api(&self) -> bool {
        false
    }

    fn polls_status(&self) -> bool {
        false
    }

    fn issue(&self) -> Option<ProviderIssue> {
        None
    }

    fn supports_recurring(&self) -> bool {
        false
    }

    fn supports_refunds(&self) -> bool {
        false
    }

    fn supports_refund_polling(&self) -> bool {
        false
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice>;

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome>;

    async fn fetch_status(
        &self,
        _config: &ProviderConfig,
        _external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        Ok(None)
    }

    async fn charge_saved(
        &self,
        _config: &ProviderConfig,
        _request: &InvoiceRequest,
        _token: &str,
    ) -> PaymentResult<WebhookOutcome> {
        Err(PaymentError::Unsupported("автосписание"))
    }

    async fn refund(
        &self,
        _config: &ProviderConfig,
        _request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        Err(PaymentError::Unsupported("возврат через платёжную систему"))
    }

    async fn fetch_refund(
        &self,
        _config: &ProviderConfig,
        _provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        Ok(None)
    }
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}
