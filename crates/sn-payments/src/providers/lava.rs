use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use uuid::Uuid;

const API: &str = "https://gate.lava.top";
const KEY_HEADER: &str = "x-api-key";

pub struct Lava {
    http: reqwest::Client,
    api_base: String,
}

impl Default for Lava {
    fn default() -> Self {
        Self::new()
    }
}

impl Lava {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Lava {
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
            .header("X-Api-Key", config.require("api_key")?)
            .header(reqwest::header::ACCEPT, "application/json"))
    }
}

fn message(body: &Json) -> Option<&str> {
    body.get("error")
        .or_else(|| body.get("message"))
        .and_then(Json::as_str)
}

fn invoice_amount(info: &Json) -> Option<PaymentResult<Amount>> {
    let total = info.get("amountTotal").or_else(|| info.get("receipt"))?;
    Some(util::json_amount(
        total.get("amount"),
        total.get("currency"),
    ))
}

#[async_trait]
impl PaymentProvider for Lava {
    fn id(&self) -> &'static str {
        providers::LAVA
    }

    fn title(&self) -> &'static str {
        "Lava"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Usd]
    }

    fn client_label(&self, _currency: Currency) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: "Visa и Mastercard любых стран",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::secret("api_key", "API-ключ", "Lava.top → Интеграция → Public API"),
            SettingField::text(
                "offer_id",
                "ID оффера",
                "Оффер продукта с признаком «Цена по запросу API»",
            ),
            SettingField::secret(
                "webhook_key",
                "Ключ вебхука",
                "API-ключ, который Lava.top присылает в X-Api-Key; пусто — тот же API-ключ",
            )
            .optional(),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        match config.get("offer_id").map(Uuid::parse_str) {
            Some(Ok(_)) => Ok(()),
            _ => Err(PaymentError::NotConfigured(
                "ID оффера Lava.top — это UUID из кабинета".into(),
            )),
        }
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Usd)?;
        let offer = config.require("offer_id")?;
        let email = util::client_email(request).ok_or(PaymentError::EmailRequired)?;
        let mut body = Json::object([
            ("email", Json::from(email)),
            ("offerId", Json::from(offer)),
            ("currency", Json::from("USD")),
            ("amount", Json::number(request.amount.units())),
        ]);
        if let Some(url) = util::return_url(request).filter(|u| util::is_https(u) && u.len() <= 512)
        {
            body.push("successful_return_url", Json::from(url.clone()));
            body.push("failure_return_url", Json::from(url.clone()));
            body.push("cancel_return_url", Json::from(url));
        }
        let reply = util::send(
            self.request(config, reqwest::Method::POST, "/api/v3/invoice")?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(message(&parsed)));
        }
        let id = parsed
            .get("id")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| util::bad("Lava.top не вернула id счёта"))?;
        let pay_url = parsed
            .get("paymentUrl")
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("Lava.top не вернула ссылку на оплату"))?;
        if let Some(Ok(charged)) = invoice_amount(&parsed)
            && charged != request.amount
        {
            return Err(PaymentError::Rejected(format!(
                "Lava.top выставила счёт на {charged}, а не на {}: включите у продукта «Цена по запросу API»",
                request.amount
            )));
        }
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "id": id,
                "status": parsed.get("status").and_then(Json::as_str),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let expected = config
            .get("webhook_key")
            .or_else(|| config.get("api_key"))
            .ok_or_else(|| PaymentError::NotConfigured("не задан ключ вебхука".into()))?;
        let received = request
            .header(KEY_HEADER)
            .ok_or(PaymentError::BadSignature)?;
        if !util::secret_eq(expected, received.trim()) {
            return Err(PaymentError::BadSignature);
        }
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        if let Some(event) = body.get("event_type").and_then(Json::as_str) {
            let id = body
                .get("event_id")
                .and_then(Json::as_str)
                .unwrap_or_default();
            return Ok(util::skipped(
                format!("{event}:{id}"),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        let event = body
            .get("eventType")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет eventType"))?;
        let contract = body
            .get("contractId")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| util::bad("нет contractId"))?;
        let event_key = format!("{event}:{contract}");
        let status = match event {
            "payment.success" => ProviderStatus::Paid,
            "payment.failed" => ProviderStatus::Failed,
            _ => {
                return Ok(util::skipped(event_key, None, None, WebhookReply::ok()));
            }
        };
        let amount = match status {
            ProviderStatus::Paid => {
                Some(util::json_amount(body.get("amount"), body.get("currency"))?)
            }
            _ => None,
        };
        Ok(WebhookOutcome {
            event_key,
            payment: Some(PaymentRef::ProviderTxid(contract.to_string())),
            provider_txid: Some(contract.to_string()),
            status,
            amount,
            saved_method: None,
            reply: WebhookReply::ok(),
        })
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let id = util::safe_id(external_id)?;
        let reply = util::send(self.request(
            config,
            reqwest::Method::GET,
            &format!("/api/v1/invoices/{id}"),
        )?)
        .await?;
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(message(&parsed)));
        }
        let status_text = parsed
            .get("status")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет статуса счёта"))?;
        let status = match status_text {
            "COMPLETED" => ProviderStatus::Paid,
            "FAILED" => ProviderStatus::Failed,
            _ => ProviderStatus::Pending,
        };
        let amount = match status {
            ProviderStatus::Paid => {
                Some(invoice_amount(&parsed).ok_or_else(|| util::bad("нет суммы счёта"))??)
            }
            _ => None,
        };
        Ok(Some(WebhookOutcome {
            event_key: format!("invoice.{status_text}:{id}"),
            payment: Some(PaymentRef::ProviderTxid(id.to_string())),
            provider_txid: Some(id.to_string()),
            status,
            amount,
            saved_method: None,
            reply: WebhookReply::ok(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;
    use sn_domain::payment::{AmountCheck, check_paid_amount};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const OFFER: &str = "836b9fc5-7ae9-4a27-9642-592bc44072b7";
    const CONTRACT: &str = "7ea82675-4ded-4133-95a7-a6efbaf165cc";

    fn settings() -> ProviderConfig {
        config(&[
            ("api_key", json!("lava-api-key")),
            ("offer_id", json!(OFFER)),
            ("webhook_key", json!("lava-webhook-key")),
        ])
    }

    fn event(kind: &str, amount: serde_json::Value, currency: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "eventType": kind,
            "product": {"id": "p-1", "title": "STEALTHNET"},
            "buyer": {"email": "client@example.com"},
            "contractId": CONTRACT,
            "amount": amount,
            "currency": currency,
            "timestamp": "2026-11-16T14:52:00Z",
            "status": "completed"
        }))
        .unwrap()
    }

    async fn parse(key: Option<&str>, body: Vec<u8>) -> PaymentResult<WebhookOutcome> {
        let headers: Vec<(&str, &str)> = key.map(|k| vec![("X-Api-Key", k)]).unwrap_or_default();
        Lava::new()
            .parse_webhook(&settings(), &hook(&headers, body))
            .await
    }

    #[tokio::test]
    async fn ключ_вебхука_сверяется() {
        let body = event("payment.success", json!(6.99), "USD");
        let outcome = parse(Some("lava-webhook-key"), body.clone()).await.unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(usd(699)));
        assert_eq!(
            outcome.payment,
            Some(PaymentRef::ProviderTxid(CONTRACT.into()))
        );
        assert_eq!(outcome.event_key, format!("payment.success:{CONTRACT}"));
        assert_eq!(
            parse(Some("lava-api-key"), body.clone()).await,
            Err(PaymentError::BadSignature)
        );
        assert_eq!(parse(None, body).await, Err(PaymentError::BadSignature));
    }

    #[tokio::test]
    async fn без_отдельного_ключа_вебхука_сверяется_api_ключ() {
        let settings = config(&[
            ("api_key", json!("lava-api-key")),
            ("offer_id", json!(OFFER)),
        ]);
        let outcome = Lava::new()
            .parse_webhook(
                &settings,
                &hook(
                    &[("x-api-key", "lava-api-key")],
                    event("payment.failed", json!(6.99), "USD"),
                ),
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Failed);
    }

    #[tokio::test]
    async fn события_lava() {
        for kind in [
            "subscription.recurring.payment.success",
            "subscription.cancelled",
        ] {
            let outcome = parse(Some("lava-webhook-key"), event(kind, json!(6.99), "USD"))
                .await
                .unwrap();
            assert_eq!(outcome.payment, None, "{kind}");
            assert_eq!(outcome.status, ProviderStatus::Pending);
        }
        let refund = serde_json::to_vec(&json!({
            "event_id": "e-1",
            "event_type": "refund.success",
            "created_at": "2026-11-16T14:52:00Z",
            "data": {"refund_id": "r-1", "refund_type": "full", "amount": 6.99, "currency": "USD"}
        }))
        .unwrap();
        let outcome = parse(Some("lava-webhook-key"), refund).await.unwrap();
        assert_eq!(outcome.payment, None);
        assert_eq!(outcome.event_key, "refund.success:e-1");
    }

    #[tokio::test]
    async fn недоплата_и_чужая_валюта_lava() {
        let expected = usd(699);
        let under = parse(
            Some("lava-webhook-key"),
            event("payment.success", json!(6.98), "USD"),
        )
        .await
        .unwrap();
        assert!(matches!(
            check_paid_amount(expected, under.amount),
            AmountCheck::Underpaid { .. }
        ));
        let rub = parse(
            Some("lava-webhook-key"),
            event("payment.success", json!(6.99), "RUB"),
        )
        .await
        .unwrap();
        assert!(matches!(
            check_paid_amount(expected, rub.amount),
            AmountCheck::CurrencyMismatch { .. }
        ));
        for (amount, currency) in [
            (json!(6.999), "USD"),
            (json!("abc"), "USD"),
            (json!(6.99), "EUR"),
        ] {
            assert!(matches!(
                parse(
                    Some("lava-webhook-key"),
                    event("payment.success", amount, currency)
                )
                .await,
                Err(PaymentError::BadPayload(_))
            ));
        }
    }

    #[tokio::test]
    async fn счёт_lava_на_сумму_тарифа() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/invoice"))
            .and(header("X-Api-Key", "lava-api-key"))
            .and(body_json(json!({
                "email": "client@example.com",
                "offerId": OFFER,
                "currency": "USD",
                "amount": 6.99,
                "successful_return_url": "https://panel.example.com/cabinet/payment-wait",
                "failure_return_url": "https://panel.example.com/cabinet/payment-wait",
                "cancel_return_url": "https://panel.example.com/cabinet/payment-wait"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": CONTRACT,
                "status": "in-progress",
                "amountTotal": {"currency": "USD", "amount": 6.99},
                "paymentUrl": "https://app.lava.top/payment/7ea82675"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let lava = Lava::with_api_base(server.uri());
        let created = lava
            .create_invoice(&settings(), &invoice(usd(699), Some("client@example.com")))
            .await
            .unwrap();
        assert_eq!(created.external_id.as_deref(), Some(CONTRACT));
        assert_eq!(created.pay_url, "https://app.lava.top/payment/7ea82675");
        let sent = server.received_requests().await.unwrap();
        assert!(
            String::from_utf8_lossy(&sent[0].body).contains("\"amount\":6.99"),
            "сумма уходит числом без плавающей точки"
        );
        assert_eq!(
            lava.create_invoice(&settings(), &invoice(usd(699), None))
                .await,
            Err(PaymentError::EmailRequired)
        );
        assert!(matches!(
            lava.create_invoice(&settings(), &invoice(rub(49_900), Some("c@example.com")))
                .await,
            Err(PaymentError::UnsupportedCurrency(_))
        ));
    }

    #[tokio::test]
    async fn цена_оффера_не_совпала_с_тарифом() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/invoice"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": CONTRACT,
                "amountTotal": {"currency": "USD", "amount": 10.0},
                "paymentUrl": "https://app.lava.top/payment/7ea82675"
            })))
            .mount(&server)
            .await;
        let result = Lava::with_api_base(server.uri())
            .create_invoice(&settings(), &invoice(usd(699), Some("client@example.com")))
            .await;
        assert!(matches!(result, Err(PaymentError::Rejected(_))));
    }

    #[tokio::test]
    async fn статус_счёта_lava() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/invoices/{CONTRACT}")))
            .and(header("X-Api-Key", "lava-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": CONTRACT,
                "type": "ONE_TIME",
                "status": "COMPLETED",
                "receipt": {"amount": 6.99, "currency": "USD", "fee": 0.35}
            })))
            .mount(&server)
            .await;
        let outcome = Lava::with_api_base(server.uri())
            .fetch_status(&settings(), CONTRACT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(usd(699)));
    }

    #[test]
    fn настройки_lava() {
        let lava = Lava::new();
        assert!(lava.is_configured(&settings()));
        assert!(!lava.is_configured(&config(&[("api_key", json!("k"))])));
        assert!(!lava.is_configured(&config(&[
            ("api_key", json!("k")),
            ("offer_id", json!("offer-1"))
        ])));
    }
}
