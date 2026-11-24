use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use sn_domain::money::Currency;
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use uuid::Uuid;

const LIVE: &str = "https://api.wata.pro/api/h2h";
const SANDBOX: &str = "https://api-sandbox.wata.pro/api/h2h";
const TOKEN: &str = "api_token";
const SANDBOX_MODE: &str = "sandbox";
const TERMINAL: &str = "terminal_id";
const TOKEN_MAX: usize = 4096;
const DESCRIPTION_MAX: usize = 255;
const KEY_TTL_SECONDS: u64 = 6 * 3600;
const KEY_RETRY_SECONDS: u64 = 60;
const LABEL: ClientLabel = ClientLabel {
    title: "Карта",
    hint: "Карты российских банков и СБП",
};

fn min_lifetime() -> Duration {
    Duration::minutes(15)
}

fn max_lifetime() -> Duration {
    Duration::days(30)
}

pub fn verify_signature(public_key_pem: &str, body: &[u8], header: &str) -> bool {
    let compact: String = header.chars().filter(|c| !c.is_whitespace()).collect();
    let Ok(raw) = STANDARD.decode(compact.as_bytes()) else {
        return false;
    };
    let Ok(key) = jsonwebtoken::DecodingKey::from_rsa_pem(public_key_pem.as_bytes()) else {
        return false;
    };
    jsonwebtoken::crypto::verify(
        &URL_SAFE_NO_PAD.encode(raw),
        body,
        &key,
        jsonwebtoken::Algorithm::RS512,
    )
    .unwrap_or(false)
}

fn text<'a>(object: &'a Json, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn uuid_of(object: &Json, key: &str) -> Option<Uuid> {
    text(object, key).and_then(|v| Uuid::parse_str(v).ok())
}

fn error_text(body: &[u8]) -> Option<(Option<String>, String)> {
    let parsed = Json::parse(body)?;
    let error = parsed.get("error")?;
    let code = text(error, "code").map(str::to_string);
    let message = text(error, "message")
        .or_else(|| text(error, "details"))
        .map(str::to_string)
        .or_else(|| {
            error
                .get("validationErrors")
                .and_then(Json::as_array)
                .and_then(|items| items.first())
                .and_then(|item| text(item, "message"))
                .map(str::to_string)
        })?;
    Some((code, message))
}

fn explained(code: Option<&str>, message: &str) -> String {
    let short = code
        .map(|c| c.rsplit(':').next().unwrap_or(c))
        .unwrap_or_default();
    match short {
        "PL_1001" => "сумма вне пределов, которые WATA разрешает терминалу".into(),
        "PL_1002" => "эта валюта не разрешена для терминала WATA".into(),
        "PL_1004" => "WATA не приняла сумму счёта".into(),
        "TER_3001" => "приём платежей на терминале WATA отключён".into(),
        "TER_3015" => "у токена нет доступа к терминалу WATA".into(),
        "MER_1004" => "WATA заблокировала аккаунт магазина".into(),
        "TRA_1007" => "оплата в WATA не в том статусе, чтобы вернуть деньги".into(),
        "TRA_1101" => "WATA не приняла сумму возврата".into(),
        "TRA_1102" => "на балансе терминала WATA не хватает денег на возврат".into(),
        "TRA_1103" => "по этой оплате в WATA уже идёт возврат, дождитесь его результата".into(),
        "TRA_1105" => "баланс терминала WATA в этой валюте недоступен".into(),
        "TRA_1108" => "WATA не разрешает возврат по этой оплате".into(),
        "TRA_1109" => "по этой оплате в WATA идёт чарджбэк, вернуть деньги нельзя".into(),
        _ => util::clip(message, 200),
    }
}

fn failure(reply: &util::Reply) -> PaymentError {
    match error_text(&reply.body) {
        Some((code, message)) if !matches!(reply.status, 401) => {
            reply.failure(Some(&explained(code.as_deref(), &message)))
        }
        _ => reply.failure(None),
    }
}

