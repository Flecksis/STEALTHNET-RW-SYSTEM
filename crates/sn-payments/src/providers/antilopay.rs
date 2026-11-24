use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::{NoticeKind, providers};
use sn_domain::settings::SettingField;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

mod keys;

use keys::Keys;
pub use keys::{signature, verify_signature};

const API: &str = "https://lk.antilopay.com/api/v1";
const PROJECT_ID: &str = "project_id";
const SECRET_ID: &str = "secret_id";
const SECRET_KEY: &str = "secret_key";
const CALLBACK_KEY: &str = "callback_key";
const VAT: &str = "vat";
const VAT_RATES: [&str; 3] = ["none", "10", "22"];
const VAT_LABELS: [&str; 3] = ["Не передавать", "10%", "22%"];
const SIGN_VERSION: &str = "1";
const PRODUCT_TYPE: &str = "services";
const PRODUCT_MAX: usize = 128;
const DESCRIPTION_MAX: usize = 255;
const EMAIL_MAX: usize = 128;
const URL_MAX: usize = 500;
const ORDER_MAX: usize = 100;
const ID_MAX: usize = 64;
const SECRET_ID_MAX: usize = 256;
const CALLBACK_IPS: &str = "81.177.221.226 и 87.228.9.243";

const SBP: PayMethod = PayMethod::new("sbp", "СБП", "Через приложение банка");
const CARD: PayMethod = PayMethod::new("card", "Карта", "Карты российских банков");
const SBERPAY: PayMethod = PayMethod::new("sberpay", "SberPay", "Через СберБанк Онлайн");
const CARD_BY: PayMethod =
    PayMethod::new("card_by", "Карта (Беларусь)", "Карты белорусских банков");
const ERIP: PayMethod = PayMethod::new("erip", "ЕРИП", "Через интернет-банк");
const PAGE: PayMethod = PayMethod::new(
    "page",
    "Все способы",
    "Способ выбирается на странице оплаты",
);
const CHANNELS: [(PayMethod, Option<&str>); 6] = [
    (SBP, Some("SBP")),
    (CARD, Some("CARD_RU")),
    (SBERPAY, Some("SBER_PAY")),
    (CARD_BY, Some("CARD_BY")),
    (ERIP, Some("ERIP")),
    (PAGE, None),
];
const LABEL: ClientLabel = ClientLabel {
    title: "Карта или СБП",
    hint: "Способ выбирается на странице оплаты",
};

const IP_REFUSED: &str = "Antilopay не принимает запросы с IP-адреса сервера панели: разрешите его в кабинете Antilopay или через её поддержку";
const SERVER_ERROR: u16 = 500;
const BROKEN: [i64; 2] = [500, 1000];
const UNKNOWN_PROJECT: i64 = 4;
const UNCONFIRMED_PROJECT: i64 = 6;
const UNKNOWN_PAYMENT: i64 = 8;
const PAYMENT_IP: i64 = 22;
const UNKNOWN_REFUND: i64 = 10;
const REFUND_IP: i64 = 15;
const REPEATED_REFUND: i64 = 18;

fn text<'a>(object: &'a Json, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Json::scalar_text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn rubles(object: &Json, key: &str) -> PaymentResult<Amount> {
    util::json_amount(object.get(key), Some(&Json::from(PayCurrency::Rub.code())))
}

fn project(config: &ProviderConfig) -> PaymentResult<&str> {
    let id = config.require(PROJECT_ID)?;
    if id.len() <= ID_MAX && util::safe_id(id).is_ok() {
        Ok(id)
    } else {
        Err(PaymentError::NotConfigured(
            "ID проекта Antilopay — латинские буквы и цифры из кабинета, без пробелов".into(),
        ))
    }
}

