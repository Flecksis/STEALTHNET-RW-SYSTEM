use crate::error::{PaymentError, PaymentResult};
use crate::json::Json;
use crate::provider::*;
use crate::util;
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use sn_domain::money::{Amount, Currency, Money, PayCurrency, parse_units};
use sn_domain::payment::{NoticeKind, providers};
use sn_domain::settings::SettingField;
use std::collections::HashSet;

const API: &str = "https://tribute.tg/api/v1";
const API_KEY: &str = "api_key";
pub const PRODUCTS: &str = "products";
pub const PLANS: &str = "plans";
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 10;
const MAX_ITEMS: usize = 200;

pub fn signature(api_key: &str, body: &[u8]) -> String {
    Hmac::<Sha256>::new_from_slice(api_key.as_bytes())
        .map(|mut mac| {
            mac.update(body);
            hex::encode(mac.finalize().into_bytes())
        })
        .unwrap_or_default()
}

fn signed(api_key: &str, request: &WebhookRequest) -> bool {
    let Some(received) = request.header("trbt-signature") else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(api_key.as_bytes()) else {
        return false;
    };
    mac.update(&request.body);
    util::hex_digest_eq(&mac.finalize().into_bytes(), received)
}

fn money_currency(raw: &str) -> Option<Currency> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "USD" => Some(Currency::Usd),
        "RUB" => Some(Currency::Rub),
        _ => None,
    }
}

fn integer(value: Option<&Json>) -> Option<i64> {
    value
        .and_then(Json::scalar_text)
        .and_then(|raw| raw.trim().parse::<i64>().ok())
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ProductEntry {
    id: i64,
    amount: i64,
    currency: String,
    link: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Product {
    id: i64,
    amount: Money,
    link: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct PlanEntry {
    period: i64,
    tariff: i64,
    tariff_period: i64,
}

impl PlanEntry {
    fn complete(&self) -> bool {
        self.period > 0 && self.tariff > 0 && self.tariff_period > 0
    }
}

fn entries<T: for<'de> Deserialize<'de>>(config: &ProviderConfig, key: &str) -> Vec<T> {
    match config.0.get(key) {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|item| serde_json::from_value(item.clone()).ok())
            .collect(),
        _ => Vec::new(),
    }
}

fn product(entry: ProductEntry) -> Option<Product> {
    let currency = money_currency(&entry.currency)?;
    (entry.id > 0 && entry.amount > 0 && util::is_https(&entry.link)).then(|| Product {
        id: entry.id,
        amount: Money::new(entry.amount, currency),
        link: entry.link,
    })
}

fn products(config: &ProviderConfig) -> Vec<Product> {
    entries::<ProductEntry>(config, PRODUCTS)
        .into_iter()
        .filter_map(product)
        .collect()
}

fn plans(config: &ProviderConfig) -> Vec<PlanEntry> {
    entries::<PlanEntry>(config, PLANS)
        .into_iter()
        .filter(PlanEntry::complete)
        .collect()
}

fn checked_list(config: &ProviderConfig, key: &str) -> PaymentResult<Vec<serde_json::Value>> {
    match config.0.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) if items.len() <= MAX_ITEMS => Ok(items.clone()),
        Some(serde_json::Value::Array(_)) => Err(PaymentError::NotConfigured(format!(
            "в списке {key} больше {MAX_ITEMS} строк"
        ))),
        Some(_) => Err(PaymentError::NotConfigured(format!(
            "поле {key} должно быть списком"
        ))),
    }
}

fn validate_lists(config: &ProviderConfig) -> PaymentResult<()> {
    let mut seen = HashSet::new();
    for item in checked_list(config, PRODUCTS)? {
        let entry = serde_json::from_value::<ProductEntry>(item)
            .ok()
            .and_then(product)
            .ok_or_else(|| {
                PaymentError::NotConfigured(
                    "товар Tribute записан неполностью: обновите список товаров".into(),
                )
            })?;
        if !seen.insert(entry.id) {
            return Err(PaymentError::NotConfigured(format!(
                "товар Tribute №{} выбран дважды",
                entry.id
            )));
        }
    }
    let mut periods = HashSet::new();
    for item in checked_list(config, PLANS)? {
        let entry = serde_json::from_value::<PlanEntry>(item)
            .ok()
            .filter(PlanEntry::complete)
            .ok_or_else(|| {
                PaymentError::NotConfigured(
                    "у привязки подписки Tribute не выбран тариф или срок".into(),
                )
            })?;
        if !periods.insert(entry.period) {
            return Err(PaymentError::NotConfigured(format!(
                "срок подписки Tribute №{} привязан дважды",
                entry.period
            )));
        }
    }
    Ok(())
}

