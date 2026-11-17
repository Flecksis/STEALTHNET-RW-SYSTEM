use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use std::ops::RangeInclusive;

const API: &str = "https://api.yookassa.ru";
const TAX_SYSTEMS: RangeInclusive<i64> = 1..=6;
const VAT_CODES: RangeInclusive<i64> = 1..=12;

pub struct YooKassa {
    http: reqwest::Client,
    api_base: String,
}

impl Default for YooKassa {
    fn default() -> Self {
        Self::new()
    }
}

impl YooKassa {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        YooKassa {
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
        let shop = config.require("shop_id")?;
        let secret = config.require("secret_key")?;
        Ok(self
            .http
            .request(method, format!("{}{path}", self.api_base))
            .basic_auth(shop, Some(secret)))
    }

    async fn post(
        &self,
        config: &ProviderConfig,
        path: &str,
        body: &Json,
        idempotence_key: &str,
    ) -> PaymentResult<Json> {
        let reply = util::send(
            self.request(config, reqwest::Method::POST, path)?
                .header("Idempotence-Key", idempotence_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        let parsed = reply.json()?;
        if reply.is_success() {
            Ok(parsed)
        } else {
            Err(reply.failure(parsed.get("description").and_then(Json::as_str)))
        }
    }

    async fn find(
        &self,
        config: &ProviderConfig,
        collection: &str,
        id: &str,
    ) -> PaymentResult<Option<Json>> {
        let id = util::safe_id(id)?;
        let reply = util::send(self.request(
            config,
            reqwest::Method::GET,
            &format!("/v3/{collection}/{id}"),
        )?)
        .await?;
        if reply.status == 404 {
            return Ok(None);
        }
        let parsed = reply.json()?;
        if !reply.is_success() {
            return Err(reply.failure(parsed.get("description").and_then(Json::as_str)));
        }
        if parsed.get("id").and_then(Json::as_str) != Some(id) {
            return Err(util::bad("ЮKassa вернула другой объект"));
        }
        Ok(Some(parsed))
    }

    async fn refund_outcome(
        &self,
        config: &ProviderConfig,
        refund_id: &str,
        refund: &Json,
    ) -> PaymentResult<WebhookOutcome> {
        let event_key = format!("refund:{refund_id}");
        let declined = match refund.get("status").and_then(Json::as_str) {
            Some("succeeded") => false,
            Some("canceled") => true,
            _ => return Ok(util::skipped(event_key, None, None, WebhookReply::ok())),
        };
        let payment_id = refund
            .get("payment_id")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("в возврате нет payment_id"))?;
        let refunded = util::json_amount(
            refund.at(&["amount", "value"]),
            refund.at(&["amount", "currency"]),
        )?;
        if declined {
            return Ok(WebhookOutcome {
                event_key,
                payment: Some(PaymentRef::ProviderTxid(payment_id.to_string())),
                provider_txid: Some(payment_id.to_string()),
                status: ProviderStatus::RefundDeclined,
                amount: Some(refunded),
                saved_method: None,
                reply: WebhookReply::ok(),
            });
        }
        let payment = self
            .find(config, "payments", payment_id)
            .await?
            .ok_or(PaymentError::BadSignature)?;
        let paid = util::json_amount(
            payment.at(&["amount", "value"]),
            payment.at(&["amount", "currency"]),
        )?;
        let total = match payment.at(&["refunded_amount", "value"]) {
            Some(_) => util::json_amount(
                payment.at(&["refunded_amount", "value"]),
                payment.at(&["refunded_amount", "currency"]),
            )?,
            None => refunded,
        };
        let status = if total.currency == paid.currency && total.minor >= paid.minor {
            ProviderStatus::Refunded
        } else {
            ProviderStatus::PartiallyRefunded { refunded }
        };
        Ok(WebhookOutcome {
            event_key,
            payment: util::public_ref(
                payment
                    .at(&["metadata", "payment_id"])
                    .and_then(Json::as_str),
            )
            .or_else(|| Some(PaymentRef::ProviderTxid(payment_id.to_string()))),
            provider_txid: Some(payment_id.to_string()),
            status,
            amount: Some(refunded),
            saved_method: None,
            reply: WebhookReply::ok(),
        })
    }
}

fn money(amount: Amount) -> Json {
    Json::object([
        ("value", Json::from(amount.units())),
        ("currency", Json::from(amount.currency.code())),
    ])
}

fn code(
    config: &ProviderConfig,
    key: &str,
    range: RangeInclusive<i64>,
) -> PaymentResult<Option<i64>> {
    let present = match config.0.get(key) {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    };
    if !present {
        return Ok(None);
    }
    match config.integer(key) {
        Some(value) if range.contains(&value) => Ok(Some(value)),
        _ => Err(PaymentError::NotConfigured(format!(
            "{key}: допустимы значения от {} до {}",
            range.start(),
            range.end()
        ))),
    }
}

fn receipt(config: &ProviderConfig, request: &InvoiceRequest) -> PaymentResult<Option<Json>> {
    let Some(vat) = code(config, "vat_code", VAT_CODES)? else {
        return Ok(None);
    };
    let email = util::client_email(request).ok_or(PaymentError::EmailRequired)?;
    let description = util::clip(request.description.trim(), 128);
    let mut receipt = Json::object([
        ("customer", Json::object([("email", Json::from(email))])),
        (
            "items",
            Json::Array(vec![Json::object([
                (
                    "description",
                    Json::from(if description.is_empty() {
                        "Оплата подписки".to_string()
                    } else {
                        description
                    }),
                ),
                ("quantity", Json::number("1")),
                ("amount", money(request.amount)),
                ("vat_code", Json::from(vat)),
                ("payment_subject", Json::from("service")),
                ("payment_mode", Json::from("full_payment")),
            ])]),
        ),
    ]);
    if let Some(tax) = code(config, "tax_system_code", TAX_SYSTEMS)? {
        receipt.push("tax_system_code", Json::from(tax));
    }
    Ok(Some(receipt))
}

fn saved_method(payment: &Json) -> Option<SavedMethod> {
    let method = payment.get("payment_method")?;
    if method.get("saved").and_then(Json::as_bool) != Some(true) {
        return None;
    }
    let token = method.get("id").and_then(Json::as_str)?.to_string();
    let title = method
        .get("title")
        .and_then(Json::as_str)
        .map(str::to_string)
        .or_else(|| {
            method
                .at(&["card", "last4"])
                .and_then(Json::as_str)
                .map(|last4| format!("Карта *{last4}"))
        });
    Some(SavedMethod { token, title })
}

fn payment_outcome(payment: &Json) -> PaymentResult<WebhookOutcome> {
    let id = payment
        .get("id")
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет id платежа ЮKassa"))?;
    let status_text = payment
        .get("status")
        .and_then(Json::as_str)
        .ok_or_else(|| util::bad("нет статуса платежа ЮKassa"))?;
    let status = match status_text {
        "succeeded" => ProviderStatus::Paid,
        "canceled" => ProviderStatus::Canceled,
        _ => ProviderStatus::Pending,
    };
    let amount = util::json_amount(
        payment.at(&["amount", "value"]),
        payment.at(&["amount", "currency"]),
    )?;
    Ok(WebhookOutcome {
        event_key: format!("payment.{status_text}:{id}"),
        payment: util::public_ref(
            payment
                .at(&["metadata", "payment_id"])
                .and_then(Json::as_str),
        )
        .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string()))),
        provider_txid: Some(id.to_string()),
        status,
        amount: Some(amount),
        saved_method: match status {
            ProviderStatus::Paid => saved_method(payment),
            _ => None,
        },
        reply: WebhookReply::ok(),
    })
}

