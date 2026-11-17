use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use uuid::Uuid;

const API: &str = "https://app.platega.io";
const METHODS: &[&str] = &["2", "3", "11", "12", "13", "14"];

pub struct Platega {
    http: reqwest::Client,
    api_base: String,
}

impl Default for Platega {
    fn default() -> Self {
        Self::new()
    }
}

impl Platega {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Platega {
            http: http_client(),
            api_base: api_base.into().trim_end_matches('/').to_string(),
        }
    }

    fn request(
        &self,
        config: &ProviderConfig,
        method: reqwest::Method,
        path: &str,
    ) -> PaymentResult<reqwest::RequestBuilder> {
        Ok(self
            .http
            .request(method, format!("{}{path}", self.api_base))
            .header("X-MerchantId", config.require("merchant_id")?)
            .header("X-Secret", config.require("secret")?))
    }

    async fn transaction(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Option<Json>> {
        let id = util::safe_id(id)?;
        let reply = util::send(self.request(
            config,
            reqwest::Method::GET,
            &format!("/transaction/{id}"),
        )?)
        .await?;
        if reply.status == 404 {
            return Ok(None);
        }
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(message(&parsed)));
        }
        if parsed.get("id").and_then(Json::as_str) != Some(id) {
            return Err(util::bad("Platega вернула другую транзакцию"));
        }
        Ok(Some(parsed))
    }
}

fn message(body: &Json) -> Option<&str> {
    body.get("message")
        .or_else(|| body.get("code"))
        .and_then(Json::as_str)
}

fn transaction_amount(tx: &Json) -> PaymentResult<Amount> {
    match tx.get("paymentDetails") {
        Some(details @ Json::Object(_)) => {
            util::json_amount(details.get("amount"), details.get("currency"))
        }
        _ => util::json_amount(tx.get("amount"), tx.get("currency")),
    }
}

fn reference(payload: Option<&str>, id: &str) -> Option<PaymentRef> {
    util::public_ref(payload).or_else(|| Some(PaymentRef::ProviderTxid(id.to_string())))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefundState {
    Succeeded,
    Declined,
    InProgress,
}

fn refund_state(raw: Option<&str>) -> RefundState {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("succeeded") => RefundState::Succeeded,
        Some("declined" | "failed" | "canceled" | "cancelled" | "rejected") => {
            RefundState::Declined
        }
        Some(_) => RefundState::InProgress,
    }
}

fn transaction_outcome(tx: &Json, id: &str) -> PaymentResult<WebhookOutcome> {
    let status_text = tx
        .get("status")
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет статуса транзакции"))?;
    let status = match status_text {
        "CONFIRMED" => ProviderStatus::Paid,
        "CANCELED" => ProviderStatus::Canceled,
        "CHARGEBACKED" => ProviderStatus::Refunded,
        _ => ProviderStatus::Pending,
    };
    Ok(WebhookOutcome {
        event_key: format!("{id}:{status_text}"),
        payment: reference(tx.get("payload").and_then(Json::as_str), id),
        provider_txid: Some(id.to_string()),
        status,
        amount: Some(transaction_amount(tx)?),
        saved_method: None,
        reply: WebhookReply::ok(),
    })
}