fn ok_reply() -> WebhookReply {
    WebhookReply::Json {
        status: 200,
        body: serde_json::json!({"status": "ok"}),
    }
}

fn paid(payload: &Json, key: &str) -> Option<(i64, String)> {
    let minor = integer(payload.get(key)).filter(|minor| *minor >= 0)?;
    let currency = payload.get("currency").and_then(Json::as_str)?.trim();
    Some((minor, currency.to_ascii_uppercase()))
}

fn amount_text(minor: i64, currency: &str) -> String {
    match money_currency(currency) {
        Some(known) => util::amount_text(Money::new(minor, known).into()),
        None => format!(
            "{}.{:02} {}",
            minor / 100,
            minor % 100,
            util::clip(currency, 8)
        ),
    }
}

fn text(payload: &Json, key: &str, max: usize) -> String {
    util::clip(
        payload
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .trim(),
        max,
    )
}

fn unmatched(base: WebhookOutcome, what: &str, buyer: Option<i64>, sum: &str) -> WebhookOutcome {
    let text = match buyer {
        None => format!(
            "Tribute прислал оплату ({what}) на {sum} без Telegram ID покупателя: клиента не найти. \
             Найдите покупателя в кабинете Tribute и зачислите деньги вручную"
        ),
        Some(telegram_id) => format!(
            "Tribute прислал оплату ({what}) на {sum} от Telegram ID {telegram_id} в валюте, которой \
             нет в панели. Зачислите деньги вручную"
        ),
    };
    base.with_notice(NoticeKind::ManualCredit, text)
}

fn purchase(payload: &Json) -> PaymentResult<WebhookOutcome> {
    let purchase = integer(payload.get("purchase_id"))
        .filter(|id| *id > 0)
        .ok_or_else(|| util::bad("нет номера покупки purchase_id"))?;
    let product = integer(payload.get("product_id"))
        .filter(|id| *id > 0)
        .ok_or_else(|| util::bad("нет номера товара product_id"))?;
    let (minor, currency) = paid(payload, "amount")
        .filter(|(minor, _)| *minor > 0)
        .ok_or_else(|| util::bad("нет суммы или валюты покупки"))?;
    let txid = format!("purchase:{purchase}");
    let buyer = integer(payload.get("telegram_user_id")).filter(|id| *id > 0);
    let known = money_currency(&currency);
    let base = WebhookOutcome {
        event_key: txid.clone(),
        payment: None,
        provider_txid: Some(txid.clone()),
        status: ProviderStatus::Pending,
        amount: known.map(|currency| Money::new(minor, currency).into()),
        saved_method: None,
        reply: ok_reply(),
        signal: None,
    };
    let what = format!(
        "товар «{}» №{product}, покупка №{purchase}",
        text(payload, "product_name", 80)
    );
    match (buyer, known) {
        (Some(telegram_id), Some(_)) => Ok(WebhookOutcome {
            payment: Some(PaymentRef::Buyer(Buyer {
                telegram_id,
                txid,
                purpose: BuyerPurpose::TopUp,
            })),
            status: ProviderStatus::Paid,
            ..base
        }),
        _ => Ok(unmatched(
            base,
            &what,
            buyer,
            &amount_text(minor, &currency),
        )),
    }
}