#[async_trait]
impl PaymentProvider for YooKassa {
    fn id(&self) -> &'static str {
        providers::YOOKASSA
    }

    fn title(&self) -> &'static str {
        "ЮKassa"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency) -> ClientLabel {
        ClientLabel {
            title: "Банковская карта",
            hint: "Карты российских банков и СБП",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                "shop_id",
                "shopId",
                "Кабинет ЮKassa → Интеграция → Ключи API",
            ),
            SettingField::secret(
                "secret_key",
                "Секретный ключ",
                "Кабинет ЮKassa → Интеграция → Ключи API",
            ),
            SettingField::number(
                "tax_system_code",
                "Система налогообложения",
                "Код для чеков 54-ФЗ, если чеки отправляет ЮKassa",
            ),
            SettingField::number(
                "vat_code",
                "Ставка НДС",
                "Код ставки для чеков 54-ФЗ; пусто — без чека",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if !config
            .get("shop_id")
            .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(PaymentError::NotConfigured(
                "shopId ЮKassa состоит из цифр".into(),
            ));
        }
        let tax = code(config, "tax_system_code", TAX_SYSTEMS)?;
        let vat = code(config, "vat_code", VAT_CODES)?;
        if tax.is_some() && vat.is_none() {
            return Err(PaymentError::NotConfigured(
                "для чеков укажите и ставку НДС".into(),
            ));
        }
        Ok(())
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn supports_recurring(&self) -> bool {
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
        let return_url = util::return_url(request)
            .ok_or_else(|| PaymentError::NotConfigured("не задан адрес панели".into()))?;
        let mut body = Json::object([
            ("amount", money(request.amount)),
            ("capture", Json::from(true)),
            (
                "confirmation",
                Json::object([
                    ("type", Json::from("redirect")),
                    ("return_url", Json::from(return_url)),
                ]),
            ),
            (
                "description",
                Json::from(util::clip(&request.description, 128)),
            ),
            (
                "metadata",
                Json::object([(
                    "payment_id",
                    Json::from(request.payment_public_id.to_string()),
                )]),
            ),
        ]);
        if let Some(receipt) = receipt(config, request)? {
            body.push("receipt", receipt);
        }
        if request.save_payment_method {
            body.push("save_payment_method", Json::from(true));
        }
        let payment = self
            .post(
                config,
                "/v3/payments",
                &body,
                &request.payment_public_id.to_string(),
            )
            .await?;
        let id = payment
            .get("id")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("ЮKassa не вернула id платежа"))?;
        let pay_url = payment
            .at(&["confirmation", "confirmation_url"])
            .and_then(Json::as_str)
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("ЮKassa не вернула ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "id": id,
                "status": payment.get("status").and_then(Json::as_str),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        let event = body
            .get("event")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет event"))?;
        let object_id = body
            .at(&["object", "id"])
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("нет object.id"))?;
        if event.starts_with("payment.") {
            let payment = self
                .find(config, "payments", object_id)
                .await?
                .ok_or(PaymentError::BadSignature)?;
            return payment_outcome(&payment);
        }
        if !event.starts_with("refund.") {
            return Ok(util::skipped(
                format!("{event}:{object_id}"),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        let refund = self
            .find(config, "refunds", object_id)
            .await?
            .ok_or(PaymentError::BadSignature)?;
        self.refund_outcome(config, object_id, &refund).await
    }

    async fn fetch_refund(
        &self,
        config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let refund = self
            .find(config, "refunds", provider_refund_id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("возврат не найден в ЮKassa".into()))?;
        self.refund_outcome(config, provider_refund_id, &refund)
            .await
            .map(Some)
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        match self.find(config, "payments", external_id).await? {
            Some(payment) => payment_outcome(&payment).map(Some),
            None => Err(PaymentError::Rejected("платёж не найден в ЮKassa".into())),
        }
    }

    async fn charge_saved(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
        token: &str,
    ) -> PaymentResult<WebhookOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let mut body = Json::object([
            ("amount", money(request.amount)),
            ("capture", Json::from(true)),
            ("payment_method_id", Json::from(token)),
            (
                "description",
                Json::from(util::clip(&request.description, 128)),
            ),
            (
                "metadata",
                Json::object([(
                    "payment_id",
                    Json::from(request.payment_public_id.to_string()),
                )]),
            ),
        ]);
        if let Some(receipt) = receipt(config, request)? {
            body.push("receipt", receipt);
        }
        let payment = self
            .post(
                config,
                "/v3/payments",
                &body,
                &request.payment_public_id.to_string(),
            )
            .await?;
        payment_outcome(&payment)
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let payment_id = util::safe_id(&request.provider_txid)?;
        let mut body = Json::object([
            ("payment_id", Json::from(payment_id)),
            ("amount", money(request.amount)),
        ]);
        if let Some(reason) = request
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            body.push("description", Json::from(util::clip(reason, 250)));
        }
        let key = util::idempotency_key(&[
            "refund",
            &request.payment_id.to_string(),
            payment_id,
            &request.amount.minor.to_string(),
            request.reason.as_deref().unwrap_or_default(),
        ]);
        let refund = self.post(config, "/v3/refunds", &body, &key).await?;
        let id = refund
            .get("id")
            .and_then(Json::as_str)
            .ok_or_else(|| util::bad("ЮKassa не вернула id возврата"))?;
        match refund.get("status").and_then(Json::as_str) {
            Some("succeeded") => Ok(RefundOutcome {
                provider_refund_id: Some(id.to_string()),
                completed: true,
            }),
            Some("canceled") => Err(PaymentError::Rejected(format!(
                "ЮKassa отклонила возврат: {}",
                refund
                    .at(&["cancellation_details", "reason"])
                    .and_then(Json::as_str)
                    .unwrap_or("без пояснения")
            ))),
            _ => Ok(RefundOutcome {
                provider_refund_id: Some(id.to_string()),
                completed: false,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;
    use sn_domain::ids::PaymentId;
    use sn_domain::payment::{AmountCheck, check_paid_amount};
    use wiremock::matchers::{basic_auth, body_partial_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID: &str = "2a8f5d1e-000f-5000-9000-1b4f0c5e7d21";

    fn settings(extra: &[(&str, serde_json::Value)]) -> ProviderConfig {
        let mut pairs = vec![
            ("shop_id", json!("504367")),
            ("secret_key", json!("live_secret")),
        ];
        pairs.extend_from_slice(extra);
        config(&pairs)
    }

    fn payment(status: &str, value: &str, currency: &str) -> serde_json::Value {
        json!({
            "id": ID,
            "status": status,
            "paid": status == "succeeded",
            "amount": {"value": value, "currency": currency},
            "metadata": {"payment_id": PAYMENT.to_string()},
            "payment_method": {
                "type": "bank_card", "id": ID, "saved": true, "title": "Bank card *4477",
                "card": {"last4": "4477"}
            }
        })
    }

    fn notification(event: &str, id: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "type": "notification",
            "event": event,
            "object": {"id": id, "status": "succeeded", "amount": {"value": "0.01", "currency": "RUB"}}
        }))
        .unwrap()
    }

    async fn api(payment_body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/v3/payments/{ID}")))
            .and(basic_auth("504367", "live_secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(payment_body))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn уведомление_подтверждается_только_запросом_в_юkassa() {
        let server = api(payment("succeeded", "499.00", "RUB")).await;
        let kassa = YooKassa::with_api_base(server.uri());
        let outcome = kassa
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("payment.succeeded", ID)),
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(rub(49_900)));
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.provider_txid.as_deref(), Some(ID));
        assert_eq!(
            outcome.saved_method,
            Some(SavedMethod {
                token: ID.into(),
                title: Some("Bank card *4477".into())
            })
        );
        let forged = kassa
            .parse_webhook(
                &settings(&[]),
                &hook(
                    &[],
                    notification("payment.succeeded", "2a8f5d1e-0000-5000-9000-000000000000"),
                ),
            )
            .await;
        assert_eq!(forged, Err(PaymentError::BadSignature));
        let traversal = kassa
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("payment.succeeded", "../me")),
            )
            .await;
        assert!(matches!(traversal, Err(PaymentError::BadPayload(_))));
    }

    #[tokio::test]
    async fn поддельный_succeeded_в_теле_не_засчитывается() {
        let server = api(payment("pending", "499.00", "RUB")).await;
        let outcome = YooKassa::with_api_base(server.uri())
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("payment.succeeded", ID)),
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Pending);
        assert_eq!(outcome.saved_method, None);
    }

    #[tokio::test]
    async fn статусы_юkassa() {
        for (status, expected) in [
            ("succeeded", ProviderStatus::Paid),
            ("canceled", ProviderStatus::Canceled),
            ("pending", ProviderStatus::Pending),
            ("waiting_for_capture", ProviderStatus::Pending),
        ] {
            let server = api(payment(status, "499.00", "RUB")).await;
            let outcome = YooKassa::with_api_base(server.uri())
                .parse_webhook(
                    &settings(&[]),
                    &hook(&[], notification("payment.succeeded", ID)),
                )
                .await
                .unwrap();
            assert_eq!(outcome.status, expected, "{status}");
            assert_eq!(outcome.event_key, format!("payment.{status}:{ID}"));
        }
        let other = YooKassa::new()
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("payout.succeeded", "po-1")),
            )
            .await
            .unwrap();
        assert_eq!(other.payment, None);
    }

    #[tokio::test]
    async fn недоплата_и_чужая_валюта_юkassa() {
        let expected = rub(49_900);
        for (value, currency, check) in [("498.99", "RUB", "under"), ("499.00", "USD", "currency")]
        {
            let server = api(payment("succeeded", value, currency)).await;
            let outcome = YooKassa::with_api_base(server.uri())
                .parse_webhook(
                    &settings(&[]),
                    &hook(&[], notification("payment.succeeded", ID)),
                )
                .await
                .unwrap();
            let result = check_paid_amount(expected, outcome.amount);
            match check {
                "under" => assert!(matches!(result, AmountCheck::Underpaid { .. })),
                _ => assert!(matches!(result, AmountCheck::CurrencyMismatch { .. })),
            }
        }
        let server = api(payment("succeeded", "499.001", "RUB")).await;
        assert!(matches!(
            YooKassa::with_api_base(server.uri())
                .parse_webhook(
                    &settings(&[]),
                    &hook(&[], notification("payment.succeeded", ID))
                )
                .await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn возврат_по_уведомлению() {
        let server = api(json!({
            "id": ID, "status": "succeeded",
            "amount": {"value": "499.00", "currency": "RUB"},
            "refunded_amount": {"value": "100.00", "currency": "RUB"},
            "metadata": {"payment_id": PAYMENT.to_string()}
        }))
        .await;
        Mock::given(method("GET"))
            .and(path("/v3/refunds/rf-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "rf-1", "payment_id": ID, "status": "succeeded",
                "amount": {"value": "100.00", "currency": "RUB"}
            })))
            .mount(&server)
            .await;
        let outcome = YooKassa::with_api_base(server.uri())
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("refund.succeeded", "rf-1")),
            )
            .await
            .unwrap();
        assert_eq!(
            outcome.status,
            ProviderStatus::PartiallyRefunded {
                refunded: rub(10_000)
            }
        );
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(outcome.event_key, "refund:rf-1");
    }

    #[tokio::test]
    async fn отменённый_и_незавершённый_возврат_юkassa() {
        let server = MockServer::start().await;
        for (id, status) in [("rf-5", "canceled"), ("rf-6", "pending")] {
            Mock::given(method("GET"))
                .and(path(format!("/v3/refunds/{id}")))
                .and(basic_auth("504367", "live_secret"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "id": id, "payment_id": ID, "status": status,
                    "amount": {"value": "100.00", "currency": "RUB"},
                    "cancellation_details": {"party": "yoo_money", "reason": "insufficient_funds"}
                })))
                .mount(&server)
                .await;
        }
        let kassa = YooKassa::with_api_base(server.uri());
        let declined = kassa
            .parse_webhook(
                &settings(&[]),
                &hook(&[], notification("refund.canceled", "rf-5")),
            )
            .await
            .unwrap();
        assert_eq!(declined.status, ProviderStatus::RefundDeclined);
        assert_eq!(declined.event_key, "refund:rf-5");
        assert_eq!(
            declined.payment,
            Some(PaymentRef::ProviderTxid(ID.to_string()))
        );
        assert_eq!(declined.amount, Some(rub(10_000)));
        let polled = kassa
            .fetch_refund(&settings(&[]), "rf-5")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(polled, declined);
        let waiting = kassa
            .fetch_refund(&settings(&[]), "rf-6")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.status, ProviderStatus::Pending);
        assert_eq!(waiting.payment, None);
        assert!(matches!(
            kassa.fetch_refund(&settings(&[]), "rf-404").await,
            Err(PaymentError::Rejected(_))
        ));
    }

    #[tokio::test]
    async fn счёт_с_ключом_идемпотентности_и_чеком() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .and(basic_auth("504367", "live_secret"))
            .and(header("Idempotence-Key", PAYMENT.to_string().as_str()))
            .and(body_partial_json(json!({
                "amount": {"value": "499.00", "currency": "RUB"},
                "capture": true,
                "confirmation": {"type": "redirect", "return_url": "https://panel.example.com/cabinet/payment-wait"},
                "metadata": {"payment_id": PAYMENT.to_string()},
                "save_payment_method": true,
                "receipt": {
                    "customer": {"email": "client@example.com"},
                    "tax_system_code": 2,
                    "items": [{
                        "quantity": 1, "vat_code": 7, "payment_subject": "service",
                        "payment_mode": "full_payment", "amount": {"value": "499.00", "currency": "RUB"}
                    }]
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": ID, "status": "pending",
                "confirmation": {"type": "redirect", "confirmation_url": "https://yoomoney.ru/checkout/payments/v2/contract?orderId=2a8f"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let kassa = YooKassa::with_api_base(server.uri());
        let receipts = settings(&[("tax_system_code", json!(2)), ("vat_code", json!("7"))]);
        let mut request = invoice(rub(49_900), Some("client@example.com"));
        request.save_payment_method = true;
        let created = kassa.create_invoice(&receipts, &request).await.unwrap();
        assert_eq!(created.external_id.as_deref(), Some(ID));
        assert!(created.pay_url.starts_with("https://yoomoney.ru/checkout/"));
        assert_eq!(
            kassa
                .create_invoice(&receipts, &invoice(rub(49_900), None))
                .await,
            Err(PaymentError::EmailRequired)
        );
        assert!(matches!(
            kassa
                .create_invoice(&receipts, &invoice(usd(699), Some("c@example.com")))
                .await,
            Err(PaymentError::UnsupportedCurrency(_))
        ));
    }

    #[tokio::test]
    async fn без_ставки_ндс_чек_не_отправляется() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .and(header_exists("Idempotence-Key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": ID, "status": "pending",
                "confirmation": {"type": "redirect", "confirmation_url": "https://yoomoney.ru/checkout/x"}
            })))
            .mount(&server)
            .await;
        YooKassa::with_api_base(server.uri())
            .create_invoice(&settings(&[]), &invoice(rub(49_900), None))
            .await
            .unwrap();
        let sent = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert!(body.get("receipt").is_none());
        assert!(body.get("save_payment_method").is_none());
    }

    #[tokio::test]
    async fn ошибка_юkassa_с_описанием() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "type": "error", "code": "invalid_request",
                "description": "Receipt is missing or illegal"
            })))
            .mount(&server)
            .await;
        let result = YooKassa::with_api_base(server.uri())
            .create_invoice(&settings(&[]), &invoice(rub(49_900), None))
            .await;
        assert_eq!(
            result,
            Err(PaymentError::Rejected(
                "Receipt is missing or illegal".into()
            ))
        );
    }

    #[tokio::test]
    async fn автосписание_сохранённой_картой() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/payments"))
            .and(body_partial_json(
                json!({"payment_method_id": "pm-1", "capture": true}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(payment(
                "succeeded",
                "499.00",
                "RUB",
            )))
            .mount(&server)
            .await;
        let outcome = YooKassa::with_api_base(server.uri())
            .charge_saved(&settings(&[]), &invoice(rub(49_900), None), "pm-1")
            .await
            .unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
    }

    #[tokio::test]
    async fn возврат_через_юkassa() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/refunds"))
            .and(header_exists("Idempotence-Key"))
            .and(body_partial_json(json!({
                "payment_id": ID, "amount": {"value": "100.00", "currency": "RUB"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "rf-2", "status": "succeeded", "payment_id": ID,
                "amount": {"value": "100.00", "currency": "RUB"}
            })))
            .mount(&server)
            .await;
        let outcome = YooKassa::with_api_base(server.uri())
            .refund(
                &settings(&[]),
                &RefundRequest {
                    payment_id: PaymentId(41),
                    provider_txid: ID.into(),
                    amount: rub(10_000),
                    reason: Some("Не подошёл тариф".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            outcome,
            RefundOutcome {
                provider_refund_id: Some("rf-2".into()),
                completed: true
            }
        );
    }

    #[test]
    fn настройки_юkassa() {
        let kassa = YooKassa::new();
        assert!(kassa.is_configured(&settings(&[])));
        assert!(kassa.is_configured(&settings(&[("vat_code", json!(7))])));
        assert!(!kassa.is_configured(&settings(&[("vat_code", json!(13))])));
        assert!(!kassa.is_configured(&settings(&[("tax_system_code", json!(1))])));
        assert!(!kassa.is_configured(&settings(&[("vat_code", json!("НДС"))])));
        assert!(!kassa.is_configured(&config(&[
            ("shop_id", json!("shop")),
            ("secret_key", json!("s"))
        ])));
        assert!(kassa.supports_recurring() && kassa.supports_refunds());
    }
}
