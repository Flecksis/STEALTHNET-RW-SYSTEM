use crate::error::{PaymentError, PaymentResult};
use crate::form;
use crate::json::Json;
use crate::md5;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::{NoticeKind, providers};
use sn_domain::settings::SettingField;

const API: &str = "https://pal24.pro/api/v1";
const API_TOKEN: &str = "api_token";
const SHOP_ID: &str = "shop_id";
const KEY_MAX: usize = 512;
const SHOP_MAX: usize = 64;
const REFUND_ROUTE: &str = "refund";
const CHARGEBACK_ROUTE: &str = "chargeback";
const REFUND_PREFIX: &str = "refund:";
const SBP: PayMethod = PayMethod::new("sbp", "СБП", "Через приложение банка");
const CARD: PayMethod = PayMethod::new("card", "Карта", "Карты российских банков");
const PAGE: PayMethod = PayMethod::new(
    "page",
    "Все способы",
    "Способ выбирается на странице оплаты",
);
const CHANNELS: [(PayMethod, Option<&str>); 3] =
    [(SBP, Some("SBP")), (CARD, Some("BANK_CARD")), (PAGE, None)];
const LABEL: ClientLabel = ClientLabel {
    title: "Карта или СБП",
    hint: "Способ выбирается на странице оплаты",
};

pub fn signature(parts: &[&str], token: &str) -> String {
    let mut text = parts.join(":");
    text.push(':');
    text.push_str(token);
    md5::hex(text.as_bytes()).to_ascii_uppercase()
}

fn signed(parts: &[&str], token: &str, received: Option<&str>) -> bool {
    received.is_some_and(|got| {
        util::secret_eq(&signature(parts, token), &got.trim().to_ascii_uppercase())
    })
}

fn key_ok(value: &str) -> bool {
    value.len() <= KEY_MAX && value.bytes().all(|b| b.is_ascii_graphic())
}

fn shop_ok(value: &str) -> bool {
    value.len() <= SHOP_MAX && util::safe_id(value).is_ok()
}

fn channel(chosen: Option<&str>) -> PaymentResult<Option<&'static str>> {
    match chosen {
        None => Ok(None),
        Some(code) => CHANNELS
            .iter()
            .find(|(method, _)| method.code == code)
            .map(|(_, wire)| *wire)
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

fn success(body: &Json) -> bool {
    match body.get("success") {
        Some(Json::Bool(flag)) => *flag,
        Some(Json::String(text)) => matches!(text.trim(), "true" | "1"),
        Some(Json::Number(number)) => number == "1",
        _ => false,
    }
}

fn text<'a>(object: &'a Json, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Json::scalar_text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn plain(message: &str) -> String {
    let key = message.trim();
    let key = key.strip_prefix("api:error.").unwrap_or(key);
    match key {
        "Unauthenticated." | "Unauthenticated" => util::KEYS_REFUSED.into(),
        "ip_access_denied" => {
            "PayPalych не пускает запросы с IP сервера панели: добавьте его в белый список API в кабинете или выключите список".into()
        }
        "shop_not_found" => "PayPalych не нашла магазин с таким ID".into(),
        "access_denied" => "у этого API-токена нет доступа к магазину: проверьте ID магазина".into(),
        "merchant_not_found" => "PayPalych не нашла мерчанта этого API-токена".into(),
        "merchant_banned" => "аккаунт мерчанта заблокирован в PayPalych".into(),
        "shop_not_enabled" => "магазин ещё не включён в PayPalych".into(),
        "invalid_amount" => "PayPalych не приняла сумму счёта".into(),
        "transaction_rejected" => "PayPalych отклонила операцию".into(),
        "payment_method_temporarily_unavailable" => {
            "способ оплаты в PayPalych временно недоступен, попробуйте позже".into()
        }
        "bill_not_found" => "счёт не найден в PayPalych".into(),
        "payment_not_found" => "платёж не найден в PayPalych".into(),
        "balance_not_enough" => "на балансе PayPalych не хватает денег на возврат".into(),
        "refund_tariff_not_found" | "This action is unauthorized." | "This action is unauthorized" => {
            "возвраты через API не включены: попросите поддержку PayPalych их подключить".into()
        }
        "general_error" => "PayPalych ответила внутренней ошибкой, попробуйте позже".into(),
        other => util::clip(other, 200),
    }
}

fn failure(reply: &util::Reply, body: Option<&Json>) -> PaymentError {
    let message = body
        .and_then(|b| b.get("message").or_else(|| b.get("error")))
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(plain);
    reply.failure(message.as_deref())
}

fn unwrap_data(body: Json) -> Json {
    match body.get("data") {
        Some(inner @ Json::Object(_)) => inner.clone(),
        _ => body,
    }
}

fn bill_outcome(bill: &Json) -> PaymentResult<WebhookOutcome> {
    let id = util::safe_id(text(bill, "id").ok_or_else(|| util::bad("нет номера счёта"))?)?;
    let status = text(bill, "status")
        .ok_or_else(|| util::bad("нет статуса счёта"))?
        .to_ascii_uppercase();
    let status = util::clip(&status, 32);
    let currency = bill
        .get("currency_in")
        .cloned()
        .unwrap_or_else(|| Json::from("RUB"));
    let amount = util::json_amount(bill.get("amount"), Some(&currency))?;
    let (event_key, state) = match status.as_str() {
        "SUCCESS" | "OVERPAID" => (format!("{id}:paid"), ProviderStatus::Paid),
        "FAIL" => (format!("{id}:fail"), ProviderStatus::Failed),
        other => (
            format!("{id}:{}", other.to_ascii_lowercase()),
            ProviderStatus::Pending,
        ),
    };
    let outcome = WebhookOutcome {
        event_key,
        payment: util::public_ref(text(bill, "order_id"))
            .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string()))),
        provider_txid: Some(id.to_string()),
        status: state,
        amount: Some(amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    };
    Ok(if status == "UNDERPAID" {
        outcome.with_notice(
            NoticeKind::ManualCredit,
            format!(
                "PayPalych сообщила о недоплате по счёту {id} на {}: покупатель заплатил меньше суммы счёта. \
                 Оплата не засчитана. Проверьте платёж в кабинете PayPalych и решите, вернуть деньги или зачислить их вручную",
                util::amount_text(amount)
            ),
        )
    } else {
        outcome
    })
}