fn payment_ref(object: &Json, txid: &str) -> Option<PaymentRef> {
    util::public_ref(text(object, "orderId"))
        .or_else(|| Some(PaymentRef::ProviderTxid(txid.to_string())))
}

fn is_refund(object: &Json) -> bool {
    text(object, "kind").is_some_and(|k| k.eq_ignore_ascii_case("refund"))
}

fn status_of(object: &Json) -> String {
    text(object, "status")
        .or_else(|| text(object, "transactionStatus"))
        .unwrap_or("Created")
        .to_ascii_lowercase()
}

fn transaction_outcome(tx: &Json) -> PaymentResult<WebhookOutcome> {
    let id = uuid_of(tx, "id")
        .or_else(|| uuid_of(tx, "transactionId"))
        .ok_or_else(|| util::bad("в транзакции WATA нет её номера"))?
        .to_string();
    let status = status_of(tx);
    let amount = || util::json_amount(tx.get("amount"), tx.get("currency"));
    if is_refund(tx) {
        let original = uuid_of(tx, "originalTransactionId").map(|u| u.to_string());
        let payment = util::public_ref(text(tx, "orderId"))
            .or_else(|| original.clone().map(PaymentRef::ProviderTxid));
        let event_key = format!("refund:{id}");
        let (state, refunded) = match status.as_str() {
            "paid" => {
                let refunded = amount()?;
                (
                    ProviderStatus::PartiallyRefunded { refunded },
                    Some(refunded),
                )
            }
            "declined" => (ProviderStatus::RefundDeclined, None),
            _ => {
                return Ok(util::skipped(
                    event_key,
                    payment,
                    original,
                    WebhookReply::ok(),
                ));
            }
        };
        return Ok(WebhookOutcome {
            event_key,
            payment,
            provider_txid: original,
            status: state,
            amount: refunded,
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        });
    }
    let payment = payment_ref(tx, &id);
    if status != "paid" {
        return Ok(util::skipped(
            format!("{id}:{}", util::clip(&status, 16)),
            payment,
            None,
            WebhookReply::ok(),
        ));
    }
    Ok(WebhookOutcome {
        event_key: format!("{id}:paid"),
        payment,
        provider_txid: Some(id),
        status: ProviderStatus::Paid,
        amount: Some(amount()?),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    })
}

fn lifetime(requested: Duration) -> Duration {
    requested.max(min_lifetime()).min(max_lifetime())
}

fn deadline(raw: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = raw?;
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|d| d.and_utc())
        })
}

fn token_ok(token: &str) -> bool {
    token.len() <= TOKEN_MAX && token.bytes().all(|b| b.is_ascii_graphic())
}

fn terminal(config: &ProviderConfig) -> PaymentResult<Option<Uuid>> {
    match config.get(TERMINAL) {
        None => Ok(None),
        Some(raw) => Uuid::parse_str(raw).map(Some).map_err(|_| {
            PaymentError::NotConfigured(
                "ID терминала WATA — это UUID из кабинета WATA, например 3b16a2f1-dead-4e5d-abff-90865d1e13b1".into(),
            )
        }),
    }
}

fn foreign_terminal(object: &Json, own: Option<Uuid>) -> bool {
    match (own, text(object, "terminalPublicId")) {
        (Some(own), Some(got)) => Uuid::parse_str(got).ok() != Some(own),
        _ => false,
    }
}

struct CachedKey {
    pem: String,
    fetched: Instant,
}

pub struct Wata {
    http: sn_egress::Http,
    live: String,
    sandbox: String,
    keys: Mutex<HashMap<String, CachedKey>>,
    refunds: Mutex<HashMap<Uuid, String>>,
}

impl Default for Wata {
    fn default() -> Self {
        Self::new()
    }
}

