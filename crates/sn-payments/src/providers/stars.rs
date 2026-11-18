use crate::error::{PaymentError, PaymentResult};
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use uuid::Uuid;

pub const STARS_TOKEN_FIELD: &str = "bot_token";

const TELEGRAM_API: &str = "https://api.telegram.org";
const PAGE: u32 = 100;
const MAX_REFUND_PAGES: u32 = 50;

pub struct TelegramStars {
    http: reqwest::Client,
    api_base: String,
}

impl Default for TelegramStars {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarsPreCheckout {
    pub query_id: String,
    pub invoice_payload: String,
    pub total_amount: i64,
    pub currency: String,
    pub telegram_user_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StarsPaymentEvent {
    SuccessfulPayment,
    RefundedPayment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarsPayment {
    pub event: StarsPaymentEvent,
    pub invoice_payload: String,
    pub total_amount: i64,
    pub currency: String,
    pub telegram_payment_charge_id: String,
    pub telegram_user_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StarDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarTransaction {
    pub id: String,
    pub amount: i64,
    pub date: i64,
    pub direction: StarDirection,
    pub partner: String,
    pub user_id: Option<i64>,
    pub invoice_payload: Option<String>,
    #[serde(default)]
    pub transaction_type: Option<String>,
}

impl StarTransaction {
    fn with_user_invoice(&self, direction: StarDirection) -> bool {
        self.direction == direction
            && self.partner == "user"
            && self.amount > 0
            && !self.id.trim().is_empty()
            && self
                .transaction_type
                .as_deref()
                .is_none_or(|kind| kind == "invoice_payment")
    }

    pub fn payment_event(&self) -> Option<StarsPayment> {
        if !self.with_user_invoice(StarDirection::Incoming) {
            return None;
        }
        Some(StarsPayment {
            event: StarsPaymentEvent::SuccessfulPayment,
            invoice_payload: self.invoice_payload.clone()?,
            total_amount: self.amount,
            currency: "XTR".into(),
            telegram_payment_charge_id: self.id.clone(),
            telegram_user_id: self.user_id?,
        })
    }

    pub fn is_refund(&self) -> bool {
        self.with_user_invoice(StarDirection::Outgoing) && self.user_id.is_some()
    }

    pub fn refund_event(&self, invoice_payload: &str) -> Option<StarsPayment> {
        if !self.is_refund() || invoice_payload.trim().is_empty() {
            return None;
        }
        Some(StarsPayment {
            event: StarsPaymentEvent::RefundedPayment,
            invoice_payload: invoice_payload.to_string(),
            total_amount: self.amount,
            currency: "XTR".into(),
            telegram_payment_charge_id: self.id.clone(),
            telegram_user_id: self.user_id?,
        })
    }
}

#[derive(Deserialize)]
struct BotReply<T> {
    ok: bool,
    result: Option<T>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct RawTransactions {
    transactions: Vec<RawTransaction>,
}

#[derive(Deserialize)]
struct RawTransaction {
    id: String,
    amount: i64,
    #[serde(default)]
    date: i64,
    #[serde(default)]
    source: Option<RawPartner>,
    #[serde(default)]
    receiver: Option<RawPartner>,
}

#[derive(Deserialize)]
struct RawPartner {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    transaction_type: Option<String>,
    #[serde(default)]
    user: Option<RawUser>,
    #[serde(default)]
    invoice_payload: Option<String>,
}

#[derive(Deserialize)]
struct RawUser {
    id: i64,
}

impl From<RawTransaction> for StarTransaction {
    fn from(raw: RawTransaction) -> Self {
        let (direction, partner) = match (raw.source, raw.receiver) {
            (Some(source), _) => (StarDirection::Incoming, Some(source)),
            (None, receiver) => (StarDirection::Outgoing, receiver),
        };
        StarTransaction {
            id: raw.id,
            amount: raw.amount,
            date: raw.date,
            direction,
            partner: partner.as_ref().map(|p| p.kind.clone()).unwrap_or_default(),
            user_id: partner.as_ref().and_then(|p| p.user.as_ref()).map(|u| u.id),
            transaction_type: partner.as_ref().and_then(|p| p.transaction_type.clone()),
            invoice_payload: partner.and_then(|p| p.invoice_payload),
        }
    }
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

impl TelegramStars {
    pub fn new() -> Self {
        Self::with_api_base(TELEGRAM_API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        TelegramStars {
            http: http_client(),
            api_base: api_base.into().trim_end_matches('/').to_string(),
        }
    }

    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        config: &ProviderConfig,
        method: &str,
        body: &serde_json::Value,
    ) -> PaymentResult<T> {
        let token = config.require(STARS_TOKEN_FIELD)?;
        let reply = util::send(
            self.http
                .post(format!("{}/bot{token}/{method}", self.api_base))
                .json(body),
        )
        .await?;
        let parsed: BotReply<T> = reply.typed()?;
        match (parsed.ok, parsed.result) {
            (true, Some(result)) => Ok(result),
            _ => Err(reply.failure(parsed.description.as_deref())),
        }
    }

    pub async fn star_transactions(
        &self,
        config: &ProviderConfig,
        offset: u32,
        limit: u32,
    ) -> PaymentResult<Vec<StarTransaction>> {
        let raw: RawTransactions = self
            .call(
                config,
                "getStarTransactions",
                &serde_json::json!({ "offset": offset, "limit": limit.clamp(1, PAGE) }),
            )
            .await?;
        Ok(raw.transactions.into_iter().map(Into::into).collect())
    }

    pub async fn refund_star_payment(
        &self,
        config: &ProviderConfig,
        user_id: i64,
        charge_id: &str,
    ) -> PaymentResult<RefundOutcome> {
        let refunded: bool = self
            .call(
                config,
                "refundStarPayment",
                &serde_json::json!({
                    "user_id": user_id,
                    "telegram_payment_charge_id": charge_id,
                }),
            )
            .await?;
        if refunded {
            Ok(RefundOutcome {
                provider_refund_id: Some(charge_id.to_string()),
                completed: true,
            })
        } else {
            Err(PaymentError::Rejected("Telegram не вернул звёзды".into()))
        }
    }

    async fn has_transaction_at(
        &self,
        config: &ProviderConfig,
        offset: u32,
    ) -> PaymentResult<bool> {
        Ok(!self.star_transactions(config, offset, 1).await?.is_empty())
    }

    pub async fn transaction_count(&self, config: &ProviderConfig) -> PaymentResult<u32> {
        if !self.has_transaction_at(config, 0).await? {
            return Ok(0);
        }
        let mut present = 0u32;
        let mut absent = 1u32;
        while self.has_transaction_at(config, absent).await? {
            present = absent;
            absent = match absent.checked_mul(2) {
                Some(next) => next,
                None => return Ok(u32::MAX),
            };
        }
        while absent - present > 1 {
            let middle = present + (absent - present) / 2;
            if self.has_transaction_at(config, middle).await? {
                present = middle;
            } else {
                absent = middle;
            }
        }
        Ok(absent)
    }

    pub async fn find_incoming(
        &self,
        config: &ProviderConfig,
        charge_id: &str,
    ) -> PaymentResult<Option<StarTransaction>> {
        let mut end = self.transaction_count(config).await?;
        for _ in 0..MAX_REFUND_PAGES {
            if end == 0 {
                break;
            }
            let start = end.saturating_sub(PAGE);
            let page = self.star_transactions(config, start, end - start).await?;
            if let Some(found) = page
                .into_iter()
                .find(|t| t.id == charge_id && t.direction == StarDirection::Incoming)
            {
                return Ok(Some(found));
            }
            end = start;
        }
        Ok(None)
    }
}

#[async_trait]
impl PaymentProvider for TelegramStars {
    fn id(&self) -> &'static str {
        providers::STARS
    }

    fn title(&self) -> &'static str {
        "Звёзды Telegram"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Stars
    }

    fn currencies(&self) -> &'static [Currency] {
        &Currency::ALL
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Звёзды Telegram",
            hint: "Оплата в Telegram",
        }
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        config.get(STARS_TOKEN_FIELD).is_some()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        config.require(STARS_TOKEN_FIELD).map(|_| ())
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn supports_refunds(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        let token = config.require(STARS_TOKEN_FIELD)?;
        if request.amount.currency != PayCurrency::Xtr {
            return Err(PaymentError::UnsupportedCurrency(
                request.amount.currency.code().into(),
            ));
        }
        let title = truncate(&request.description, 32);
        let body = serde_json::json!({
            "title": title,
            "description": truncate(&request.description, 255),
            "payload": request.payment_public_id.to_string(),
            "provider_token": "",
            "currency": "XTR",
            "prices": [{ "label": title, "amount": request.amount.minor }],
        });
        let response = self
            .http
            .post(format!("{}/bot{token}/createInvoiceLink", self.api_base))
            .json(&body)
            .send()
            .await
            .map_err(PaymentError::transport)?;
        let status = response.status();
        let parsed: BotReply<String> = response.json().await.map_err(|_| PaymentError::Http {
            status: status.as_u16(),
        })?;
        match (parsed.ok, parsed.result) {
            (true, Some(link)) => Ok(Invoice {
                pay_url: link,
                external_id: None,
                expires_at: None,
                payload: serde_json::json!({ "stars": request.amount.minor }),
            }),
            _ => Err(PaymentError::Rejected(
                parsed
                    .description
                    .unwrap_or_else(|| format!("HTTP {status}")),
            )),
        }
    }

    async fn parse_webhook(
        &self,
        _config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let event: StarsPayment = serde_json::from_slice(&request.body)
            .map_err(|e| PaymentError::BadPayload(e.to_string()))?;
        if event.currency != "XTR" {
            return Err(PaymentError::BadPayload(format!(
                "валюта {}",
                event.currency
            )));
        }
        let payment = Uuid::parse_str(&event.invoice_payload)
            .map(PaymentRef::PublicId)
            .map_err(|_| PaymentError::BadPayload("payload счёта не наш".into()))?;
        let status = match event.event {
            StarsPaymentEvent::SuccessfulPayment => ProviderStatus::Paid,
            StarsPaymentEvent::RefundedPayment => ProviderStatus::Refunded,
        };
        Ok(WebhookOutcome {
            event_key: format!(
                "{}:{}",
                serde_json::to_value(event.event)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
                event.telegram_payment_charge_id
            ),
            payment: Some(payment),
            provider_txid: Some(event.telegram_payment_charge_id),
            status,
            amount: Some(Amount::new(event.total_amount, PayCurrency::Xtr)),
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        })
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Xtr)?;
        let charge = request.provider_txid.trim();
        if charge.is_empty() {
            return Err(PaymentError::Rejected(
                "у платежа нет идентификатора оплаты Telegram".into(),
            ));
        }
        let found = self.find_incoming(config, charge).await?.ok_or_else(|| {
            PaymentError::Rejected(
                "оплата звёздами не найдена среди последних транзакций бота".into(),
            )
        })?;
        if found.amount != request.amount.minor {
            return Err(PaymentError::Rejected(
                "звёзды возвращаются только всей суммой оплаты".into(),
            ));
        }
        let user = found
            .user_id
            .ok_or_else(|| PaymentError::Rejected("в транзакции звёзд нет плательщика".into()))?;
        self.refund_star_payment(config, user, charge).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use bytes::Bytes;
    use serde_json::json;
    use sn_domain::ids::PaymentId;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    const TOKEN: &str = "123:abc";

    #[tokio::test]
    async fn оплата_звёздами_из_бота() {
        let payment = Uuid::from_u128(9);
        let event = StarsPayment {
            event: StarsPaymentEvent::SuccessfulPayment,
            invoice_payload: payment.to_string(),
            total_amount: 350,
            currency: "XTR".into(),
            telegram_payment_charge_id: "charge-1".into(),
            telegram_user_id: 5_093_118_422,
        };
        let request = WebhookRequest {
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&event).unwrap()),
            remote_ip: None,
            route_suffix: None,
            query: None,
        };
        let outcome = TelegramStars::new()
            .parse_webhook(&ProviderConfig::default(), &request)
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(Amount::new(350, PayCurrency::Xtr)));
        assert_eq!(outcome.event_key, "successful_payment:charge-1");
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(payment)));
    }

    #[test]
    fn без_токена_бота_звёзды_не_настроены() {
        let stars = TelegramStars::new();
        assert!(!stars.is_configured(&ProviderConfig::default()));
        let mut config = ProviderConfig::default();
        config.set(STARS_TOKEN_FIELD, serde_json::json!("123:abc"));
        assert!(stars.is_configured(&config));
        assert!(!stars.accepts_http_webhooks());
    }

    struct Ledger {
        transactions: Vec<serde_json::Value>,
    }

    impl Ledger {
        fn new(count: usize, refunded: Option<usize>) -> Self {
            let mut transactions: Vec<serde_json::Value> = (0..count)
                .map(|i| {
                    json!({
                        "id": format!("charge-{i}"),
                        "amount": 350,
                        "date": 1_763_300_000 + i as i64,
                        "source": {
                            "type": "user", "transaction_type": "invoice_payment",
                            "user": {"id": 1_000 + i as i64, "is_bot": false, "first_name": "A"},
                            "invoice_payload": PAYMENT.to_string()
                        }
                    })
                })
                .collect();
            if let Some(i) = refunded {
                transactions.push(json!({
                    "id": format!("charge-{i}"), "amount": 350, "date": 1_763_400_000,
                    "receiver": {"type": "user", "user": {"id": 1_000 + i as i64, "is_bot": false, "first_name": "A"}}
                }));
            }
            Ledger { transactions }
        }
    }

    impl Respond for Ledger {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let offset = body["offset"].as_u64().unwrap() as usize;
            let limit = body["limit"].as_u64().unwrap() as usize;
            let page: Vec<serde_json::Value> = self
                .transactions
                .iter()
                .skip(offset)
                .take(limit)
                .cloned()
                .collect();
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "result": {"transactions": page}}))
        }
    }

    async fn telegram(ledger: Ledger) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/getStarTransactions")))
            .respond_with(ledger)
            .mount(&server)
            .await;
        server
    }

    fn settings() -> ProviderConfig {
        config(&[(STARS_TOKEN_FIELD, json!(TOKEN))])
    }

    fn refund_request(charge: &str, stars: i64) -> RefundRequest {
        RefundRequest {
            payment_id: PaymentId(41),
            provider_txid: charge.into(),
            amount: Amount::new(stars, PayCurrency::Xtr),
            reason: None,
            key: Uuid::from_u128(5),
            email: None,
            description: None,
        }
    }

    #[tokio::test]
    async fn транзакции_звёзд_читаются_по_смещению() {
        let server = telegram(Ledger::new(250, Some(3))).await;
        let stars = TelegramStars::with_api_base(server.uri());
        assert_eq!(stars.transaction_count(&settings()).await.unwrap(), 251);
        let page = stars
            .star_transactions(&settings(), 100, 100)
            .await
            .unwrap();
        assert_eq!(page.len(), 100);
        assert_eq!(page[0].id, "charge-100");
        assert_eq!(page[0].direction, StarDirection::Incoming);
        assert_eq!(page[0].user_id, Some(1_100));
        let event = page[0].payment_event().unwrap();
        assert_eq!(event.telegram_payment_charge_id, "charge-100");
        assert_eq!(event.invoice_payload, PAYMENT.to_string());
        let tail = stars
            .star_transactions(&settings(), 250, 100)
            .await
            .unwrap();
        assert_eq!(tail[0].direction, StarDirection::Outgoing);
        assert_eq!(tail[0].payment_event(), None);
        let empty = telegram(Ledger::new(0, None)).await;
        assert_eq!(
            TelegramStars::with_api_base(empty.uri())
                .transaction_count(&settings())
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn сверка_отличает_возврат_оплаты_от_подарка_и_вывода() {
        let server = MockServer::start().await;
        let user = |id: i64| json!({"id": id, "is_bot": false, "first_name": "A"});
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/getStarTransactions")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {"transactions": [
                    {"id": "charge-1", "amount": 350, "date": 1, "receiver": {
                        "type": "user", "transaction_type": "invoice_payment", "user": user(42)}},
                    {"id": "gift-1", "amount": 50, "date": 2, "receiver": {
                        "type": "user", "transaction_type": "gift_purchase", "user": user(42)}},
                    {"id": "withdrawal-1", "amount": 1000, "date": 3, "receiver": {"type": "fragment"}},
                    {"id": "media-1", "amount": 10, "date": 4, "source": {
                        "type": "user", "transaction_type": "paid_media_payment", "user": user(43),
                        "paid_media_payload": "album"}},
                    {"id": "charge-2", "amount": 350, "date": 5, "receiver": {
                        "type": "user", "user": user(44), "invoice_payload": PAYMENT.to_string()}}
                ]}
            })))
            .mount(&server)
            .await;
        let page = TelegramStars::with_api_base(server.uri())
            .star_transactions(&settings(), 0, 100)
            .await
            .unwrap();
        let refunds: Vec<&str> = page
            .iter()
            .filter(|t| t.is_refund())
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(refunds, ["charge-1", "charge-2"]);
        assert!(page.iter().all(|t| t.payment_event().is_none()));
        assert_eq!(page[1].transaction_type.as_deref(), Some("gift_purchase"));
        assert_eq!(page[1].refund_event(&PAYMENT.to_string()), None);
        assert_eq!(page[2].refund_event(&PAYMENT.to_string()), None);
        assert_eq!(page[0].refund_event("  "), None);
        let event = page[0].refund_event(&PAYMENT.to_string()).unwrap();
        assert_eq!(
            event,
            StarsPayment {
                event: StarsPaymentEvent::RefundedPayment,
                invoice_payload: PAYMENT.to_string(),
                total_amount: 350,
                currency: "XTR".into(),
                telegram_payment_charge_id: "charge-1".into(),
                telegram_user_id: 42,
            }
        );
        let outcome = TelegramStars::new()
            .parse_webhook(
                &ProviderConfig::default(),
                &WebhookRequest {
                    headers: Vec::new(),
                    body: Bytes::from(serde_json::to_vec(&event).unwrap()),
                    remote_ip: None,
                    route_suffix: None,
                    query: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Refunded);
        assert_eq!(outcome.event_key, "refunded_payment:charge-1");
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(
            page[4].invoice_payload.as_deref(),
            Some(PAYMENT.to_string().as_str())
        );
    }

    #[tokio::test]
    async fn возврат_звёзд_находит_плательщика_по_транзакции() {
        let server = telegram(Ledger::new(250, None)).await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/refundStarPayment")))
            .and(body_json(
                json!({"user_id": 1_237, "telegram_payment_charge_id": "charge-237"}),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": true})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let stars = TelegramStars::with_api_base(server.uri());
        let outcome = stars
            .refund(&settings(), &refund_request("charge-237", 350))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            RefundOutcome {
                provider_refund_id: Some("charge-237".into()),
                completed: true
            }
        );
        assert!(matches!(
            stars
                .refund(&settings(), &refund_request("charge-237", 100))
                .await,
            Err(PaymentError::Rejected(_))
        ));
        assert!(matches!(
            stars
                .refund(&settings(), &refund_request("charge-999", 350))
                .await,
            Err(PaymentError::Rejected(_))
        ));
        assert!(matches!(
            stars
                .refund(
                    &settings(),
                    &RefundRequest {
                        amount: rub(35_000),
                        ..refund_request("charge-237", 350)
                    }
                )
                .await,
            Err(PaymentError::UnsupportedCurrency(_))
        ));
    }

    #[tokio::test]
    async fn повторный_возврат_звёзд_отклоняет_telegram() {
        let server = telegram(Ledger::new(5, Some(2))).await;
        Mock::given(method("POST"))
            .and(path(format!("/bot{TOKEN}/refundStarPayment")))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "ok": false, "error_code": 400, "description": "Bad Request: CHARGE_ALREADY_REFUNDED"
            })))
            .mount(&server)
            .await;
        let result = TelegramStars::with_api_base(server.uri())
            .refund(&settings(), &refund_request("charge-2", 350))
            .await;
        assert_eq!(
            result,
            Err(PaymentError::Rejected(
                "Bad Request: CHARGE_ALREADY_REFUNDED".into()
            ))
        );
    }
}