fn refund_state(refund: &Json, key: &str) -> PaymentResult<WebhookOutcome> {
    let status = text(refund, "status")
        .ok_or_else(|| util::bad("нет статуса возврата"))?
        .to_ascii_uppercase();
    let currency = refund
        .get("currency")
        .cloned()
        .unwrap_or_else(|| Json::from("RUB"));
    let amount = util::json_amount(refund.get("amount"), Some(&currency))?;
    let state = match status.as_str() {
        "SUCCESS" => ProviderStatus::PartiallyRefunded { refunded: amount },
        "FAIL" => ProviderStatus::RefundDeclined,
        _ => ProviderStatus::Pending,
    };
    Ok(WebhookOutcome {
        event_key: key.to_string(),
        payment: None,
        provider_txid: None,
        status: state,
        amount: Some(amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    })
}

fn route(request: &WebhookRequest) -> PaymentResult<Option<&'static str>> {
    match request
        .route_suffix
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => Ok(None),
        Some(kind) if kind.eq_ignore_ascii_case(REFUND_ROUTE) => Ok(Some(REFUND_ROUTE)),
        Some(kind) if kind.eq_ignore_ascii_case(CHARGEBACK_ROUTE) => Ok(Some(CHARGEBACK_ROUTE)),
        Some(_) => Err(util::bad(
            "у PayPalych три адреса уведомлений: основной, /refund и /chargeback",
        )),
    }
}

pub struct PayPalych {
    http: reqwest::Client,
    api_base: String,
}

impl Default for PayPalych {
    fn default() -> Self {
        Self::new()
    }
}