#[async_trait]
impl PaymentProvider for Platega {
    fn id(&self) -> &'static str {
        providers::PLATEGA
    }

    fn title(&self) -> &'static str {
        "Platega"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency) -> ClientLabel {
        ClientLabel {
            title: "СБП",
            hint: "Через приложение банка",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text("merchant_id", "MerchantId", "Кабинет Platega → Настройки"),
            SettingField::secret("secret", "Секрет", "Кабинет Platega → Настройки"),
            SettingField::select(
                "method",
                "Способ оплаты",
                "2 — СБП, 11 — карты, 12 — международные карты, 13 — криптовалюта, 14 — SberPay",
                METHODS,
            )
            .with_default("2"),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if !matches!(config.get("merchant_id").map(Uuid::parse_str), Some(Ok(_))) {
            return Err(PaymentError::NotConfigured(
                "MerchantId Platega — это UUID из кабинета".into(),
            ));
        }
        match config.get("method") {
            Some(method) if !METHODS.contains(&method) => Err(PaymentError::NotConfigured(
                format!("способ оплаты Platega: {}", METHODS.join(", ")),
            )),
            _ => Ok(()),
        }
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn accepts_http_webhooks(&self) -> bool {
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
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let method: i64 = config
            .get("method")
            .unwrap_or("2")
            .parse()
            .map_err(|_| PaymentError::NotConfigured("способ оплаты Platega".into()))?;
        let return_url = util::return_url(request)
            .ok_or_else(|| PaymentError::NotConfigured("не задан адрес панели".into()))?;
        let mut metadata =
            Json::object([("userId", Json::from(request.client.public_id.to_string()))]);
        if let Some(email) = util::client_email(request) {
            metadata.push("user_email", Json::from(email));
        }
        let body = Json::object([
            ("paymentMethod", Json::from(method)),
            (
                "paymentDetails",
                Json::object([
                    ("amount", Json::number(request.amount.units())),
                    ("currency", Json::from("RUB")),
                ]),
            ),
            (
                "description",
                Json::from(util::clip(&request.description, 255)),
            ),
            ("return", Json::from(return_url.clone())),
            ("failedUrl", Json::from(return_url)),
            ("payload", Json::from(request.payment_public_id.to_string())),
            ("metadata", metadata),
        ]);
        let reply = util::send(
            self.request(config, reqwest::Method::POST, "/transaction/process")?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(message(&parsed)));
        }
        let id = parsed
            .get("transactionId")
            .or_else(|| parsed.get("id"))
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| util::bad("Platega не вернула id транзакции"))?;
        let pay_url = ["redirect", "url", "paymentUrl"]
            .iter()
            .filter_map(|key| parsed.get(key).and_then(Json::as_str))
            .find(|u| util::is_https(u))
            .ok_or_else(|| util::bad("Platega не вернула ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "transactionId": id,
                "status": parsed.get("status").and_then(Json::as_str),
                "expiresIn": parsed.get("expiresIn").and_then(Json::as_str),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let merchant = config.require("merchant_id")?;
        let secret = config.require("secret")?;
        let (Some(got_merchant), Some(got_secret)) =
            (request.header("x-merchantid"), request.header("x-secret"))
        else {
            return Err(PaymentError::BadSignature);
        };
        let merchant_ok = util::secret_eq(merchant, got_merchant.trim());
        let secret_ok = util::secret_eq(secret, got_secret.trim());
        if !(merchant_ok & secret_ok) {
            return Err(PaymentError::BadSignature);
        }
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        let id = body
            .get("id")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет id"))?;
        let id = util::safe_id(id)?;
        let status = body
            .get("status")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет status"))?;
        if status.starts_with("SUBSCRIPTION_") {
            return Ok(util::skipped(
                format!("{id}:{status}"),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        let payload = body.get("payload").and_then(Json::as_str);
        if let Some(refund_id) = body.get("refundId").and_then(Json::as_str) {
            let event_key = format!("{id}:refund:{refund_id}");
            let refunded = || util::json_amount(body.get("refundAmount"), body.get("currency"));
            let (refund_status, amount) =
                match refund_state(body.get("refundStatus").and_then(Json::as_str)) {
                    RefundState::InProgress => {
                        return Ok(util::skipped(
                            event_key,
                            reference(payload, id),
                            Some(id.to_string()),
                            WebhookReply::ok(),
                        ));
                    }
                    RefundState::Declined => (ProviderStatus::RefundDeclined, refunded().ok()),
                    RefundState::Succeeded if status == "CHARGEBACKED" => {
                        (ProviderStatus::Refunded, Some(refunded()?))
                    }
                    RefundState::Succeeded => {
                        let refunded = refunded()?;
                        (
                            ProviderStatus::PartiallyRefunded { refunded },
                            Some(refunded),
                        )
                    }
                };
            return Ok(WebhookOutcome {
                event_key,
                payment: reference(payload, id),
                provider_txid: Some(id.to_string()),
                status: refund_status,
                amount,
                saved_method: None,
                reply: WebhookReply::ok(),
            });
        }
        let tx = self
            .transaction(config, id)
            .await?
            .ok_or(PaymentError::BadSignature)?;
        transaction_outcome(&tx, id)
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        match self.transaction(config, external_id).await? {
            Some(tx) => transaction_outcome(&tx, external_id).map(Some),
            None => Err(PaymentError::Rejected(
                "транзакция не найдена в Platega".into(),
            )),
        }
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let id = util::safe_id(&request.provider_txid)?;
        let tx = self
            .transaction(config, id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("транзакция не найдена в Platega".into()))?;
        let paid = transaction_amount(&tx)?;
        let builder = if paid == request.amount {
            self.request(
                config,
                reqwest::Method::POST,
                &format!("/transaction/{id}/cancel"),
            )?
        } else {
            self.request(
                config,
                reqwest::Method::POST,
                &format!("/transaction/{id}/cancel-partial"),
            )?
            .header(
                "Idempotency-Key",
                util::idempotency_key(&[
                    "refund",
                    &request.payment_id.to_string(),
                    id,
                    &request.amount.minor.to_string(),
                    request.reason.as_deref().unwrap_or_default(),
                ]),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(Json::object([("amount", Json::number(request.amount.units()))]).to_bytes())
        };
        let reply = util::send(builder).await?;
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(message(&parsed)));
        }
        Ok(RefundOutcome {
            provider_refund_id: parsed
                .get("refundOperationId")
                .and_then(Json::as_str)
                .map(str::to_string),
            completed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;
    use sn_domain::ids::PaymentId;
    use sn_domain::payment::{AmountCheck, PaymentStatus, check_paid_amount};
    use wiremock::matchers::{body_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const MERCHANT: &str = "29ef6fa6-0d2b-466c-9604-0363a30436cc";
    const TX: &str = "3fa85f64-5717-4562-b3fc-2c963f66afa6";

    fn settings() -> ProviderConfig {
        config(&[
            ("merchant_id", json!(MERCHANT)),
            ("secret", json!("platega-secret")),
        ])
    }

    fn headers() -> Vec<(&'static str, &'static str)> {
        vec![("X-MerchantId", MERCHANT), ("X-Secret", "platega-secret")]
    }

    fn callback(status: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": TX, "amount": 499.0, "currency": "RUB", "status": status,
            "paymentMethod": 2, "payload": PAYMENT.to_string()
        }))
        .unwrap()
    }

    async fn api(status: &str, amount: serde_json::Value, currency: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/transaction/{TX}")))
            .and(header("X-MerchantId", MERCHANT))
            .and(header("X-Secret", "platega-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": TX, "status": status,
                "paymentDetails": {"amount": amount, "currency": currency},
                "paymentMethod": "SBPQR", "payload": PAYMENT.to_string()
            })))
            .mount(&server)
            .await;
        server
    }

    async fn parse(
        server: &MockServer,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> PaymentResult<WebhookOutcome> {
        Platega::with_api_base(server.uri())
            .parse_webhook(&settings(), &hook(headers, body))
            .await
    }

    #[tokio::test]
    async fn заголовки_platega_и_повторный_запрос_статуса() {
        let server = api("CONFIRMED", json!(499), "RUB").await;
        let outcome = parse(&server, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(rub(49_900)));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.provider_txid.as_deref(), Some(TX));
        let wrong = parse(
            &server,
            &[("X-MerchantId", MERCHANT), ("X-Secret", "guess")],
            callback("CONFIRMED"),
        )
        .await;
        assert_eq!(wrong, Err(PaymentError::BadSignature));
        let foreign = parse(
            &server,
            &[
                ("X-MerchantId", "11111111-1111-4111-8111-111111111111"),
                ("X-Secret", "platega-secret"),
            ],
            callback("CONFIRMED"),
        )
        .await;
        assert_eq!(foreign, Err(PaymentError::BadSignature));
        assert_eq!(
            parse(&server, &[], callback("CONFIRMED")).await,
            Err(PaymentError::BadSignature)
        );
    }

    #[tokio::test]
    async fn статус_берётся_из_api_а_не_из_тела() {
        let server = api("PENDING", json!(499), "RUB").await;
        let outcome = parse(&server, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        let unknown = MockServer::start().await;
        assert_eq!(
            parse(&unknown, &headers(), callback("CONFIRMED")).await,
            Err(PaymentError::BadSignature)
        );
    }

    #[tokio::test]
    async fn отмена_и_подтверждение_той_же_транзакции() {
        let canceled = api("CANCELED", json!(499), "RUB").await;
        let first = parse(&canceled, &headers(), callback("CANCELED"))
            .await
            .unwrap();
        assert_eq!(first.status, ProviderStatus::Canceled);
        let confirmed = api("CONFIRMED", json!(499), "RUB").await;
        let second = parse(&confirmed, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert_eq!(second.status, ProviderStatus::Paid);
        assert_eq!(first.provider_txid, second.provider_txid);
        assert_ne!(first.event_key, second.event_key);
        let mut payment = PaymentStatus::Pending;
        for outcome in [&first, &second] {
            let target = match outcome.status {
                ProviderStatus::Canceled => PaymentStatus::Canceled,
                _ => PaymentStatus::Paid,
            };
            assert_eq!(payment.transition(target), Ok(true));
            payment = target;
        }
        assert!(
            PaymentStatus::Refunded
                .transition(PaymentStatus::Paid)
                .is_err()
        );
    }

    #[tokio::test]
    async fn статусы_platega() {
        for (status, expected) in [
            ("CONFIRMED", ProviderStatus::Paid),
            ("CANCELED", ProviderStatus::Canceled),
            ("CHARGEBACKED", ProviderStatus::Refunded),
            ("PENDING", ProviderStatus::Pending),
        ] {
            let server = api(status, json!(499), "RUB").await;
            let outcome = parse(&server, &headers(), callback(status)).await.unwrap();
            assert_eq!(outcome.status, expected, "{status}");
            assert_eq!(outcome.event_key, format!("{TX}:{status}"));
        }
        let server = MockServer::start().await;
        let subscription = parse(&server, &headers(), callback("SUBSCRIPTION_ACTIVATED"))
            .await
            .unwrap();
        assert_eq!(subscription.payment, None);
    }

    #[tokio::test]
    async fn частичный_возврат_по_колбэку() {
        let server = MockServer::start().await;
        let body = serde_json::to_vec(&json!({
            "id": TX, "amount": 499.0, "currency": "RUB", "status": "PARTIALLY_REFUNDED",
            "payload": PAYMENT.to_string(), "refundId": "rf-1", "refundAmount": 100,
            "refundedAmount": 100, "refundableAmount": 399, "refundStatus": "Succeeded"
        }))
        .unwrap();
        let outcome = parse(&server, &headers(), body).await.unwrap();
        assert_eq!(
            outcome.status,
            ProviderStatus::PartiallyRefunded {
                refunded: rub(10_000)
            }
        );
        assert_eq!(outcome.event_key, format!("{TX}:refund:rf-1"));
        let declined = serde_json::to_vec(&json!({
            "id": TX, "amount": 499.0, "currency": "RUB", "status": "PARTIALLY_REFUNDED",
            "payload": PAYMENT.to_string(), "refundId": "rf-2", "refundAmount": 100,
            "refundStatus": "Declined"
        }))
        .unwrap();
        let declined = parse(&server, &headers(), declined).await.unwrap();
        assert_eq!(declined.status, ProviderStatus::RefundDeclined);
        assert_eq!(declined.event_key, format!("{TX}:refund:rf-2"));
        assert_eq!(declined.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(declined.amount, Some(rub(10_000)));
        let chargeback = serde_json::to_vec(&json!({
            "id": TX, "amount": 499.0, "currency": "RUB", "status": "CHARGEBACKED",
            "payload": PAYMENT.to_string(), "refundId": "rf-4", "refundAmount": 499,
            "refundStatus": "Succeeded"
        }))
        .unwrap();
        let chargeback = parse(&server, &headers(), chargeback).await.unwrap();
        assert_eq!(chargeback.status, ProviderStatus::Refunded);
        assert_eq!(chargeback.amount, Some(rub(49_900)));
    }

    #[tokio::test]
    async fn незавершённый_возврат_по_колбэку_не_засчитывается() {
        let server = MockServer::start().await;
        for state in ["Pending", "InProgress", "Created"] {
            let body = serde_json::to_vec(&json!({
                "id": TX, "amount": 499.0, "currency": "RUB", "status": "CONFIRMED",
                "payload": PAYMENT.to_string(), "refundId": "rf-3", "refundAmount": 100,
                "refundStatus": state
            }))
            .unwrap();
            let outcome = parse(&server, &headers(), body).await.unwrap();
            assert_eq!(outcome.status, ProviderStatus::Pending, "{state}");
            assert_eq!(outcome.event_key, format!("{TX}:refund:rf-3"));
        }
    }

    #[tokio::test]
    async fn недоплата_и_чужая_валюта_platega() {
        let expected = rub(49_900);
        let under = api("CONFIRMED", json!(498.99), "RUB").await;
        let outcome = parse(&under, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, outcome.amount),
            AmountCheck::Underpaid { .. }
        ));
        let usd_tx = api("CONFIRMED", json!(499), "USD").await;
        let outcome = parse(&usd_tx, &headers(), callback("CONFIRMED"))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, outcome.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
        let fractional = api("CONFIRMED", json!(498.999), "RUB").await;
        assert!(matches!(
            parse(&fractional, &headers(), callback("CONFIRMED")).await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn счёт_platega() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/transaction/process"))
            .and(header("X-MerchantId", MERCHANT))
            .and(header("X-Secret", "platega-secret"))
            .and(body_json(json!({
                "paymentMethod": 2,
                "paymentDetails": {"amount": 499.5, "currency": "RUB"},
                "description": "Стандарт · 3 месяца",
                "return": "https://panel.example.com/cabinet/payment-wait",
                "failedUrl": "https://panel.example.com/cabinet/payment-wait",
                "payload": PAYMENT.to_string(),
                "metadata": {"userId": CLIENT.to_string(), "user_email": "client@example.com"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "paymentMethod": "SBPQR", "transactionId": TX,
                "redirect": "https://pay.platega.io/?id=3fa85f64",
                "status": "PENDING", "expiresIn": "00:15:00"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let created = Platega::with_api_base(server.uri())
            .create_invoice(
                &settings(),
                &invoice(rub(49_950), Some("client@example.com")),
            )
            .await
            .unwrap();
        assert_eq!(created.external_id.as_deref(), Some(TX));
        assert_eq!(created.pay_url, "https://pay.platega.io/?id=3fa85f64");
        let sent = server.received_requests().await.unwrap();
        assert!(String::from_utf8_lossy(&sent[0].body).contains("\"amount\":499.50"));
    }

    #[tokio::test]
    async fn возврат_platega_полный_и_частичный() {
        let server = api("CONFIRMED", json!(499), "RUB").await;
        Mock::given(method("POST"))
            .and(path(format!("/transaction/{TX}/cancel")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "transactionId": TX, "accepted": true, "manualControlRequired": false,
                "message": null, "refundOperationId": "op-full"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/transaction/{TX}/cancel-partial")))
            .and(header_exists("Idempotency-Key"))
            .and(body_json(json!({"amount": 100.0})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "transactionId": TX, "accepted": true, "manualControlRequired": false,
                "message": null, "refundOperationId": "op-part"
            })))
            .mount(&server)
            .await;
        let platega = Platega::with_api_base(server.uri());
        let refund = |amount| RefundRequest {
            payment_id: PaymentId(41),
            provider_txid: TX.into(),
            amount,
            reason: None,
        };
        let full = platega
            .refund(&settings(), &refund(rub(49_900)))
            .await
            .unwrap();
        assert_eq!(full.provider_refund_id.as_deref(), Some("op-full"));
        let part = platega
            .refund(&settings(), &refund(rub(10_000)))
            .await
            .unwrap();
        assert_eq!(part.provider_refund_id.as_deref(), Some("op-part"));
        assert!(!part.completed);
    }

    #[test]
    fn настройки_platega() {
        let platega = Platega::new();
        assert!(platega.is_configured(&settings()));
        let mut bad = settings();
        bad.set("method", json!("99"));
        assert!(!platega.is_configured(&bad));
        assert!(!platega.is_configured(&config(&[
            ("merchant_id", json!("m-1")),
            ("secret", json!("s"))
        ])));
    }
}