fn secret_id(config: &ProviderConfig) -> PaymentResult<&str> {
    let id = config.require(SECRET_ID)?;
    if id.len() <= SECRET_ID_MAX && id.bytes().all(|b| b.is_ascii_graphic()) {
        Ok(id)
    } else {
        Err(PaymentError::NotConfigured(
            "Secret ID Antilopay вставлен с лишними символами: нужны латинские буквы, цифры и знаки без пробелов".into(),
        ))
    }
}

fn vat(config: &ProviderConfig) -> PaymentResult<Option<&str>> {
    match config.get(VAT) {
        None => Ok(None),
        Some(rate) if rate == VAT_RATES[0] => Ok(None),
        Some(rate) if VAT_RATES.contains(&rate) => Ok(Some(rate)),
        Some(_) => Err(PaymentError::NotConfigured(
            "ставка НДС Antilopay — 10% или 22%".into(),
        )),
    }
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

fn buyer_email(request: &InvoiceRequest) -> String {
    if let Some(email) =
        util::client_email(request).filter(|e| e.is_ascii() && e.len() <= EMAIL_MAX)
    {
        return email.to_string();
    }
    let host = reqwest::Url::parse(&request.webhook_url)
        .ok()
        .and_then(|url| url.domain().map(str::to_string))
        .filter(|host| host.contains('.'))
        .unwrap_or_else(|| "telegram.org".into());
    format!("client{}@{host}", request.client.client_id.0)
}

fn back_url(request: &InvoiceRequest) -> Option<String> {
    util::return_url(request).filter(|url| util::is_https(url) && url.len() <= URL_MAX)
}

#[derive(Debug)]
enum Answer {
    Done(Json),
    Refused { code: i64, message: String },
}

fn answer(reply: &util::Reply) -> PaymentResult<Answer> {
    let parsed = Json::parse(&reply.body);
    let code = parsed
        .as_ref()
        .and_then(|body| text(body, "code"))
        .and_then(|raw| raw.parse::<i64>().ok());
    match (parsed, code) {
        (Some(body), Some(0)) if reply.is_success() => Ok(Answer::Done(body)),
        (Some(body), Some(code)) if code != 0 => {
            if BROKEN.contains(&code) {
                return Err(PaymentError::Http {
                    status: SERVER_ERROR,
                });
            }
            Ok(Answer::Refused {
                code,
                message: text(&body, "error")
                    .map(|m| util::clip(m, 200))
                    .unwrap_or_else(|| format!("код ошибки {code}")),
            })
        }
        _ if reply.is_success() => Err(PaymentError::Transport(
            "ответ Antilopay не разобран".into(),
        )),
        _ => Err(reply.failure(None)),
    }
}

fn refused(code: i64, known: Option<&str>, message: &str) -> PaymentError {
    PaymentError::Rejected(match (code, known) {
        (1..=3, _) => util::KEYS_REFUSED.into(),
        (_, Some(text)) => text.into(),
        _ => format!("Antilopay: {message}"),
    })
}

fn create_refused(code: i64, message: &str) -> PaymentError {
    let known = match code {
        UNKNOWN_PROJECT => Some("Antilopay не нашла проект с таким ID"),
        5 => Some("в Antilopay уже есть платёж с таким номером"),
        UNCONFIRMED_PROJECT => Some("проект Antilopay ещё не подтверждён"),
        7 | 21 => Some("в проекте Antilopay нет доступных способов оплаты"),
        9 | 27 => Some("Antilopay не разрешает проекту принимать платежи через API"),
        10..=12 => Some("Antilopay не приняла почту покупателя"),
        13 => Some("Antilopay не приняла сумму счёта"),
        14 => Some("Antilopay не приняла адрес возврата на сайт"),
        17 => Some("Antilopay требует ставку НДС: выберите её в настройках платёжки"),
        20 => Some("сумма меньше минимальной для проекта Antilopay"),
        PAYMENT_IP => Some(IP_REFUSED),
        23 | 26 => Some("превышен лимит платежей проекта Antilopay"),
        25 => Some("этот способ оплаты не подключён в проекте Antilopay"),
        31 => Some("сумма больше максимальной для проекта Antilopay"),
        32 => Some(
            "Antilopay требует IP-адрес покупателя, панель его не передаёт: попросите поддержку Antilopay снять это требование",
        ),
        _ => None,
    };
    refused(code, known, message)
}

fn check_refused(code: i64, message: &str) -> PaymentError {
    let known = match code {
        UNKNOWN_PROJECT => Some("Antilopay не нашла проект с таким ID"),
        PAYMENT_IP => Some(IP_REFUSED),
        _ => None,
    };
    refused(code, known, message)
}

fn refund_refused(code: i64, message: &str) -> PaymentError {
    let known = match code {
        UNKNOWN_PAYMENT => Some("платёж не найден в Antilopay"),
        9 | 16 => Some("по этому платежу Antilopay не делает возврат"),
        UNKNOWN_REFUND => Some("возврат не найден в Antilopay"),
        11 => Some("Antilopay не разрешает возвраты через API: их включает её поддержка"),
        12 => Some("Antilopay не приняла сумму возврата"),
        REFUND_IP => Some(IP_REFUSED),
        17 => Some("на балансе проекта Antilopay не хватает денег на возврат"),
        REPEATED_REFUND => Some("возврат с таким номером в Antilopay уже есть"),
        20 => Some("по этому платежу Antilopay возвращает только всю сумму"),
        _ => None,
    };
    refused(code, known, message)
}

struct Returned {
    total: i64,
    only: Option<String>,
}

fn returned(payment: &Json) -> PaymentResult<Returned> {
    let mut total: i64 = 0;
    let mut ids = Vec::new();
    for refund in payment
        .get("refunds")
        .and_then(Json::as_array)
        .unwrap_or_default()
    {
        if text(refund, "status").is_some_and(|s| s.eq_ignore_ascii_case("COMPLETE")) {
            total = rubles(refund, "amount")?.minor.saturating_add(total);
            ids.push(text(refund, "refund_id").map(str::to_string));
        }
    }
    Ok(Returned {
        total,
        only: if ids.len() == 1 { ids.remove(0) } else { None },
    })
}

fn payment_outcome(payment: &Json) -> PaymentResult<WebhookOutcome> {
    let id =
        util::safe_id(text(payment, "payment_id").ok_or_else(|| util::bad("нет payment_id"))?)?;
    let status = text(payment, "status")
        .ok_or_else(|| util::bad("нет статуса платежа"))?
        .to_ascii_uppercase();
    let status = util::clip(&status, 32);
    let amount = util::json_amount(payment.get("original_amount"), payment.get("currency"));
    let reference = util::public_ref(text(payment, "order_id"))
        .or_else(|| Some(PaymentRef::ProviderTxid(id.to_string())));
    let outcome =
        |event_key: String, status: ProviderStatus, amount: Option<Amount>| WebhookOutcome {
            event_key,
            payment: reference.clone(),
            provider_txid: Some(id.to_string()),
            status,
            amount,
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        };
    Ok(match status.as_str() {
        "SUCCESS" => {
            let amount = amount?;
            let returned = returned(payment)?;
            if returned.total == 0 {
                outcome(format!("{id}:paid"), ProviderStatus::Paid, Some(amount))
            } else if returned.total >= amount.minor {
                let key = returned.only.unwrap_or_else(|| id.to_string());
                outcome(
                    format!("refund:{key}"),
                    ProviderStatus::Refunded,
                    Some(amount),
                )
            } else {
                let back = Amount::new(returned.total, amount.currency);
                outcome(
                    format!("{id}:returned:{}", returned.total),
                    ProviderStatus::Pending,
                    Some(back),
                )
                .with_notice(
                    NoticeKind::RefundReported,
                    format!(
                        "По платежу {id} Antilopay уже вернула {} из {}, поэтому панель не засчитывает его сама. \
                         Если оплата ещё не засчитана, сверьте платёж в кабинете Antilopay",
                        util::amount_text(back),
                        util::amount_text(amount)
                    ),
                )
            }
        }
        "FAIL" => outcome(format!("{id}:fail"), ProviderStatus::Failed, amount.ok()),
        "CANCEL" | "EXPIRED" => outcome(
            format!("{id}:{}", status.to_ascii_lowercase()),
            ProviderStatus::Canceled,
            amount.ok(),
        ),
        "REVERSED" => outcome(
            format!("reversed:{id}"),
            ProviderStatus::Refunded,
            Some(amount?),
        ),
        "CHARGEBACK" => outcome(format!("chargeback:{id}"), ProviderStatus::Refunded, None)
            .with_notice(
                NoticeKind::Chargeback,
                format!(
                    "Antilopay сообщила о чарджбэке по платежу {id}: банк покупателя отозвал оплату. \
                     Платёж отмечен возвращённым, подписка не изменена — решите, отключать ли её"
                ),
            ),
        other => util::skipped(
            format!("{id}:{}", other.to_ascii_lowercase()),
            reference,
            Some(id.to_string()),
            WebhookReply::ok(),
        ),
    })
}

fn refund_outcome(refund: &Json, payment: Option<&str>) -> PaymentResult<WebhookOutcome> {
    let id = util::safe_id(text(refund, "refund_id").ok_or_else(|| util::bad("нет refund_id"))?)?;
    let status = text(refund, "status")
        .ok_or_else(|| util::bad("нет статуса возврата"))?
        .to_ascii_uppercase();
    let event_key = format!("refund:{id}");
    let reference = payment.map(|p| PaymentRef::ProviderTxid(p.to_string()));
    let txid = payment.map(str::to_string);
    let (state, amount) = match status.as_str() {
        "COMPLETE" => {
            let refunded = rubles(refund, "amount")?;
            (
                ProviderStatus::PartiallyRefunded { refunded },
                Some(refunded),
            )
        }
        "FAIL" => (ProviderStatus::RefundDeclined, None),
        _ => {
            return Ok(util::skipped(
                event_key,
                reference,
                txid,
                WebhookReply::ok(),
            ));
        }
    };
    Ok(WebhookOutcome {
        event_key,
        payment: reference,
        provider_txid: txid,
        status: state,
        amount,
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    })
}

struct Cached {
    digest: [u8; 32],
    keys: PaymentResult<Arc<Keys>>,
}

pub struct Antilopay {
    http: sn_egress::Http,
    api_base: String,
    keys: Mutex<Option<Cached>>,
}

impl Default for Antilopay {
    fn default() -> Self {
        Self::new()
    }
}

impl Antilopay {
    pub fn new() -> Self {
        Self::with_api_base(API)
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Antilopay {
            http: http_client(providers::ANTILOPAY),
            api_base: api_base.into().trim_end_matches('/').to_string(),
            keys: Mutex::new(None),
        }
    }

    fn keys(&self, config: &ProviderConfig) -> PaymentResult<Arc<Keys>> {
        let secret = config.require(SECRET_KEY)?;
        let callback = config.require(CALLBACK_KEY)?;
        let digest: [u8; 32] = Sha256::new()
            .chain_update(secret.len().to_be_bytes())
            .chain_update(secret)
            .chain_update(callback)
            .finalize()
            .into();
        let mut cached = match self.keys.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(found) = cached.as_ref().filter(|c| c.digest == digest) {
            return found.keys.clone();
        }
        let keys = Keys::parse(secret, callback).map(Arc::new);
        *cached = Some(Cached {
            digest,
            keys: keys.clone(),
        });
        keys
    }

    async fn call(
        &self,
        config: &ProviderConfig,
        path: &str,
        body: &Json,
    ) -> PaymentResult<Answer> {
        let raw = body.to_bytes();
        let sign = self.keys(config)?.sign(&raw)?;
        let reply = util::send(
            self.http
                .post(format!("{}{path}", self.api_base))
                .header("X-Apay-Secret-Id", secret_id(config)?)
                .header("X-Apay-Sign", sign)
                .header("X-Apay-Sign-Version", SIGN_VERSION)
                .header(reqwest::header::ACCEPT, "application/json")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(raw),
        )
        .await?;
        answer(&reply)
    }

    async fn ask_payment(&self, config: &ProviderConfig, order: &str) -> PaymentResult<Answer> {
        self.call(
            config,
            "/payment/check",
            &Json::object([
                ("project_identificator", Json::from(project(config)?)),
                ("order_id", Json::from(order)),
            ]),
        )
        .await
    }

    async fn payment(&self, config: &ProviderConfig, order: &str) -> PaymentResult<Option<Json>> {
        match self.ask_payment(config, order).await? {
            Answer::Done(payment) => {
                if text(&payment, "order_id").is_some_and(|o| o != order) {
                    return Err(util::bad("Antilopay вернула другой платёж"));
                }
                Ok(Some(payment))
            }
            Answer::Refused {
                code: UNKNOWN_PAYMENT,
                ..
            } => Ok(None),
            Answer::Refused { code, message } => Err(check_refused(code, &message)),
        }
    }

    async fn refund_by(
        &self,
        config: &ProviderConfig,
        key: &'static str,
        id: &str,
    ) -> PaymentResult<Option<Json>> {
        let found = self
            .call(
                config,
                "/refund/check",
                &Json::object([
                    ("project_identificator", Json::from(project(config)?)),
                    (key, Json::from(id)),
                ]),
            )
            .await?;
        match found {
            Answer::Done(refund) => {
                if text(&refund, key).is_some_and(|got| got != id) {
                    return Err(util::bad("Antilopay вернула другой возврат"));
                }
                Ok(Some(refund))
            }
            Answer::Refused {
                code: UNKNOWN_REFUND,
                ..
            } => Ok(None),
            Answer::Refused { code, message } => Err(refund_refused(code, &message)),
        }
    }

    async fn earlier_refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
        payment: &str,
    ) -> PaymentResult<Option<String>> {
        let Some(found) = self
            .refund_by(config, "order_id", &request.key.to_string())
            .await?
        else {
            return Ok(None);
        };
        if text(&found, "payment_id").is_some_and(|p| p != payment)
            || rubles(&found, "amount")? != request.amount
        {
            return Err(PaymentError::Rejected(
                "возврат с таким номером в Antilopay относится к другому платежу или сумме".into(),
            ));
        }
        refund_number(&found).map(Some)
    }

    async fn payment_notice(
        &self,
        config: &ProviderConfig,
        body: &Json,
    ) -> PaymentResult<WebhookOutcome> {
        let id =
            util::safe_id(text(body, "payment_id").ok_or_else(|| util::bad("нет payment_id"))?)?;
        let order = text(body, "order_id")
            .filter(|o| o.chars().count() <= ORDER_MAX)
            .ok_or_else(|| util::bad("нет order_id"))?;
        let payment = self
            .payment(config, order)
            .await?
            .ok_or_else(|| util::bad("Antilopay не знает платёж из уведомления"))?;
        if text(&payment, "payment_id") != Some(id) {
            return Err(util::bad(
                "номер платежа в уведомлении и в Antilopay не совпал",
            ));
        }
        let outcome = payment_outcome(&payment)?;
        let claimed = text(body, "status").is_some_and(|s| s.eq_ignore_ascii_case("SUCCESS"));
        if claimed && outcome.status == ProviderStatus::Pending && outcome.signal.is_none() {
            return Err(unconfirmed("оплату", &payment));
        }
        Ok(outcome)
    }

    async fn refund_notice(
        &self,
        config: &ProviderConfig,
        body: &Json,
    ) -> PaymentResult<WebhookOutcome> {
        let id = util::safe_id(text(body, "refund_id").ok_or_else(|| util::bad("нет refund_id"))?)?;
        let refund = self
            .refund_by(config, "refund_id", id)
            .await?
            .ok_or_else(|| util::bad("Antilopay не знает возврат из уведомления"))?;
        let claimed = text(body, "payment_id");
        let payment = text(&refund, "payment_id")
            .or(claimed)
            .ok_or_else(|| util::bad("нет payment_id"))?;
        if claimed.is_some_and(|p| p != payment) {
            return Err(util::bad(
                "номер платежа в уведомлении о возврате и в Antilopay не совпал",
            ));
        }
        let outcome = refund_outcome(&refund, Some(util::safe_id(payment)?))?;
        let complete = text(body, "status").is_some_and(|s| s.eq_ignore_ascii_case("COMPLETE"));
        if complete && outcome.status == ProviderStatus::Pending {
            return Err(unconfirmed("возврат", &refund));
        }
        Ok(outcome)
    }
}