impl PayPalych {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        PayPalych {
            http: http_client(),
            api_base: api_base.into().trim_end_matches('/').to_string(),
        }
    }

    async fn send(
        &self,
        config: &ProviderConfig,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        form_body: Option<&[(&str, String)]>,
    ) -> PaymentResult<(util::Reply, Option<Json>)> {
        let mut url = format!("{}{path}", self.api_base);
        if !query.is_empty() {
            url.push('?');
            url.push_str(&form::encode(query));
        }
        let mut builder = self
            .http
            .request(method, url)
            .bearer_auth(config.require(API_TOKEN)?)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(pairs) = form_body {
            builder = builder
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(form::encode(pairs));
        }
        let reply = util::send(builder).await?;
        let parsed = Json::parse(&reply.body);
        Ok((reply, parsed))
    }

    async fn ok_json(
        &self,
        config: &ProviderConfig,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        form_body: Option<&[(&str, String)]>,
    ) -> PaymentResult<Json> {
        let (reply, parsed) = self.send(config, method, path, query, form_body).await?;
        match parsed {
            Some(body) if reply.is_success() && success(&body) => Ok(body),
            Some(body) => Err(failure(&reply, Some(&body))),
            None if reply.is_success() => Err(util::bad("ответ PayPalych не JSON")),
            None => Err(failure(&reply, None)),
        }
    }

    async fn bill_ref(
        &self,
        config: &ProviderConfig,
        bill: &str,
        claimed: Option<&str>,
    ) -> PaymentResult<PaymentRef> {
        let found = self
            .bill(config, bill)
            .await?
            .ok_or_else(|| util::bad("счёт из уведомления не найден в PayPalych"))?;
        if text(&found, "id") != Some(bill) {
            return Err(util::bad("PayPalych вернула другой счёт"));
        }
        let order = util::public_ref(text(&found, "order_id"));
        if let (Some(order), Some(claimed)) = (&order, util::public_ref(claimed))
            && *order != claimed
        {
            return Err(util::bad(
                "номер заказа в уведомлении не совпал с заказом счёта PayPalych",
            ));
        }
        Ok(order.unwrap_or_else(|| PaymentRef::ProviderTxid(bill.to_string())))
    }

    async fn bill(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Option<Json>> {
        let id = util::safe_id(id.trim())?;
        let (reply, parsed) = self
            .send(
                config,
                reqwest::Method::GET,
                "/bill/status",
                &[("id", id.to_string())],
                None,
            )
            .await?;
        let missing = parsed
            .as_ref()
            .and_then(|b| b.get("message"))
            .and_then(Json::as_str)
            .is_some_and(|m| m.contains("bill_not_found"));
        if reply.status == 404 || missing {
            return Ok(None);
        }
        let body = match parsed {
            Some(body) if reply.is_success() && success(&body) => body,
            Some(body) => return Err(failure(&reply, Some(&body))),
            None => return Err(failure(&reply, None)),
        };
        if text(&body, "id") != Some(id) {
            return Err(util::bad("PayPalych вернула другой счёт"));
        }
        Ok(Some(body))
    }

    async fn bill_of_order(
        &self,
        config: &ProviderConfig,
        order: &str,
        hint: Option<&str>,
    ) -> PaymentResult<Option<Json>> {
        if let Some(hint) = hint.filter(|h| util::safe_id(h).is_ok())
            && let Some(bill) = self.bill(config, hint).await?
            && text(&bill, "order_id") == Some(order)
        {
            return Ok(Some(bill));
        }
        let shop = config.require(SHOP_ID)?;
        let found = self
            .ok_json(
                config,
                reqwest::Method::GET,
                "/bill/search",
                &[
                    ("order_id", order.to_string()),
                    ("shop_id", shop.to_string()),
                    ("per_page", "10".to_string()),
                ],
                None,
            )
            .await?;
        Ok(found
            .get("data")
            .and_then(Json::as_array)
            .and_then(|bills| bills.iter().find(|b| text(b, "order_id") == Some(order)))
            .cloned())
    }

    async fn paid_payment(&self, config: &ProviderConfig, bill: &str) -> PaymentResult<Json> {
        let body = self
            .ok_json(
                config,
                reqwest::Method::GET,
                "/bill/payments",
                &[("id", bill.to_string())],
                None,
            )
            .await?;
        body.get("data")
            .and_then(Json::as_array)
            .and_then(|payments| {
                payments.iter().find(|p| {
                    matches!(
                        text(p, "status").map(str::to_ascii_uppercase).as_deref(),
                        Some("SUCCESS" | "OVERPAID")
                    )
                })
            })
            .cloned()
            .ok_or_else(|| {
                PaymentError::Rejected("в счёте PayPalych нет оплаченного платежа".into())
            })
    }

    async fn known_refund(
        &self,
        config: &ProviderConfig,
        payment: &str,
        key: &str,
    ) -> PaymentResult<Option<Json>> {
        let body = self
            .ok_json(
                config,
                reqwest::Method::GET,
                "/refund/search",
                &[("payment_id", payment.to_string())],
                None,
            )
            .await?;
        Ok(body
            .get("data")
            .and_then(Json::as_array)
            .and_then(|refunds| refunds.iter().find(|r| text(r, "order_id") == Some(key)))
            .cloned())
    }

    fn payment_notice(
        &self,
        token: &str,
        fields: &[(String, String)],
    ) -> PaymentResult<Option<(String, Option<String>)>> {
        let get = |key: &str| util::field(fields, key);
        let sign = get("SignatureValue");
        if let (Some(sum), Some(order)) = (get("OutSum"), get("InvId")) {
            if !signed(&[sum, order], token, sign) {
                return Err(PaymentError::BadSignature);
            }
            return Ok(Some((
                order.to_string(),
                get("TrsId").map(|t| t.trim().to_string()),
            )));
        }
        if let (Some(amount), Some(trs)) = (get("Amount"), get("TrsId")) {
            if !signed(&[amount, trs], token, sign) {
                return Err(PaymentError::BadSignature);
            }
            return Ok(None);
        }
        Err(PaymentError::BadSignature)
    }
}