impl Wata {
    pub fn new() -> Self {
        Wata {
            http: http_client(sn_domain::payment::providers::WATA),
            live: LIVE.to_string(),
            sandbox: SANDBOX.to_string(),
            keys: Mutex::new(HashMap::new()),
            refunds: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        let base = api_base.into().trim_end_matches('/').to_string();
        Wata {
            http: http_client(sn_domain::payment::providers::WATA),
            live: base.clone(),
            sandbox: base,
            keys: Mutex::new(HashMap::new()),
            refunds: Mutex::new(HashMap::new()),
        }
    }

    fn base(&self, config: &ProviderConfig) -> &str {
        if config.flag(SANDBOX_MODE) {
            &self.sandbox
        } else {
            &self.live
        }
    }

    fn request(
        &self,
        config: &ProviderConfig,
        method: reqwest::Method,
        path: &str,
    ) -> PaymentResult<reqwest::RequestBuilder> {
        let token = config.require(TOKEN)?;
        Ok(self
            .http
            .request(method, format!("{}{path}", self.base(config)))
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "application/json"))
    }

    async fn transaction(&self, config: &ProviderConfig, id: &str) -> PaymentResult<Option<Json>> {
        let id =
            Uuid::parse_str(id.trim()).map_err(|_| util::bad("номер транзакции WATA не UUID"))?;
        let reply = util::send(self.request(
            config,
            reqwest::Method::GET,
            &format!("/transactions/{id}"),
        )?)
        .await?;
        if reply.status == 404 {
            return Ok(None);
        }
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        if uuid_of(&parsed, "id") != Some(id) {
            return Err(util::bad("WATA вернула другую транзакцию"));
        }
        Ok(Some(parsed))
    }

    fn cached_key(&self, base: &str) -> Option<(String, u64)> {
        let guard = match self.keys.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .get(base)
            .map(|k| (k.pem.clone(), k.fetched.elapsed().as_secs()))
    }

    async fn fetch_key(&self, base: &str) -> PaymentResult<String> {
        let reply = util::send(
            self.http
                .get(format!("{base}/public-key"))
                .header(reqwest::header::ACCEPT, "application/json"),
        )
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        let pem = text(&parsed, "value")
            .filter(|v| v.contains("PUBLIC KEY"))
            .ok_or_else(|| util::bad("WATA не вернула ключ проверки уведомлений"))?
            .replace("\\n", "\n");
        jsonwebtoken::DecodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|_| util::bad("ключ проверки уведомлений WATA не разобран"))?;
        let mut guard = match self.keys.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(
            base.to_string(),
            CachedKey {
                pem: pem.clone(),
                fetched: Instant::now(),
            },
        );
        Ok(pem)
    }

    async fn genuine(
        &self,
        config: &ProviderConfig,
        body: &[u8],
        header: &str,
    ) -> PaymentResult<bool> {
        let base = self.base(config).to_string();
        let cached = self.cached_key(&base);
        if let Some((pem, age)) = &cached
            && *age < KEY_TTL_SECONDS
            && verify_signature(pem, body, header)
        {
            return Ok(true);
        }
        if cached
            .as_ref()
            .is_some_and(|(_, age)| *age < KEY_RETRY_SECONDS)
        {
            return Ok(false);
        }
        let pem = self.fetch_key(&base).await?;
        Ok(verify_signature(&pem, body, header))
    }

    async fn paid_by_order(
        &self,
        config: &ProviderConfig,
        order: Uuid,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let reply = util::send(
            self.request(config, reqwest::Method::GET, "/v2/transactions")?
                .query(&[
                    ("OrderId", order.to_string().as_str()),
                    ("MaxResultCount", "100"),
                ]),
        )
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        let items = parsed
            .get("items")
            .and_then(Json::as_array)
            .unwrap_or_default();
        let wanted = order.to_string();
        let paid = items.iter().find(|tx| {
            !is_refund(tx)
                && status_of(tx) == "paid"
                && text(tx, "orderId").is_some_and(|o| o.eq_ignore_ascii_case(&wanted))
        });
        match paid {
            Some(tx) => transaction_outcome(tx).map(Some),
            None => Ok(None),
        }
    }

    fn remembered(&self, key: Uuid) -> Option<String> {
        match self.refunds.lock() {
            Ok(guard) => guard.get(&key).cloned(),
            Err(poisoned) => poisoned.into_inner().get(&key).cloned(),
        }
    }

    fn remember(&self, key: Uuid, refund: &str) {
        let mut guard = match self.refunds.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(key, refund.to_string());
    }

    async fn probe(&self, config: &ProviderConfig) -> PaymentResult<util::Reply> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let reply = util::send(
            self.request(config, reqwest::Method::GET, "/finance/balance")?
                .query(&[("Date", today.as_str())]),
        )
        .await?;
        if matches!(reply.status, 403 | 404) {
            return util::send(
                self.request(config, reqwest::Method::GET, "/links")?
                    .query(&[("MaxResultCount", "1")]),
            )
            .await;
        }
        Ok(reply)
    }
}