fn unconfirmed(what: &str, object: &Json) -> PaymentError {
    PaymentError::Rejected(format!(
        "Antilopay не подтвердила {what}: статус «{}»",
        util::clip(text(object, "status").unwrap_or_default(), 32)
    ))
}

fn refund_number(refund: &Json) -> PaymentResult<String> {
    util::safe_id(
        text(refund, "refund_id")
            .ok_or_else(|| util::bad("Antilopay не вернула номер возврата"))?,
    )
    .map(str::to_string)
}

#[async_trait]
impl PaymentProvider for Antilopay {
    fn api_url(&self, _config: &ProviderConfig) -> Option<String> {
        Some(self.api_base.clone())
    }

    fn id(&self) -> &'static str {
        providers::ANTILOPAY
    }

    fn title(&self) -> &'static str {
        "Antilopay"
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
            SettingField::text(
                PROJECT_ID,
                "ID проекта",
                "Идентификатор проекта из кабинета Antilopay, начинается с P. Проект должен пройти модерацию",
            ),
            SettingField::secret(
                SECRET_ID,
                "Secret ID",
                "Идентификатор мерчанта. Antilopay присылает его вместе с секретным ключом после подтверждения аккаунта",
            ),
            SettingField::secret(
                SECRET_KEY,
                "Секретный ключ",
                "Secret Key для платежей, начинается с MII. Секретный ключ для выплат не подходит. Можно вставить одной строкой или вместе со строками BEGIN и END",
            )
            .large(),
            SettingField::secret(
                CALLBACK_KEY,
                "Публичный ключ проекта",
                "Ключ для проверки уведомлений (Callback). Antilopay присылает его на каждый подтверждённый проект. Без него панель не примет ни одного уведомления",
            )
            .large(),
            SettingField::select(
                VAT,
                "Ставка НДС",
                "Нужна, только если вы на общей системе налогообложения (ОСНО), иначе оставьте «Не передавать»",
                &VAT_RATES,
            )
            .labels(&VAT_LABELS)
            .with_default(VAT_RATES[0]),
        ]
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        project(config)?;
        secret_id(config)?;
        vat(config)?;
        self.keys(config).map(|_| ())
    }

    fn accepts_http_webhooks(&self) -> bool {
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

    fn status_by_order(&self) -> bool {
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
        let mut body = Json::object([
            ("project_identificator", Json::from(project(config)?)),
            ("amount", Json::number(request.amount.units())),
            ("order_id", Json::from(order)),
            ("currency", Json::from(PayCurrency::Rub.code())),
            (
                "product_name",
                Json::from(util::clip(&request.description, PRODUCT_MAX)),
            ),
            ("product_type", Json::from(PRODUCT_TYPE)),
            (
                "description",
                Json::from(util::clip(&request.description, DESCRIPTION_MAX)),
            ),
            (
                "customer",
                Json::object([("email", Json::from(buyer_email(request)))]),
            ),
        ]);
        if let Some(rate) = vat(config)? {
            body.push("vat", Json::number(rate));
        }
        if let Some(url) = back_url(request) {
            body.push("success_url", Json::from(url.clone()));
            body.push("fail_url", Json::from(url));
            body.push(
                "params",
                Json::object([("skip_success_page", Json::from(true))]),
            );
        }
        if let Some(code) = wire {
            body.push("prefer_methods", Json::Array(vec![Json::from(code)]));
        }
        let created = match self.call(config, "/payment/create", &body).await? {
            Answer::Done(created) => created,
            Answer::Refused { code, message } => return Err(create_refused(code, &message)),
        };
        let id = util::safe_id(
            text(&created, "payment_id")
                .ok_or_else(|| util::bad("Antilopay не вернула номер платежа"))?,
        )?;
        let pay_url = text(&created, "payment_url")
            .filter(|url| util::is_https(url))
            .ok_or_else(|| util::bad("Antilopay не вернула ссылку на оплату"))?;
        Ok(Invoice {
            pay_url: pay_url.to_string(),
            external_id: Some(id.to_string()),
            expires_at: None,
            payload: serde_json::json!({
                "payment_id": id,
                "method": wire,
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let keys = self.keys(config)?;
        if request
            .route_suffix
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(util::bad(
                "у Antilopay один адрес уведомлений, без дополнительной части",
            ));
        }
        let Some(signed) = request.header("x-apay-callback") else {
            return Err(PaymentError::BadSignature);
        };
        if request
            .header("x-apay-callback-version")
            .is_some_and(|v| v.trim() != SIGN_VERSION)
        {
            return Err(util::bad(
                "уведомление подписано способом, который панель не знает: в настройках проекта Antilopay выберите SHA256WithRSA",
            ));
        }
        if !keys.genuine(&request.body, signed) {
            return Err(PaymentError::BadSignature);
        }
        let body = Json::parse(&request.body).ok_or_else(|| util::bad("тело не JSON"))?;
        match text(&body, "type") {
            None | Some("payment") => self.payment_notice(config, &body).await,
            Some("refund") => self.refund_notice(config, &body).await,
            Some(other) => Ok(util::skipped(
                util::clip(other, 64),
                None,
                None,
                WebhookReply::ok(),
            )),
        }
    }

    async fn fetch_status(
        &self,
        config: &ProviderConfig,
        external_id: &str,
    ) -> PaymentResult<Option<WebhookOutcome>> {
        let Ok(order) = Uuid::parse_str(external_id.trim()) else {
            return Ok(None);
        };
        match self.payment(config, &order.to_string()).await? {
            Some(payment) => payment_outcome(&payment).map(Some),
            None => Err(PaymentError::Rejected(
                "платёж не найден в Antilopay".into(),
            )),
        }
    }

    async fn refund(
        &self,
        config: &ProviderConfig,
        request: &RefundRequest,
    ) -> PaymentResult<RefundOutcome> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let payment = util::safe_id(request.provider_txid.trim())
            .map_err(|_| PaymentError::Rejected("у платежа нет номера в Antilopay".into()))?;
        let asked = self
            .call(
                config,
                "/refund/create",
                &Json::object([
                    ("project_identificator", Json::from(project(config)?)),
                    ("transaction_id", Json::from(payment)),
                    ("order_id", Json::from(request.key.to_string())),
                    ("amount", Json::number(request.amount.units())),
                ]),
            )
            .await;
        let id = match asked {
            Ok(Answer::Done(created)) => refund_number(&created)?,
            Ok(Answer::Refused { code, message }) if code == REPEATED_REFUND => self
                .earlier_refund(config, request, payment)
                .await?
                .ok_or_else(|| refund_refused(code, &message))?,
            Ok(Answer::Refused { code, message }) => return Err(refund_refused(code, &message)),
            Err(err) if err.is_retryable() => {
                match self.earlier_refund(config, request, payment).await {
                    Ok(Some(id)) => id,
                    _ => return Err(err),
                }
            }
            Err(err) => return Err(err),
        };
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
        let Ok(id) = util::safe_id(provider_refund_id.trim()) else {
            return Ok(None);
        };
        let refund = self
            .refund_by(config, "refund_id", id)
            .await?
            .ok_or_else(|| PaymentError::Rejected("возврат не найден в Antilopay".into()))?;
        refund_outcome(&refund, text(&refund, "payment_id")).map(Some)
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let mut check = match self.ask_payment(config, &Uuid::nil().to_string()).await {
            Ok(Answer::Done(_))
            | Ok(Answer::Refused {
                code: UNKNOWN_PAYMENT,
                ..
            }) => ConnectionCheck::ok("ID проекта, Secret ID и секретный ключ приняты Antilopay"),
            Ok(Answer::Refused { code: 1 | 2, .. }) => {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "Antilopay не приняла Secret ID",
                )
                .detail(
                    "Secret ID приходит вместе с секретным ключом после подтверждения аккаунта",
                );
            }
            Ok(Answer::Refused { code: 3, .. }) => {
                return ConnectionCheck::new(
                    CheckStatus::KeysRejected,
                    "Antilopay не приняла подпись: секретный ключ не подходит к Secret ID",
                )
                .detail("Нужен ключ для платежей. Секретный ключ для выплат здесь не работает");
            }
            Ok(Answer::Refused {
                code: UNKNOWN_PROJECT,
                ..
            }) => {
                return ConnectionCheck::new(
                    CheckStatus::SettingsRejected,
                    "Antilopay не нашла проект с таким ID",
                )
                .detail("Скопируйте ID проекта из кабинета Antilopay ещё раз");
            }
            Ok(Answer::Refused {
                code: PAYMENT_IP, ..
            }) => {
                return ConnectionCheck::new(
                    CheckStatus::SettingsRejected,
                    "Antilopay не принимает запросы с IP-адреса сервера панели",
                )
                .detail("Разрешите адрес сервера в кабинете Antilopay или через её поддержку. Если платёжка идёт через исходящий прокси, нужен адрес прокси");
            }
            Ok(Answer::Refused { code, message }) => {
                return ConnectionCheck::failed(&check_refused(code, &message));
            }
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let balance = match project(config) {
            Ok(id) => {
                self.call(
                    config,
                    "/project/balance",
                    &Json::object([("project_identificator", Json::from(id))]),
                )
                .await
            }
            Err(err) => Err(err),
        };
        match balance {
            Ok(Answer::Refused {
                code: UNCONFIRMED_PROJECT,
                ..
            }) => {
                return ConnectionCheck::new(
                    CheckStatus::SettingsRejected,
                    "Проект Antilopay ещё не подтверждён: счета создаваться не будут",
                )
                .detail("Ключи приняты. Дождитесь, пока Antilopay проверит проект, и повторите");
            }
            Ok(Answer::Done(found)) => {
                let available = found
                    .get("rub")
                    .and_then(|balance| rubles(balance, "available").ok());
                if let Some(amount) = available {
                    check = check.detail(format!(
                        "Доступно на балансе проекта: {}",
                        util::amount_text(amount)
                    ));
                }
            }
            _ => {}
        }
        check = match context.webhook_url.as_deref() {
            Some(url) => check.detail(format!(
                "В настройках проекта Antilopay укажите адрес для Callback {url}. Antilopay не даёт прочитать его по API, поэтому сверьте его сами"
            )),
            None => check.detail(
                "Адрес панели не задан в основных настройках: Antilopay некуда присылать уведомления",
            ),
        };
        check = check
            .detail(format!(
                "Уведомления приходят с адресов {CALLBACK_IPS}: если перед панелью стоит фильтр, пропустите их"
            ))
            .detail("Публичный ключ проекта этим запросом не проверить: он проверяется на уведомлениях. Чтобы проверить его, оплатите небольшой счёт");
        check.scrub(&[
            config.get(SECRET_ID).unwrap_or_default(),
            config.get(SECRET_KEY).unwrap_or_default(),
            config.get(CALLBACK_KEY).unwrap_or_default(),
        ]);
        check
    }
}

#[cfg(test)]
pub(crate) mod tests;