fn subscription(payload: &Json, renewed: bool) -> PaymentResult<WebhookOutcome> {
    let subscription = integer(payload.get("subscription_id"))
        .filter(|id| *id > 0)
        .ok_or_else(|| util::bad("нет номера подписки subscription_id"))?;
    let period = integer(payload.get("period_id"))
        .filter(|id| *id > 0)
        .ok_or_else(|| util::bad("нет номера срока подписки period_id"))?;
    let (minor, currency) = paid(payload, "price")
        .filter(|(minor, _)| *minor > 0)
        .or_else(|| paid(payload, "amount"))
        .ok_or_else(|| util::bad("нет цены или валюты подписки"))?;
    let expires = payload
        .get("expires_at")
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|at| !at.is_empty() && at.len() <= 64)
        .ok_or_else(|| util::bad("нет срока подписки expires_at"))?;
    let buyer = integer(payload.get("telegram_user_id")).filter(|id| *id > 0);
    let known = money_currency(&currency);
    let series = format!("sub:{subscription}:{}:", buyer.unwrap_or_default());
    let txid = format!("{series}{expires}");
    let base = WebhookOutcome {
        event_key: txid.clone(),
        payment: None,
        provider_txid: Some(txid.clone()),
        status: ProviderStatus::Pending,
        amount: known.map(|currency| Money::new(minor, currency).into()),
        saved_method: None,
        reply: ok_reply(),
        signal: None,
    };
    let what = format!(
        "{} подписки «{}» №{subscription}, срок №{period}",
        if renewed {
            "продление"
        } else {
            "новая оплата"
        },
        text(payload, "subscription_name", 80)
    );
    match (buyer, known) {
        (Some(telegram_id), Some(_)) => Ok(WebhookOutcome {
            payment: Some(PaymentRef::Buyer(Buyer {
                telegram_id,
                txid,
                purpose: BuyerPurpose::Plan {
                    plan: period.to_string(),
                    series,
                },
            })),
            status: ProviderStatus::Paid,
            ..base
        }),
        _ => Ok(unmatched(
            base,
            &what,
            buyer,
            &amount_text(minor, &currency),
        )),
    }
}

fn refunded(payload: &Json) -> PaymentResult<WebhookOutcome> {
    let purchase = integer(payload.get("purchase_id"))
        .filter(|id| *id > 0)
        .ok_or_else(|| util::bad("нет номера покупки purchase_id"))?;
    let amount = paid(payload, "amount").and_then(|(minor, currency)| {
        money_currency(&currency).map(|currency| Amount::from(Money::new(minor, currency)))
    });
    let txid = format!("purchase:{purchase}");
    Ok(WebhookOutcome {
        event_key: format!("refund:{purchase}"),
        payment: Some(PaymentRef::ProviderTxid(txid.clone())),
        provider_txid: Some(txid),
        status: ProviderStatus::Refunded,
        amount,
        saved_method: None,
        reply: ok_reply(),
        signal: None,
    })
}

fn event(raw: &[u8]) -> PaymentResult<WebhookOutcome> {
    let body = Json::parse(raw).ok_or_else(|| util::bad("тело не JSON"))?;
    let name = body
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .trim();
    match (name, body.get("payload")) {
        ("new_digital_product", Some(payload)) => purchase(payload),
        ("digital_product_refunded", Some(payload)) => refunded(payload),
        ("new_subscription", Some(payload)) => subscription(payload, false),
        ("renewed_subscription", Some(payload)) => subscription(payload, true),
        _ => {
            let at = body
                .get("created_at")
                .and_then(Json::as_str)
                .unwrap_or_default();
            Ok(util::skipped(
                format!("{}:{}", util::clip(name, 40), util::clip(at, 40)),
                None,
                None,
                ok_reply(),
            ))
        }
    }
}

fn catalog_product(item: &Json) -> Option<serde_json::Value> {
    if item.get("type").and_then(Json::as_str) != Some("digital")
        || item.get("status").and_then(Json::as_str) != Some("approved")
        || item.get("isCustom").and_then(Json::as_bool) == Some(true)
    {
        return None;
    }
    let currency = money_currency(item.get("currency").and_then(Json::as_str)?)?;
    let minor = integer(item.get("amount")).filter(|minor| *minor > 0)?;
    let link = item
        .get("link")
        .and_then(Json::as_str)
        .filter(|link| util::is_https(link))?;
    Some(serde_json::json!({
        "id": integer(item.get("id")).filter(|id| *id > 0)?,
        "name": util::clip(item.get("name").and_then(Json::as_str).unwrap_or_default(), 120),
        "amount": minor,
        "currency": currency.code(),
        "link": link,
    }))
}

fn catalog_subscription(item: &Json) -> Option<serde_json::Value> {
    let id = integer(item.get("subscriptionId")).filter(|id| *id > 0)?;
    let currency = item
        .get("currency")
        .and_then(Json::as_str)
        .map(|c| c.trim().to_ascii_uppercase())
        .unwrap_or_default();
    let known = money_currency(&currency);
    let periods: Vec<serde_json::Value> = item
        .get("periods")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|period| {
            let period_id = integer(period.get("periodId")).filter(|id| *id > 0)?;
            let price = known.and_then(|currency| {
                parse_units(
                    period.get("price").and_then(Json::scalar_text)?,
                    PayCurrency::from(currency),
                )
            });
            Some(serde_json::json!({
                "id": period_id,
                "period": util::clip(period.get("period").and_then(Json::as_str).unwrap_or_default(), 24),
                "price": price,
            }))
        })
        .collect();
    Some(serde_json::json!({
        "id": id,
        "name": util::clip(item.get("name").and_then(Json::as_str).unwrap_or_default(), 120),
        "currency": currency,
        "periods": periods,
    }))
}