#[async_trait]
impl PaymentProvider for Wata {
    fn api_url(&self, config: &ProviderConfig) -> Option<String> {
        Some(self.base(config).to_string())
    }

    fn id(&self) -> &'static str {
        providers::WATA
    }

    fn title(&self) -> &'static str {
        "WATA"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub, Currency::Usd]
    }

    fn client_label(&self, currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        match currency {
            Currency::Rub => LABEL,
            Currency::Usd => ClientLabel {
                title: "Карта",
                hint: "Банковская карта",
            },
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::secret(
                TOKEN,
                "Токен доступа",
                "Кабинет WATA → Терминалы → терминал → Access Token. Токен определяет терминал, у которого создаются ссылки на оплату",
            ),
            SettingField::flag(
                SANDBOX_MODE,
                "Тестовая среда",
                "Запросы идут в песочницу WATA, оплата тестовыми картами. Токен для неё выдаётся в тестовом кабинете, который открывает менеджер WATA",
            ),
            SettingField::text(
                TERMINAL,
                "ID терминала",
                "Необязательно. ID терминала из кабинета WATA, его же показывает «Проверить подключение». Если указан, панель принимает уведомления только этого терминала",
            )
            .optional(),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        if !config.get(TOKEN).is_some_and(token_ok) {
            return Err(PaymentError::NotConfigured(
                "токен WATA вставлен с лишними символами: нужен текст без пробелов и переносов строк".into(),
            ));
        }
        terminal(config).map(|_| ())
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

    fn polls_by_payment_id(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        if request.amount.as_money().is_none() {
            return Err(PaymentError::UnsupportedCurrency(
                request.amount.currency.code().into(),
            ));
        }
        if request.method.is_some() {
            return Err(PaymentError::Rejected(
                "этот способ оплаты недоступен".into(),
            ));
        }
        let order = request.payment_public_id.to_string();
        let until = Utc::now() + lifetime(request.expires_in);
        let mut body = Json::object([
            ("type", Json::from("OneTime")),
            ("amount", Json::number(request.amount.units())),
            ("currency", Json::from(request.amount.currency.code())),
            (
                "description",
                Json::from(util::clip(&request.description, DESCRIPTION_MAX)),
            ),
            ("orderId", Json::from(order.clone())),
            (
                "expirationDateTime",
                Json::from(until.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            ),
        ]);
        if let Some(url) = util::return_url(request) {
            body.push("successRedirectUrl", Json::from(url.clone()));
            body.push("failRedirectUrl", Json::from(url));
        }
        if let Some(email) = util::client_email(request) {
            body.push("email", Json::from(email));
        }
        let reply = util::send(
            self.request(config, reqwest::Method::POST, "/links")?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        let pay_url = text(&parsed, "url")
            .filter(|u| util::is_https(u))
            .ok_or_else(|| util::bad("WATA не вернула ссылку на оплату"))?;
        if text(&parsed, "orderId").is_some_and(|o| !o.eq_ignore_ascii_case(&order)) {
            return Err(util::bad("WATA вернула ссылку другого заказа"));
        }
        if parsed.get("amount").is_some() {
            let billed = util::json_amount(parsed.get("amount"), parsed.get("currency"))?;
            if billed != request.amount {
                return Err(util::bad(format!(
                    "WATA выставила счёт на {} вместо {}",
                    util::amount_text(billed),
                    util::amount_text(request.amount)
                )));
            }
        }
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: None,
            expires_at: deadline(text(&parsed, "expirationDateTime")).or(Some(until)),
            payload: serde_json::json!({
                "link_id": text(&parsed, "id"),
                "terminal": text(&parsed, "terminalPublicId"),
                "status": text(&parsed, "status"),
                "sandbox": config.flag(SANDBOX_MODE),
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        config.require(TOKEN)?;
        if request
            .route_suffix
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(util::bad(
                "у WATA один адрес уведомлений, без дополнительной части",
            ));
        }
        let Some(header) = request.header("x-signature") else {
            return Err(PaymentError::BadSignature);
        };
        if !self.genuine(config, &request.body, header).await? {
            return Err(PaymentError::BadSignature);
        }
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        let own = terminal(config)?;
        if foreign_terminal(&body, own) {
            return Err(util::bad(
                "уведомление другого терминала WATA: его ID не совпал с ID терминала в настройках",
            ));
        }
        if let Some(event) = text(&body, "eventType") {
            return Ok(util::skipped(
                util::clip(event, 64),
                None,
                None,
                WebhookReply::ok(),
            ));
        }
        let id = uuid_of(&body, "transactionId")
            .ok_or_else(|| util::bad("нет transactionId"))?
            .to_string();
        let status = status_of(&body);
        let refund = is_refund(&body);
        let settles = status == "paid" || (refund && status == "declined");
        if !settles {
            let original = uuid_of(&body, "originalTransactionId").map(|u| u.to_string());
            let payment = if refund {
                util::public_ref(text(&body, "orderId"))
                    .or_else(|| original.clone().map(PaymentRef::ProviderTxid))
            } else {
                payment_ref(&body, &id)
            };
            let key = if refund {
                format!("refund:{id}:{}", util::clip(&status, 16))
            } else {
                format!("{id}:{}", util::clip(&status, 16))
            };
            return Ok(util::skipped(key, payment, None, WebhookReply::ok()));
        }
        let tx = self.transaction(config, &id).await?.ok_or_else(|| {
            util::bad(
                "у терминала панели нет такой транзакции WATA: уведомление пришло от другого терминала",
            )
        })?;
        if is_refund(&tx) != refund {
            return Err(util::bad("вид транзакции WATA не совпал с уведомлением"));
        }
        if foreign_terminal(&tx, own) {
            return Err(util::bad(
                "транзакция WATA относится к другому терминалу, чем указан в настройках",
            ));
        }
        transaction_outcome(&tx)
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let order = Uuid::parse_str(external_id.trim())
            .map_err(|_| util::bad("номер заказа WATA не UUID"))?;
        match self.paid_by_order(config, order).await? {
            Some(outcome) => Ok(Some(outcome)),
            None => Ok(Some(util::skipped(
                format!("order:{order}:open"),
                Some(PaymentRef::PublicId(order)),
                None,
                WebhookReply::ok(),
            ))),
        }
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        if let Some(known) = self.remembered(request.key) {
            return Ok(RefundOutcome {
                provider_refund_id: Some(known),
                completed: false,
            });
        }
        let original = Uuid::parse_str(request.provider_txid.trim())
            .map_err(|_| PaymentError::Rejected("у платежа нет номера транзакции WATA".into()))?;
        let body = Json::object([
            ("originalTransactionId", Json::from(original.to_string())),
            ("amount", Json::number(request.amount.units())),
        ]);
        let reply = util::send(
            self.request(config, reqwest::Method::POST, "/transactions/refunds")?
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_bytes()),
        )
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        let parsed = reply.json()?;
        if status_of(&parsed) == "declined" {
            let reason = text(&parsed, "errorDescription")
                .or_else(|| text(&parsed, "errorCode"))
                .map(|r| explained(text(&parsed, "errorCode"), r))
                .unwrap_or_else(|| "WATA отклонила возврат".into());
            return Err(PaymentError::Rejected(reason));
        }
        let id = uuid_of(&parsed, "transactionId")
            .ok_or_else(|| util::bad("WATA не вернула номер возврата"))?
            .to_string();
        self.remember(request.key, &id);
        Ok(RefundOutcome {
            provider_refund_id: Some(id),
            completed: false,
        })
    }

    async fn fetch_refund(
        &self,
        config: &ProviderConfig,
        provider_refund_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        if Uuid::parse_str(provider_refund_id.trim()).is_err() {
            return Ok(None);
        }
        let tx = self
            .transaction(config, provider_refund_id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("возврат не найден в WATA".into()))?;
        if !is_refund(&tx) {
            return Err(util::bad("транзакция WATA не возврат"));
        }
        transaction_outcome(&tx).map(Some)
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let reply = match self.probe(config).await {
            Ok(reply) => reply,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let sandbox = config.flag(SANDBOX_MODE);
        if matches!(reply.status, 401 | 403) {
            let mut check = ConnectionCheck::new(
                CheckStatus::KeysRejected,
                "WATA не приняла токен доступа",
            )
            .detail("Токен выпускается в кабинете WATA: Терминалы → терминал → Access Token. Срок жизни токена от 1 до 12 месяцев, после него нужен новый");
            check = check.detail(if sandbox {
                "Включена тестовая среда: нужен токен из тестового кабинета WATA, боевой там не подходит"
            } else {
                "Если токен из тестового кабинета WATA, включите тестовую среду"
            });
            return check;
        }
        if reply.status == 429 {
            return ConnectionCheck::new(
                CheckStatus::Unreachable,
                "WATA ограничивает частоту запросов: повторите проверку через минуту",
            );
        }
        if !reply.is_success() {
            return ConnectionCheck::failed(&failure(&reply));
        }
        let mut check = ConnectionCheck::ok("Токен принят WATA");
        let wanted = match terminal(config) {
            Ok(wanted) => wanted,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        if let Some(parsed) = Json::parse(&reply.body)
            && let Some(found) = text(&parsed, "terminalPublicId")
        {
            if foreign_terminal(&parsed, wanted) {
                return ConnectionCheck::new(
                    CheckStatus::SettingsRejected,
                    "ID терминала в настройках не совпадает с терминалом токена WATA",
                )
                .detail(format!(
                    "Токен относится к терминалу {}. Впишите этот ID в поле «ID терминала» или очистите поле: с другим ID панель отклонит уведомления об оплате",
                    util::clip(found, 64)
                ));
            }
            let currency = text(&parsed, "currency")
                .map(|c| format!(", валюта баланса {}", util::clip(c, 8)))
                .unwrap_or_default();
            check = check.detail(format!(
                "Токен относится к терминалу {}{currency}",
                util::clip(found, 64)
            ));
            if wanted.is_none() {
                check = check.detail(
                    "Чтобы панель принимала уведомления только этого терминала, впишите его ID в поле «ID терминала»",
                );
            }
        }
        let base = self.base(config).to_string();
        match self.fetch_key(&base).await {
            Ok(_) => {}
            Err(_) => {
                check = check.detail(
                    "Ключ проверки уведомлений WATA сейчас не скачался: уведомления будут отклоняться, пока он недоступен",
                );
            }
        }
        if sandbox {
            check = check.detail(
                "Включена тестовая среда: ссылки создаются в песочнице WATA, оплата тестовыми картами. Перед приёмом настоящих оплат выключите её и вставьте боевой токен",
            );
        }
        match context.webhook_url.as_deref() {
            Some(url) => {
                check = check.detail(format!(
                    "Адрес уведомлений {url} укажите в кабинете WATA в настройках терминала. WATA не даёт прочитать его по API, поэтому сверьте его сами"
                ));
            }
            None => {
                check = check.detail(
                    "Адрес панели не задан в основных настройках: WATA некуда присылать уведомления",
                );
            }
        }
        check = check.detail(
            "WATA принимает запросы только с согласованных IP-адресов: если ссылки не создаются, передайте менеджеру WATA адрес сервера панели",
        );
        check.scrub(&[config.get(TOKEN).unwrap_or_default()]);
        check
    }
}

#[cfg(test)]
mod tests;