#[async_trait]
impl PaymentProvider for PayPalych {
    fn id(&self) -> &'static str {
        providers::PAL24
    }

    fn title(&self) -> &'static str {
        "PayPalych"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        LABEL
    }

    fn methods(&self, currency: Currency) -> Vec<PayMethod> {
        if currency == Currency::Rub {
            CHANNELS.iter().map(|(method, _)| *method).collect()
        } else {
            Vec::new()
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::secret(
                API_TOKEN,
                "API-токен",
                "Кабинет PayPalych (сейчас Pally, pal24.pro) → API-интеграция. Вставьте токен целиком, вместе с числом и чертой в начале, например 72|AbC…. Им же подписываются уведомления",
            ),
            SettingField::text(
                SHOP_ID,
                "ID магазина",
                "Кабинет PayPalych → Магазины, например LXZv3R7Q8B. Без него уведомления об оплате не придут",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if !config.get(API_TOKEN).is_some_and(key_ok) {
            return Err(PaymentError::NotConfigured(
                "API-токен PayPalych вставлен с лишними символами: нужны латинские буквы, цифры и знаки без пробелов".into(),
            ));
        }
        if !config.get(SHOP_ID).is_some_and(shop_ok) {
            return Err(PaymentError::NotConfigured(
                "ID магазина PayPalych — латинские буквы и цифры из кабинета".into(),
            ));
        }
        Ok(())
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn verifies_webhooks_by_api(&self) -> bool {
        true
    }

    fn supports_refunds(&self) -> bool {
        true
    }

    fn supports_refund_polling(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let wire = channel(request.method.as_deref())?;
        let order = request.payment_public_id.to_string();
        let mut pairs: Vec<(&str, String)> = vec![
            ("amount", request.amount.units()),
            ("shop_id", config.require(SHOP_ID)?.to_string()),
            ("order_id", order.clone()),
            ("description", util::clip(&request.description, 255)),
            ("type", "normal".into()),
            ("currency_in", "RUB".into()),
            ("locale", "ru".into()),
            ("ttl", request.expires_in.num_seconds().max(60).to_string()),
        ];
        if let Some(code) = wire {
            pairs.push(("payment_method", code.into()));
        }
        if let Some(email) = util::client_email(request) {
            pairs.push(("payer_data[email]", email.to_string()));
        }
        let body = self
            .ok_json(
                config,
                reqwest::Method::POST,
                "/bill/create",
                &[],
                Some(&pairs),
            )
            .await?;
        let bill = util::safe_id(
            text(&body, "bill_id").ok_or_else(|| util::bad("PayPalych не вернула номер счёта"))?,
        )?;
        let pay_url = ["link_page_url", "link_url"]
            .iter()
            .filter_map(|key| body.get(key).and_then(Json::as_str))
            .find(|url| util::is_https(url))
            .ok_or_else(|| util::bad("PayPalych не вернула ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(bill.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "bill_id": bill,
                "payment_method": wire,
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let token = config.require(API_TOKEN)?;
        let kind = route(request)?;
        let fields = form::fields(request)?;
        let get = |key: &str| util::field(&fields, key).map(str::trim);
        match kind {
            Some(REFUND_ROUTE) => {
                let (Some(amount), Some(currency), Some(bill), Some(payment), Some(id)) = (
                    get("Amount"),
                    get("Currency"),
                    get("BillId"),
                    get("PaymentId"),
                    get("Id"),
                ) else {
                    return Err(PaymentError::BadSignature);
                };
                if !signed(
                    &[amount, currency, bill, payment, id],
                    token,
                    get("SignatureValue"),
                ) {
                    return Err(PaymentError::BadSignature);
                }
                let id = util::safe_id(id)?;
                let bill = util::safe_id(bill)?;
                let currency = util::pay_currency(currency)?;
                let refunded = Amount::new(util::minor(amount, currency)?, currency);
                let mut outcome = self
                    .fetch_refund(config, &format!("{REFUND_PREFIX}{id}"))
                    .await?
                    .ok_or_else(|| util::bad("номер возврата PayPalych не прочитан"))?;
                if outcome.amount != Some(refunded) {
                    return Err(util::bad(
                        "сумма возврата в уведомлении не совпала с ответом PayPalych",
                    ));
                }
                outcome.payment = Some(self.bill_ref(config, bill, get("InvId")).await?);
                outcome.provider_txid = Some(bill.to_string());
                Ok(outcome)
            }
            Some(_) => {
                let (Some(bill), Some(payment), Some(id)) =
                    (get("BillId"), get("PaymentId"), get("Id"))
                else {
                    return Err(PaymentError::BadSignature);
                };
                if !signed(&[bill, payment, id], token, get("SignatureValue")) {
                    return Err(PaymentError::BadSignature);
                }
                let id = util::safe_id(id)?;
                let bill = util::safe_id(bill)?;
                let reference = Some(self.bill_ref(config, bill, get("InvId")).await?);
                if get("Status").is_none_or(|s| !s.eq_ignore_ascii_case("SUCCESS")) {
                    return Ok(util::skipped(
                        format!(
                            "chargeback:{id}:{}",
                            get("Status").map(|s| util::clip(s, 16)).unwrap_or_default()
                        ),
                        reference,
                        Some(bill.to_string()),
                        WebhookReply::ok(),
                    ));
                }
                Ok(WebhookOutcome {
                    event_key: format!("chargeback:{id}"),
                    payment: reference,
                    provider_txid: Some(bill.to_string()),
                    status: ProviderStatus::Refunded,
                    amount: None,
                    saved_method: None,
                    reply: WebhookReply::ok(),
                    signal: None,
                }
                .with_notice(
                    NoticeKind::Chargeback,
                    format!(
                        "PayPalych сообщила о чарджбэке по счёту {bill}: банк покупателя отозвал оплату. \
                         Платёж отмечен возвращённым, подписка не изменена — решите, отключать ли её"
                    ),
                ))
            }
            None => {
                let Some((order, hint)) = self.payment_notice(token, &fields)? else {
                    return Ok(util::skipped(
                        format!(
                            "p2p:{}",
                            get("TrsId").map(|t| util::clip(t, 64)).unwrap_or_default()
                        ),
                        None,
                        None,
                        WebhookReply::ok(),
                    ));
                };
                let bill = self
                    .bill_of_order(config, &order, hint.as_deref())
                    .await?
                    .ok_or(PaymentError::BadSignature)?;
                bill_outcome(&bill)
            }
        }
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        match self.bill(config, external_id).await? {
            Some(bill) => bill_outcome(&bill).map(Some),
            None => Err(PaymentError::Rejected("счёт не найден в PayPalych".into())),
        }
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let bill = self
            .bill(config, &request.provider_txid)
            .await?
            .ok_or_else(|| PaymentError::Rejected("счёт не найден в PayPalych".into()))?;
        let state = bill_outcome(&bill)?;
        if state.status != ProviderStatus::Paid {
            return Err(PaymentError::Rejected(
                "PayPalych возвращает деньги только по оплаченному счёту".into(),
            ));
        }
        let billed = state.amount.unwrap_or(request.amount);
        if request.amount.minor <= 0 || request.amount.minor > billed.minor {
            return Err(PaymentError::Rejected(format!(
                "в PayPalych по этому счёту можно вернуть не больше {}",
                util::amount_text(billed)
            )));
        }
        let payment = self.paid_payment(config, &request.provider_txid).await?;
        let payment_id = util::safe_id(
            text(&payment, "id").ok_or_else(|| util::bad("у платежа PayPalych нет номера"))?,
        )?
        .to_string();
        let key = request.key.to_string();
        let refund = match self.known_refund(config, &payment_id, &key).await? {
            Some(found) => found,
            None => {
                let refunded = text(&payment, "refunded_amount")
                    .and_then(|r| util::minor(r, PayCurrency::Rub).ok())
                    .unwrap_or(0);
                let full = request.amount == billed && refunded == 0;
                let mut pairs: Vec<(&str, String)> =
                    vec![("payment_id", payment_id.clone()), ("order_id", key)];
                let path = if full {
                    "/refund/full/create"
                } else {
                    pairs.push(("amount", request.amount.units()));
                    "/refund/partial/create"
                };
                unwrap_data(
                    self.ok_json(config, reqwest::Method::POST, path, &[], Some(&pairs))
                        .await?,
                )
            }
        };
        let id = util::safe_id(
            text(&refund, "id").ok_or_else(|| util::bad("PayPalych не вернула номер возврата"))?,
        )?
        .to_string();
        let outcome = refund_state(&refund, &format!("{REFUND_PREFIX}{id}"))?;
        if outcome.status == ProviderStatus::RefundDeclined {
            return Err(PaymentError::Rejected(
                "PayPalych отказала в возврате".into(),
            ));
        }
        Ok(RefundOutcome {
            provider_refund_id: Some(format!("{REFUND_PREFIX}{id}")),
            completed: matches!(outcome.status, ProviderStatus::PartiallyRefunded { .. }),
        })
    }

    async fn fetch_refund(
        &self,
        config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let Some(id) = provider_refund_id
            .strip_prefix(REFUND_PREFIX)
            .and_then(|id| util::safe_id(id).ok())
        else {
            return Ok(None);
        };
        let body = unwrap_data(
            self.ok_json(
                config,
                reqwest::Method::GET,
                "/refund/status",
                &[("id", id.to_string())],
                None,
            )
            .await?,
        );
        if text(&body, "id") != Some(id) {
            return Err(util::bad("PayPalych вернула другой возврат"));
        }
        refund_state(&body, provider_refund_id).map(Some)
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let balance = match self
            .ok_json(config, reqwest::Method::GET, "/merchant/balance", &[], None)
            .await
        {
            Ok(body) => body,
            Err(err) if util::keys_refused(&err) => {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "PayPalych не приняла API-токен",
                )
                .detail("Токен есть в кабинете PayPalych на странице API-интеграции. Вставьте его целиком, вместе с числом и чертой в начале. После смены токена старый сразу перестаёт работать");
            }
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let shop = config.get(SHOP_ID).unwrap_or_default().to_string();
        if let Err(err) = self
            .ok_json(
                config,
                reqwest::Method::GET,
                "/bill/search",
                &[("shop_id", shop.clone()), ("per_page", "1".into())],
                None,
            )
            .await
        {
            return match err {
                PaymentError::Rejected(reason) => ConnectionCheck::new(
                    CheckStatus::SettingsRejected,
                    format!("Токен принят, но магазин {shop} не подходит: {reason}"),
                ),
                other => ConnectionCheck::failed(&other),
            };
        }
        let mut check = ConnectionCheck::ok(format!(
            "API-токен принят, магазин {shop} найден в PayPalych"
        ));
        if let Some(rub) = balance
            .get("balances")
            .and_then(Json::as_array)
            .and_then(|list| {
                list.iter()
                    .find(|b| text(b, "currency").is_some_and(|c| c.eq_ignore_ascii_case("RUB")))
            })
            .and_then(|b| {
                util::json_amount(b.get("balance_available"), Some(&Json::from("RUB"))).ok()
            })
        {
            check = check.detail(format!("Доступно на балансе: {}", util::amount_text(rub)));
        }
        match context.webhook_url.as_deref() {
            Some(url) => check
                .detail(format!(
                    "В настройках магазина PayPalych укажите Result URL {url}. Каждое уведомление об оплате панель перепроверяет запросом к API PayPalych"
                ))
                .detail(format!(
                    "Refund URL — {url}/{REFUND_ROUTE}, Chargeback URL — {url}/{CHARGEBACK_ROUTE}: по ним панель узнает о возвратах и чарджбэках"
                )),
            None => check.detail(
                "Адрес панели не задан в основных настройках: PayPalych некуда присылать уведомления",
            ),
        }
    }
}

#[cfg(test)]
mod tests;