fn failure(reply: &util::Reply) -> PaymentError {
    let message = Json::parse(&reply.body).and_then(|parsed| {
        parsed
            .get("message")
            .and_then(Json::as_str)
            .map(str::to_string)
    });
    reply.failure(message.as_deref())
}

pub struct Tribute {
    http: reqwest::Client,
    api_base: Option<String>,
}

impl Default for Tribute {
    fn default() -> Self {
        Self::new()
    }
}

impl Tribute {
    pub fn new() -> Self {
        Tribute {
            http: http_client(),
            api_base: None,
        }
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Tribute {
            http: http_client(),
            api_base: Some(api_base.into()),
        }
    }

    fn base(&self) -> String {
        self.api_base
            .as_deref()
            .unwrap_or(API)
            .trim_end_matches('/')
            .to_string()
    }

    async fn get(&self, key: &str, path: &str, query: &[(&str, String)]) -> PaymentResult<Json> {
        let reply = util::send(
            self.http
                .get(format!("{}{path}", self.base()))
                .query(query)
                .header("Api-Key", key),
        )
        .await?;
        if !reply.is_success() {
            return Err(failure(&reply));
        }
        reply.json()
    }

    async fn sold(&self, key: &str) -> PaymentResult<Vec<serde_json::Value>> {
        let mut found = Vec::new();
        for page in 1..=MAX_PAGES {
            let parsed = self
                .get(
                    key,
                    "/products",
                    &[
                        ("type", "digital".to_string()),
                        ("page", page.to_string()),
                        ("size", PAGE_SIZE.to_string()),
                    ],
                )
                .await?;
            let rows = parsed
                .get("rows")
                .and_then(Json::as_array)
                .ok_or_else(|| util::bad("в ответе Tribute нет списка товаров"))?;
            found.extend(rows.iter().filter_map(catalog_product));
            if rows.len() < PAGE_SIZE {
                break;
            }
        }
        Ok(found)
    }

    async fn subscriptions(&self, key: &str) -> PaymentResult<Vec<serde_json::Value>> {
        let parsed = self.get(key, "/subscriptions", &[]).await?;
        Ok(parsed
            .get("result")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(catalog_subscription)
            .collect())
    }
}

fn currency_name(currency: Currency) -> &'static str {
    match currency {
        Currency::Rub => "рублях",
        Currency::Usd => "долларах",
    }
}

#[async_trait]
impl PaymentProvider for Tribute {
    fn id(&self) -> &'static str {
        providers::TRIBUTE
    }

    fn title(&self) -> &'static str {
        "Tribute"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Usd, Currency::Rub]
    }

    fn client_label(&self, _currency: Currency, _config: &ProviderConfig) -> ClientLabel {
        ClientLabel {
            title: "Tribute",
            hint: "Оплата в Telegram",
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::secret(
                API_KEY,
                "API-ключ",
                "Tribute → кабинет автора → ⋯ → «Настройки» → «API-ключи» → «Сгенерировать ключ». Этим же ключом Tribute подписывает уведомления",
            ),
            SettingField::list(
                PRODUCTS,
                "Товары для пополнения",
                "Цифровые товары Tribute, которыми клиент пополняет баланс",
            ),
            SettingField::list(
                PLANS,
                "Подписки Tribute",
                "Срок подписки Tribute и тариф с периодом, которые он выдаёт",
            ),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        validate_lists(config)
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    fn needs_telegram(&self) -> bool {
        true
    }

    fn fixed_amounts(&self, config: &ProviderConfig, currency: Currency) -> Option<Vec<Money>> {
        let mut amounts: Vec<Money> = products(config)
            .into_iter()
            .map(|product| product.amount)
            .filter(|amount| amount.currency == currency)
            .collect();
        amounts.sort_by_key(|amount| amount.minor);
        amounts.dedup();
        Some(amounts)
    }

    fn plan_binding(&self, config: &ProviderConfig, plan: &str) -> Option<PlanBinding> {
        let period = plan.trim().parse::<i64>().ok()?;
        plans(config)
            .into_iter()
            .find(|entry| entry.period == period)
            .map(|entry| PlanBinding {
                tariff_id: entry.tariff,
                period_id: entry.tariff_period,
            })
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        let wanted = request.amount.as_money().ok_or_else(|| {
            PaymentError::UnsupportedCurrency(request.amount.currency.code().into())
        })?;
        if request.client.telegram_id.is_none() {
            return Err(PaymentError::Rejected(
                "Tribute доступен только клиентам с привязанным Telegram".into(),
            ));
        }
        let product = products(config)
            .into_iter()
            .filter(|product| {
                product.amount.currency == wanted.currency && product.amount.minor >= wanted.minor
            })
            .min_by_key(|product| product.amount.minor)
            .ok_or_else(|| PaymentError::Rejected("в Tribute нет товара на эту сумму".into()))?;
        Ok(Invoice {
            pay_url: product.link,
            external_id: None,
            expires_at: None,
            payload: serde_json::json!({
                "product": product.id,
                "charge": product.amount.minor,
            }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let key = config.require(API_KEY)?;
        if !signed(key, request) {
            return Err(PaymentError::BadSignature);
        }
        event(&request.body)
    }

    async fn catalog(&self, config: &ProviderConfig) -> PaymentResult<Option<serde_json::Value>> {
        let key = config.require(API_KEY)?;
        let products = self.sold(key).await?;
        let subscriptions = self.subscriptions(key).await?;
        Ok(Some(serde_json::json!({
            "products": products,
            "subscriptions": subscriptions,
        })))
    }

    async fn check_connection(
        &self,
        config: &ProviderConfig,
        context: &CheckContext,
    ) -> ConnectionCheck {
        let key = match config.require(API_KEY) {
            Ok(key) => key,
            Err(err) => return ConnectionCheck::failed(&err),
        };
        let available = match self.sold(key).await {
            Ok(found) => found,
            Err(err) => {
                let mut check = if util::keys_refused(&err) {
                    ConnectionCheck::new(CheckStatus::KeysRejected, "Tribute не принял API-ключ")
                        .detail("Ключ создаётся в Tribute: «Настройки» → «API-ключи»")
                } else {
                    ConnectionCheck::failed(&err)
                };
                check.scrub(&[key]);
                return check;
            }
        };
        let mut check = ConnectionCheck::ok("API-ключ принят Tribute");
        let ids: HashSet<i64> = available
            .iter()
            .filter_map(|item| item.get("id").and_then(serde_json::Value::as_i64))
            .collect();
        let chosen = products(config);
        check = if chosen.is_empty() {
            check.detail(
                "Товары для пополнения не выбраны: без них кнопки Tribute у клиентов не будет",
            )
        } else {
            check.detail(format!("Товаров для пополнения: {}", chosen.len()))
        };
        let gone: Vec<String> = chosen
            .iter()
            .filter(|product| !ids.contains(&product.id))
            .map(|product| format!("№{}", product.id))
            .collect();
        if !gone.is_empty() {
            check = check.detail(format!(
                "Tribute больше не продаёт товары {}: обновите список товаров",
                gone.join(", ")
            ));
        }
        let mut currencies: Vec<Currency> = chosen.iter().map(|p| p.amount.currency).collect();
        currencies.sort_by_key(|currency| currency.code());
        currencies.dedup();
        if currencies.len() > 1 {
            check = check.detail(format!(
                "Выбраны товары в {}: клиенту показываются только товары в валюте панели",
                currencies
                    .iter()
                    .map(|currency| currency_name(*currency))
                    .collect::<Vec<_>>()
                    .join(" и ")
            ));
        }
        check = match self.subscriptions(key).await {
            Ok(subscriptions) => check.detail(format!(
                "Подписок в Tribute: {}, привязано сроков: {}",
                subscriptions.len(),
                plans(config).len()
            )),
            Err(err) => check.detail(format!("Список подписок Tribute не прочитан: {err}")),
        };
        check = match context.webhook_url.as_deref() {
            Some(url) => check.detail(format!(
                "Адрес уведомлений {url} укажите в Tribute: «Настройки» → «API-ключи» → Webhook URL"
            )),
            None => check.detail(
                "Адрес панели не задан в основных настройках: Tribute некуда присылать уведомления",
            ),
        };
        check.scrub(&[key]);
        check
    }
}

#[cfg(test)]
mod tests;
