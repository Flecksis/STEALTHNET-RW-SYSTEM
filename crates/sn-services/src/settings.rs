use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::{EnvFallbacks, RemnaConnection, Services};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sn_domain::ids::AdminId;
use sn_domain::module::ModuleId;
use sn_domain::money::Currency;
use sn_domain::payment::providers;
use sn_domain::settings::{
    AutoRenewSettings, CoreSettings, FieldKind, PaymentOrder, SettingField, TopUpSettings,
    TrialSettings, keys, secret_keys,
};
use sn_modules::{ModuleConfig, ModuleView};
use sn_payments::providers::STARS_TOKEN_FIELD;
use sn_payments::{
    ProviderConfig, ProviderKind, ProviderRegistry, ProviderState, ProviderView, RegistrySnapshot,
};
use sn_remnawave::{
    CustomRemarks, HwidSettings, RemnaConfig, RemnawaveApi, RemnawaveClient, SubscriptionSettings,
    UpdateSubscriptionSettingsRequest,
};
use sn_store::{Db, ModuleStateRow, SecretBox};
use std::collections::HashMap;
use std::sync::Arc;

pub const SETTINGS_CHANNEL: &str = "settings_changed";
pub const REMNA_WEBHOOK_PATH: &str = "/api/remnawave/webhook";
pub const MAX_TEXT_FIELD: usize = 2_000;
pub const MAX_REMARK_CHARS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderSettings {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub config: serde_json::Map<String, Value>,
}

fn parse<T: serde::de::DeserializeOwned>(key: &str, value: Value) -> Option<T> {
    match serde_json::from_value(value) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            tracing::warn!(key, error = %err, "настройка не разобрана, взято значение по умолчанию");
            None
        }
    }
}

pub async fn load_core(db: &mut impl Db) -> ServiceResult<CoreSettings> {
    let rows = db.settings().with_prefix("").await?;
    let mut settings = CoreSettings::default();
    let mut topup: Option<TopUpSettings> = None;
    for (key, value) in rows {
        match key.as_str() {
            keys::CURRENCY => {
                if let Some(currency) = parse::<Currency>(&key, value) {
                    settings.currency = currency;
                }
            }
            keys::BRAND_NAME => {
                if let Some(name) = parse::<String>(&key, value) {
                    settings.brand_name = name;
                }
            }
            keys::PUBLIC_URL => settings.public_url = parse(&key, value),
            keys::SUPPORT_URL => settings.support_url = parse(&key, value),
            keys::REMNA_UTC_OFFSET => {
                if let Some(minutes) = parse::<i32>(&key, value) {
                    settings.remna_utc_offset_minutes = minutes;
                }
            }
            keys::AUTORENEW => {
                if let Some(v) = parse::<AutoRenewSettings>(&key, value) {
                    settings.autorenew = v;
                }
            }
            keys::TRIAL => {
                if let Some(v) = parse::<TrialSettings>(&key, value) {
                    settings.trial = v;
                }
            }
            keys::TOPUP => topup = parse(&key, value),
            keys::PAYMENT_ORDER => {
                if let Some(v) = parse::<PaymentOrder>(&key, value) {
                    settings.payment_order = v;
                }
            }
            keys::INVOICE_TTL => {
                if let Some(v) = parse::<u32>(&key, value) {
                    settings.invoice_ttl_minutes = v.clamp(5, 24 * 60);
                }
            }
            _ => {}
        }
    }
    settings.topup = topup.unwrap_or_else(|| TopUpSettings::default_for(settings.currency));
    Ok(settings)
}

pub async fn read_secret(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    key: &str,
) -> ServiceResult<Option<String>> {
    let Some(sealed) = db.secrets().get(key).await? else {
        return Ok(None);
    };
    let Some(secrets) = secrets else {
        tracing::warn!(key, "секрет есть в базе, но SN_SECRET_KEY не задан");
        return Ok(None);
    };
    match secrets.open_string(key, &sealed) {
        Ok(value) => Ok(Some(value)),
        Err(err) => {
            tracing::error!(key, error = %err, "секрет не расшифрован: SN_SECRET_KEY сменился?");
            Ok(None)
        }
    }
}

pub async fn write_secret(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    key: &str,
    value: Option<&str>,
    by: Option<AdminId>,
) -> ServiceResult<()> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => {
            let secrets = secrets.ok_or(ServiceError::NoSecretKey)?;
            let sealed = secrets.seal_str(key, value)?;
            db.secrets().set(key, &sealed, by).await?;
        }
        None => {
            db.secrets().delete(key).await?;
        }
    }
    Ok(())
}

pub async fn secret_or(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    key: &str,
    fallback: Option<String>,
) -> ServiceResult<Option<String>> {
    Ok(read_secret(db, secrets, key).await?.or(fallback))
}

pub async fn remna_connection(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    env: &EnvFallbacks,
) -> ServiceResult<RemnaConnection> {
    let fallback = env.remna.clone().unwrap_or_default();
    let url = db
        .settings()
        .get_as::<String>(keys::REMNA_URL)
        .await?
        .filter(|u| !u.trim().is_empty())
        .unwrap_or(fallback.url);
    let token = read_secret(db, secrets, secret_keys::REMNA_TOKEN)
        .await?
        .unwrap_or(fallback.token);
    let cookie = secret_or(db, secrets, secret_keys::REMNA_COOKIE, fallback.cookie).await?;
    let webhook_secret = secret_or(
        db,
        secrets,
        secret_keys::REMNA_WEBHOOK_SECRET,
        fallback.webhook_secret,
    )
    .await?;
    Ok(RemnaConnection {
        url,
        token,
        cookie,
        webhook_secret,
    })
}

pub async fn registry_snapshot(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    registry: &ProviderRegistry,
    core: &CoreSettings,
    bot_token: Option<&str>,
) -> ServiceResult<RegistrySnapshot> {
    let mut states = HashMap::new();
    for provider in registry.providers() {
        let id = provider.id();
        let stored: ProviderSettings = db
            .settings()
            .get(&keys::provider(id))
            .await?
            .and_then(|v| parse(&keys::provider(id), v))
            .unwrap_or_default();
        let mut config = ProviderConfig::new(stored.config);
        for field in provider.settings_schema().iter().filter(|f| f.is_secret()) {
            if let Some(value) =
                read_secret(db, secrets, &secret_keys::provider(id, field.key)).await?
            {
                config.set(field.key, Value::String(value));
            }
        }
        if id == providers::STARS
            && let Some(token) = bot_token
        {
            config.set(STARS_TOKEN_FIELD, Value::String(token.to_string()));
        }
        states.insert(
            id.to_string(),
            ProviderState {
                enabled: stored
                    .enabled
                    .unwrap_or_else(|| provider.enabled_by_default()),
                config,
            },
        );
    }
    Ok(RegistrySnapshot {
        currency: core.currency,
        states,
        order: core.payment_order.for_currency(core.currency).to_vec(),
    })
}

pub async fn module_secret_rows(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    modules: &sn_modules::ModuleRegistry,
) -> ServiceResult<Vec<ModuleStateRow>> {
    let mut merged = Vec::new();
    for row in db.modules().all().await? {
        let Ok(id) = ModuleId::parse(&row.module) else {
            continue;
        };
        let schema = modules.get(id).settings_schema();
        let mut config = row.config.as_object().cloned().unwrap_or_default();
        let mut found = false;
        for field in schema.iter().filter(|f| f.is_secret()) {
            if let Some(value) =
                read_secret(db, secrets, &secret_keys::module(id.as_str(), field.key)).await?
            {
                config.insert(field.key.to_string(), Value::String(value));
                found = true;
            }
        }
        if found {
            merged.push(ModuleStateRow {
                config: Value::Object(config),
                ..row
            });
        }
    }
    Ok(merged)
}

fn field_error(field: &str, reason: &str) -> ServiceError {
    ServiceError::rejected(
        "settings.invalid_field",
        format!("поле «{field}»: {reason}"),
    )
}

fn checked_value(field: &SettingField, value: &Value) -> ServiceResult<Value> {
    match field.kind {
        FieldKind::Flag => match value {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) if matches!(s.trim(), "true" | "1" | "yes") => Ok(Value::Bool(true)),
            Value::String(s) if matches!(s.trim(), "false" | "0" | "no" | "") => {
                Ok(Value::Bool(false))
            }
            _ => Err(field_error(field.key, "ожидается да или нет")),
        },
        FieldKind::Number => {
            let raw = match value {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.trim().to_string(),
                _ => return Err(field_error(field.key, "ожидается число")),
            };
            let digits = raw.strip_prefix('-').unwrap_or(&raw);
            let valid = !digits.is_empty()
                && digits.split('.').count() <= 2
                && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
                && !digits.starts_with('.')
                && !digits.ends_with('.');
            if valid {
                Ok(Value::String(raw))
            } else {
                Err(field_error(field.key, "ожидается число"))
            }
        }
        FieldKind::Url => {
            let raw = value
                .as_str()
                .map(str::trim)
                .ok_or_else(|| field_error(field.key, "ожидается адрес"))?;
            if raw.is_empty() {
                return Ok(Value::String(String::new()));
            }
            if !(raw.starts_with("https://") || raw.starts_with("http://"))
                || raw.contains(char::is_whitespace)
                || raw.len() > MAX_TEXT_FIELD
            {
                return Err(field_error(field.key, "адрес должен начинаться с https://"));
            }
            Ok(Value::String(raw.to_string()))
        }
        FieldKind::Select { options } => {
            let raw = value
                .as_str()
                .map(str::trim)
                .ok_or_else(|| field_error(field.key, "выберите значение из списка"))?;
            if options.contains(&raw) {
                Ok(Value::String(raw.to_string()))
            } else {
                Err(field_error(field.key, "выберите значение из списка"))
            }
        }
        FieldKind::Text | FieldKind::Secret => {
            let raw = match value {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => return Err(field_error(field.key, "ожидается текст")),
            };
            if raw.chars().count() > MAX_TEXT_FIELD {
                return Err(field_error(field.key, "слишком длинное значение"));
            }
            Ok(Value::String(raw))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretChange {
    Set(String, String),
    Delete(String),
}

pub fn apply_fields(
    schema: &[SettingField],
    incoming: &Map<String, Value>,
    clear: &[String],
    stored: &mut Map<String, Value>,
) -> ServiceResult<Vec<SecretChange>> {
    let mut changes = Vec::new();
    for (key, value) in incoming {
        let field = schema
            .iter()
            .find(|f| f.key == key)
            .ok_or_else(|| field_error(key, "такого поля нет"))?;
        if field.is_secret() {
            match value {
                Value::Null => {}
                Value::String(s) if s.trim().is_empty() => {}
                other => {
                    if let Value::String(text) = checked_value(field, other)? {
                        changes.push(SecretChange::Set(field.key.to_string(), text));
                    }
                }
            }
            continue;
        }
        match value {
            Value::Null => {
                stored.remove(field.key);
            }
            other => {
                stored.insert(field.key.to_string(), checked_value(field, other)?);
            }
        }
    }
    for key in clear {
        let field = schema
            .iter()
            .find(|f| f.key == key)
            .ok_or_else(|| field_error(key, "такого поля нет"))?;
        if field.is_secret() {
            changes.push(SecretChange::Delete(field.key.to_string()));
        } else {
            stored.remove(field.key);
        }
    }
    Ok(changes)
}

pub fn public_config(schema: &[SettingField], config: &Map<String, Value>) -> Map<String, Value> {
    schema
        .iter()
        .map(|field| {
            let value = if field.is_secret() {
                let filled = config
                    .get(field.key)
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.trim().is_empty());
                serde_json::json!({ "filled": filled })
            } else {
                config.get(field.key).cloned().unwrap_or(Value::Null)
            };
            (field.key.to_string(), value)
        })
        .collect()
}

pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 8 {
        return "…".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

pub fn webhook_env(public_url: Option<&str>, secret: &str) -> String {
    let url = match public_url {
        Some(url) => format!("{}{REMNA_WEBHOOK_PATH}", url.trim_end_matches('/')),
        None => format!("<SN_PUBLIC_URL>{REMNA_WEBHOOK_PATH}"),
    };
    format!("WEBHOOK_ENABLED=true\nWEBHOOK_URL={url}\nWEBHOOK_SECRET_HEADER={secret}")
}

fn optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoreSettingsView {
    pub settings: CoreSettings,
    pub currencies: [Currency; 2],
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CoreUpdate {
    #[serde(default)]
    pub currency: Option<Currency>,
    #[serde(default)]
    pub confirm_currency_change: bool,
    #[serde(default)]
    pub brand_name: Option<String>,
    #[serde(default)]
    pub support_url: Option<String>,
    #[serde(default)]
    pub invoice_ttl_minutes: Option<u32>,
    #[serde(default)]
    pub remna_utc_offset_minutes: Option<i32>,
    #[serde(default)]
    pub topup: Option<TopUpSettings>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentsView {
    pub currency: Currency,
    pub order: Vec<String>,
    pub providers: Vec<ProviderView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaymentsUpdate {
    pub order: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ProviderUpdate {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub config: Map<String, Value>,
    #[serde(default)]
    pub clear: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderDetails {
    pub provider: ProviderView,
    pub schema: Vec<SettingField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemnaSettingsView {
    pub url: Option<String>,
    pub url_from_env: bool,
    pub token_set: bool,
    pub token_from_env: bool,
    pub cookie_set: bool,
    pub webhook_secret_set: bool,
    pub webhook_secret_hint: Option<String>,
    pub webhook_url: Option<String>,
    pub env: Option<String>,
    pub utc_offset_minutes: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RemnaUpdate {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub cookie: Option<String>,
    #[serde(default)]
    pub clear_cookie: bool,
    #[serde(default)]
    pub webhook_secret: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RemnaCheckRequest {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub cookie: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemnaCheck {
    pub ok: bool,
    pub version: Option<String>,
    pub supported: bool,
    pub sub_public_domain: Option<String>,
    pub webhooks_enabled: Option<bool>,
    pub usage_records: Option<bool>,
    pub request_history: Option<bool>,
    pub error: Option<String>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebhookSecret {
    pub secret: String,
    pub env: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BotSettingsView {
    pub token_set: bool,
    pub token_from_env: bool,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct BotUpdate {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleConfigView {
    pub module: ModuleId,
    pub schema: Vec<SettingField>,
    pub config: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ModuleConfigUpdate {
    #[serde(default)]
    pub config: Map<String, Value>,
    #[serde(default)]
    pub clear: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SubscriptionSettingsUpdate {
    #[serde(default)]
    pub hwid: Option<HwidSettings>,
    #[serde(default)]
    pub show_custom_remarks: Option<bool>,
    #[serde(default)]
    pub custom_remarks: Option<CustomRemarks>,
}

fn validate_remarks(remarks: &CustomRemarks) -> ServiceResult<()> {
    let groups: [(&str, &Vec<String>); 6] = [
        ("expiredUsers", &remarks.expired_users),
        ("limitedUsers", &remarks.limited_users),
        ("disabledUsers", &remarks.disabled_users),
        ("emptyHosts", &remarks.empty_hosts),
        ("HWIDMaxDevicesExceeded", &remarks.hwid_max_devices_exceeded),
        ("HWIDNotSupported", &remarks.hwid_not_supported),
    ];
    for (name, lines) in groups {
        if lines.iter().all(|l| l.trim().is_empty()) {
            return Err(field_error(name, "нужна хотя бы одна строка"));
        }
        if lines.iter().any(|l| l.chars().count() > MAX_REMARK_CHARS) {
            return Err(field_error(name, "строка длиннее 200 символов"));
        }
    }
    Ok(())
}

fn bot_username(raw: &str) -> ServiceResult<String> {
    let name = raw.trim().trim_start_matches('@');
    let valid = (5..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if valid {
        Ok(name.to_string())
    } else {
        Err(field_error(
            "username",
            "имя бота из латинских букв, цифр и _",
        ))
    }
}

fn bot_token(raw: &str) -> ServiceResult<String> {
    let token = raw.trim();
    let valid = token.split_once(':').is_some_and(|(id, secret)| {
        !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_digit())
            && secret.len() >= 20
            && secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    });
    if valid {
        Ok(token.to_string())
    } else {
        Err(field_error(
            "token",
            "токен выдаёт @BotFather, вида 123456:ABC…",
        ))
    }
}

fn remna_url(raw: &str) -> ServiceResult<String> {
    let url = raw.trim().trim_end_matches('/');
    if (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() > "http://".len()
        && !url.contains(char::is_whitespace)
    {
        Ok(url.to_string())
    } else {
        Err(field_error(
            "url",
            "адрес панели должен начинаться с https://",
        ))
    }
}

pub fn generate_webhook_secret() -> String {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    hex::encode(raw)
}

pub struct SettingsOps<'a> {
    services: &'a Services,
}

impl<'a> SettingsOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        SettingsOps { services }
    }

    async fn saved(&self) -> ServiceResult<()> {
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    pub fn core(&self) -> CoreSettingsView {
        CoreSettingsView {
            settings: self.services.settings().as_ref().clone(),
            currencies: Currency::ALL,
        }
    }

    async fn check_currency_switch(
        &self,
        db: &mut impl Db,
        from: Currency,
        to: Currency,
        confirmed: bool,
    ) -> ServiceResult<()> {
        let mut missing = Vec::new();
        for tariff in db.tariffs().list(false).await? {
            let days: Vec<String> = tariff
                .periods
                .iter()
                .filter(|p| p.active && p.price(to).is_none())
                .map(|p| format!("{} дн.", p.days))
                .collect();
            if !days.is_empty() {
                missing.push(format!("«{}» ({})", tariff.name, days.join(", ")));
            } else if tariff
                .devices
                .extra
                .as_ref()
                .is_some_and(|e| e.monthly.get(to).is_none())
            {
                missing.push(format!("«{}» (доп. устройство)", tariff.name));
            }
        }
        if !missing.is_empty() {
            return Err(ServiceError::rejected(
                "settings.currency_prices",
                format!(
                    "Сначала задайте цены в {}: {}",
                    to.code(),
                    missing.join("; ")
                ),
            ));
        }
        let holders: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT client_id) FROM client_balances WHERE currency = $1 AND amount_minor > 0",
        )
        .bind(from.code())
        .fetch_one(db.conn())
        .await?;
        if holders > 0 && !confirmed {
            return Err(ServiceError::rejected(
                "settings.currency_balances",
                format!(
                    "У {holders} клиентов есть баланс в {}. В {} он не переносится и не будет виден. Подтвердите смену валюты",
                    from.code(),
                    to.code()
                ),
            ));
        }
        Ok(())
    }

    pub async fn update_core(
        &self,
        update: &CoreUpdate,
        admin: AdminId,
    ) -> ServiceResult<CoreSettingsView> {
        let current = self.services.settings();
        let mut tx = self.services.store().begin().await?;
        let mut changed = serde_json::Map::new();
        if let Some(currency) = update.currency
            && currency != current.currency
        {
            self.check_currency_switch(
                &mut tx,
                current.currency,
                currency,
                update.confirm_currency_change,
            )
            .await?;
            tx.settings()
                .set_as(keys::CURRENCY, &currency, Some(admin))
                .await?;
            changed.insert(
                "currency".into(),
                serde_json::json!([current.currency, currency]),
            );
        }
        if let Some(name) = &update.brand_name {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 64 {
                return Err(field_error("brand_name", "от 1 до 64 символов"));
            }
            tx.settings()
                .set_as(keys::BRAND_NAME, &name, Some(admin))
                .await?;
            changed.insert("brand_name".into(), Value::String(name.to_string()));
        }
        if let Some(url) = &update.support_url {
            match optional_text(Some(url)) {
                None => {
                    tx.settings().delete(keys::SUPPORT_URL).await?;
                }
                Some(url) => {
                    let valid = (url.starts_with("https://") || url.starts_with("tg://"))
                        && !url.contains(char::is_whitespace)
                        && url.len() <= 500;
                    if !valid {
                        return Err(field_error(
                            "support_url",
                            "ссылка должна начинаться с https:// или tg://",
                        ));
                    }
                    tx.settings()
                        .set_as(keys::SUPPORT_URL, &url, Some(admin))
                        .await?;
                }
            }
            changed.insert("support_url".into(), Value::Bool(true));
        }
        if let Some(ttl) = update.invoice_ttl_minutes {
            if !(5..=24 * 60).contains(&ttl) {
                return Err(field_error("invoice_ttl_minutes", "от 5 минут до суток"));
            }
            tx.settings()
                .set_as(keys::INVOICE_TTL, &ttl, Some(admin))
                .await?;
            changed.insert("invoice_ttl_minutes".into(), Value::from(ttl));
        }
        if let Some(offset) = update.remna_utc_offset_minutes {
            if !(-14 * 60..=14 * 60).contains(&offset) {
                return Err(field_error(
                    "remna_utc_offset_minutes",
                    "смещение от UTC от −14 до +14 часов",
                ));
            }
            tx.settings()
                .set_as(keys::REMNA_UTC_OFFSET, &offset, Some(admin))
                .await?;
            changed.insert("remna_utc_offset_minutes".into(), Value::from(offset));
        }
        if let Some(topup) = update.topup {
            if topup.min_minor <= 0 || topup.max_minor < topup.min_minor {
                return Err(field_error(
                    "topup",
                    "минимум больше нуля и не больше максимума",
                ));
            }
            tx.settings()
                .set_as(keys::TOPUP, &topup, Some(admin))
                .await?;
            changed.insert("topup".into(), Value::Bool(true));
        }
        if changed.is_empty() {
            tx.commit().await?;
            return Ok(self.core());
        }
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.core",
                "settings",
                None,
                Value::Object(changed),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.core())
    }

    pub fn trial(&self) -> TrialSettings {
        self.services.settings().trial.clone()
    }

    pub async fn update_trial(
        &self,
        trial: &TrialSettings,
        admin: AdminId,
    ) -> ServiceResult<TrialSettings> {
        if !(1..=90).contains(&trial.days) {
            return Err(field_error("days", "пробный период от 1 до 90 дней"));
        }
        if !(1..=100).contains(&trial.devices) {
            return Err(field_error("devices", "от 1 до 100 устройств"));
        }
        trial.traffic.validate()?;
        if trial.enabled && trial.squads.is_empty() {
            return Err(field_error(
                "squads",
                "выберите хотя бы один сквад, иначе у пробной подписки не будет серверов",
            ));
        }
        let mut unique = trial.squads.clone();
        unique.sort();
        unique.dedup();
        if unique.len() != trial.squads.len() {
            return Err(field_error("squads", "сквад указан дважды"));
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::TRIAL, trial, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.trial",
                "settings",
                None,
                serde_json::to_value(trial).map_err(|e| ServiceError::Internal(e.to_string()))?,
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.trial())
    }

    pub fn autorenew(&self) -> AutoRenewSettings {
        self.services.settings().autorenew
    }

    pub async fn update_autorenew(
        &self,
        settings: &AutoRenewSettings,
        admin: AdminId,
    ) -> ServiceResult<AutoRenewSettings> {
        if settings.charge_days_before > 7 {
            return Err(field_error(
                "charge_days_before",
                "списывать не раньше чем за 7 дней",
            ));
        }
        if settings.retry_days > 14 {
            return Err(field_error("retry_days", "повторять не дольше 14 дней"));
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(keys::AUTORENEW, settings, Some(admin))
            .await?;
        tx.audit()
            .record(
                &Actor::Admin(admin).audit(
                    "settings.autorenew",
                    "settings",
                    None,
                    serde_json::to_value(settings)
                        .map_err(|e| ServiceError::Internal(e.to_string()))?,
                ),
            )
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.autorenew())
    }

    pub fn payments(&self) -> PaymentsView {
        let settings = self.services.settings();
        PaymentsView {
            currency: settings.currency,
            order: settings
                .payment_order
                .for_currency(settings.currency)
                .to_vec(),
            providers: self.services.payments().views(),
        }
    }

    pub async fn update_payments(
        &self,
        update: &PaymentsUpdate,
        admin: AdminId,
    ) -> ServiceResult<PaymentsView> {
        let settings = self.services.settings();
        let registry = self.services.payments();
        let mut order: Vec<String> = Vec::new();
        for id in &update.order {
            let id = id.trim();
            let provider = registry.get(id).ok_or_else(|| {
                field_error("order", &format!("неизвестная платёжная система {id}"))
            })?;
            if provider.kind() == ProviderKind::Balance {
                continue;
            }
            if !provider.currencies().contains(&settings.currency) {
                return Err(field_error(
                    "order",
                    &format!(
                        "{} не работает с {}",
                        provider.title(),
                        settings.currency.code()
                    ),
                ));
            }
            if order.iter().any(|o| o == id) {
                return Err(field_error("order", &format!("{id} указан дважды")));
            }
            order.push(id.to_string());
        }
        let mut tx = self.services.store().begin().await?;
        let mut payment_order = tx
            .settings()
            .get(keys::PAYMENT_ORDER)
            .await?
            .and_then(|v| parse::<PaymentOrder>(keys::PAYMENT_ORDER, v))
            .unwrap_or_default();
        match settings.currency {
            Currency::Usd => payment_order.usd = order.clone(),
            Currency::Rub => payment_order.rub = order.clone(),
        }
        tx.settings()
            .set_as(keys::PAYMENT_ORDER, &payment_order, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.payment_order",
                "settings",
                None,
                serde_json::json!({ "currency": settings.currency, "order": order }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.payments())
    }

    async fn stored_provider(&self, db: &mut impl Db, id: &str) -> ServiceResult<ProviderSettings> {
        Ok(db
            .settings()
            .get(&keys::provider(id))
            .await?
            .and_then(|v| parse(&keys::provider(id), v))
            .unwrap_or_default())
    }

    pub fn provider(&self, id: &str) -> ServiceResult<ProviderDetails> {
        let provider = self
            .services
            .payments()
            .get(id)
            .ok_or(ServiceError::NotFound("платёжная система"))?;
        let view = self
            .services
            .payments()
            .views()
            .into_iter()
            .find(|v| v.id == provider.id())
            .ok_or(ServiceError::NotFound("платёжная система"))?;
        Ok(ProviderDetails {
            provider: view,
            schema: provider.settings_schema(),
        })
    }

    pub async fn update_provider(
        &self,
        id: &str,
        update: &ProviderUpdate,
        admin: AdminId,
    ) -> ServiceResult<ProviderDetails> {
        let provider = self
            .services
            .payments()
            .get(id)
            .ok_or(ServiceError::NotFound("платёжная система"))?;
        let id = provider.id();
        let schema = provider.settings_schema();
        let mut tx = self.services.store().begin().await?;
        let mut stored = self.stored_provider(&mut tx, id).await?;
        let changes = apply_fields(&schema, &update.config, &update.clear, &mut stored.config)?;
        if let Some(enabled) = update.enabled {
            stored.enabled = Some(enabled);
        }
        let secrets = self.services.secrets();
        let mut merged = stored.config.clone();
        for field in schema.iter().filter(|f| f.is_secret()) {
            if let Some(value) =
                read_secret(&mut tx, secrets, &secret_keys::provider(id, field.key)).await?
            {
                merged.insert(field.key.to_string(), Value::String(value));
            }
        }
        for change in &changes {
            match change {
                SecretChange::Set(key, value) => {
                    merged.insert(key.clone(), Value::String(value.clone()));
                }
                SecretChange::Delete(key) => {
                    merged.remove(key);
                }
            }
        }
        let mut merged = ProviderConfig::new(merged);
        if id == providers::STARS
            && let Some(token) = self.services.bot_token()
        {
            merged.set(STARS_TOKEN_FIELD, Value::String(token));
        }
        let enabled = stored
            .enabled
            .unwrap_or_else(|| provider.enabled_by_default());
        if enabled && provider.kind() != ProviderKind::Balance {
            provider
                .validate_settings(&merged)
                .map_err(|e| ServiceError::rejected("settings.provider_invalid", e.to_string()))?;
            if !provider.is_configured(&merged) {
                return Err(ServiceError::rejected(
                    "settings.provider_unavailable",
                    format!(
                        "{} пока нельзя включить: подключение не готово",
                        provider.title()
                    ),
                ));
            }
        }
        for change in &changes {
            match change {
                SecretChange::Set(key, value) => {
                    write_secret(
                        &mut tx,
                        secrets,
                        &secret_keys::provider(id, key),
                        Some(value),
                        Some(admin),
                    )
                    .await?;
                }
                SecretChange::Delete(key) => {
                    write_secret(
                        &mut tx,
                        secrets,
                        &secret_keys::provider(id, key),
                        None,
                        None,
                    )
                    .await?;
                }
            }
        }
        tx.settings()
            .set_as(&keys::provider(id), &stored, Some(admin))
            .await?;
        let changed_secrets: Vec<&str> = changes
            .iter()
            .map(|c| match c {
                SecretChange::Set(key, _) | SecretChange::Delete(key) => key.as_str(),
            })
            .collect();
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.provider",
                "provider",
                None,
                serde_json::json!({
                    "provider": id,
                    "enabled": stored.enabled,
                    "fields": update.config.keys().collect::<Vec<_>>(),
                    "secrets": changed_secrets,
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        self.provider(id)
    }

    pub async fn remnawave(&self) -> ServiceResult<RemnaSettingsView> {
        let env = self.services.env().remna.clone().unwrap_or_default();
        let mut conn = self.services.store().conn().await?;
        let stored_url = conn
            .settings()
            .get_as::<String>(keys::REMNA_URL)
            .await?
            .filter(|u| !u.trim().is_empty());
        let secrets = self.services.secrets();
        let token = read_secret(&mut conn, secrets, secret_keys::REMNA_TOKEN).await?;
        let cookie = read_secret(&mut conn, secrets, secret_keys::REMNA_COOKIE).await?;
        drop(conn);
        let webhook = self.services.remna_webhook_secret();
        let public_url = self.services.settings().public_url.clone();
        let url = stored_url.clone().or_else(|| optional_text(Some(&env.url)));
        Ok(RemnaSettingsView {
            url_from_env: stored_url.is_none() && url.is_some(),
            url,
            token_set: token.is_some() || !env.token.trim().is_empty(),
            token_from_env: token.is_none() && !env.token.trim().is_empty(),
            cookie_set: cookie.is_some() || env.cookie.is_some(),
            webhook_secret_set: webhook.is_some(),
            webhook_secret_hint: webhook.as_deref().map(mask_secret),
            webhook_url: public_url
                .as_deref()
                .map(|u| format!("{}{REMNA_WEBHOOK_PATH}", u.trim_end_matches('/'))),
            env: webhook
                .as_deref()
                .map(|secret| webhook_env(public_url.as_deref(), &mask_secret(secret))),
            utc_offset_minutes: self.services.settings().remna_utc_offset_minutes,
        })
    }

    pub async fn update_remnawave(
        &self,
        update: &RemnaUpdate,
        admin: AdminId,
    ) -> ServiceResult<RemnaSettingsView> {
        let secrets = self.services.secrets();
        let mut tx = self.services.store().begin().await?;
        let mut changed = Vec::new();
        if let Some(url) = &update.url {
            match optional_text(Some(url)) {
                Some(url) => {
                    let url = remna_url(&url)?;
                    tx.settings()
                        .set_as(keys::REMNA_URL, &url, Some(admin))
                        .await?;
                }
                None => {
                    tx.settings().delete(keys::REMNA_URL).await?;
                }
            }
            changed.push("url");
        }
        if let Some(token) = optional_text(update.token.as_deref()) {
            if token.contains(char::is_whitespace) {
                return Err(field_error("token", "токен без пробелов"));
            }
            write_secret(
                &mut tx,
                secrets,
                secret_keys::REMNA_TOKEN,
                Some(&token),
                Some(admin),
            )
            .await?;
            changed.push("token");
        }
        if update.clear_cookie {
            write_secret(&mut tx, secrets, secret_keys::REMNA_COOKIE, None, None).await?;
            changed.push("cookie");
        } else if let Some(cookie) = optional_text(update.cookie.as_deref()) {
            let cookie = sn_remnawave::client::normalize_cookie(&cookie)
                .ok_or_else(|| field_error("cookie", "кука в виде имя=значение"))?;
            write_secret(
                &mut tx,
                secrets,
                secret_keys::REMNA_COOKIE,
                Some(&cookie),
                Some(admin),
            )
            .await?;
            changed.push("cookie");
        }
        if let Some(secret) = optional_text(update.webhook_secret.as_deref()) {
            sn_remnawave::webhook::validate_secret(&secret)
                .map_err(|e| field_error("webhook_secret", &e.to_string()))?;
            write_secret(
                &mut tx,
                secrets,
                secret_keys::REMNA_WEBHOOK_SECRET,
                Some(&secret),
                Some(admin),
            )
            .await?;
            changed.push("webhook_secret");
        }
        if !changed.is_empty() {
            tx.audit()
                .record(&Actor::Admin(admin).audit(
                    "settings.remnawave",
                    "settings",
                    None,
                    serde_json::json!({ "fields": changed }),
                ))
                .await?;
        }
        tx.commit().await?;
        if !changed.is_empty() {
            self.saved().await?;
        }
        self.remnawave().await
    }

    pub async fn generate_remna_webhook_secret(
        &self,
        admin: AdminId,
    ) -> ServiceResult<WebhookSecret> {
        let secret = generate_webhook_secret();
        let mut tx = self.services.store().begin().await?;
        write_secret(
            &mut tx,
            self.services.secrets(),
            secret_keys::REMNA_WEBHOOK_SECRET,
            Some(&secret),
            Some(admin),
        )
        .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.remnawave_webhook_secret",
                "settings",
                None,
                serde_json::json!({}),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        let env = webhook_env(self.services.settings().public_url.as_deref(), &secret);
        Ok(WebhookSecret { secret, env })
    }

    pub async fn check_remnawave(&self, request: &RemnaCheckRequest) -> ServiceResult<RemnaCheck> {
        let custom = request.url.is_some() || request.token.is_some() || request.cookie.is_some();
        let api: Arc<dyn RemnawaveApi> = if custom {
            let mut conn = self.services.store().conn().await?;
            let stored =
                remna_connection(&mut conn, self.services.secrets(), self.services.env()).await?;
            drop(conn);
            let url = match optional_text(request.url.as_deref()) {
                Some(url) => remna_url(&url)?,
                None => stored.url,
            };
            let token = optional_text(request.token.as_deref()).unwrap_or(stored.token);
            let mut config = RemnaConfig::new(url, token);
            config.cookie = optional_text(request.cookie.as_deref()).or(stored.cookie);
            match RemnawaveClient::new(config) {
                Ok(client) => Arc::new(client),
                Err(err) => return Ok(self.failed_check(err.to_string())),
            }
        } else {
            match self.services.remna() {
                Ok(api) => api,
                Err(err) => return Ok(self.failed_check(err.to_string())),
            }
        };
        let metadata = match api.metadata().await {
            Ok(metadata) => metadata,
            Err(err) => return Ok(self.failed_check(err.to_string())),
        };
        let configuration = api.configuration().await;
        let supported = metadata.version == sn_remnawave::SUPPORTED_VERSION;
        Ok(match configuration {
            Ok(configuration) => RemnaCheck {
                ok: true,
                supported,
                version: Some(metadata.version),
                sub_public_domain: optional_text(Some(&configuration.misc.sub_public_domain)),
                webhooks_enabled: Some(configuration.notifications.webhook),
                usage_records: Some(!configuration.service.disable_user_usage_records),
                request_history: Some(!configuration.service.disable_srh_records),
                error: None,
                checked_at: self.services.now(),
            },
            Err(err) => RemnaCheck {
                ok: false,
                supported,
                version: Some(metadata.version),
                error: Some(err.to_string()),
                ..self.failed_check(String::new())
            },
        })
    }

    fn failed_check(&self, error: String) -> RemnaCheck {
        RemnaCheck {
            ok: false,
            version: None,
            supported: false,
            sub_public_domain: None,
            webhooks_enabled: None,
            usage_records: None,
            request_history: None,
            error: Some(error),
            checked_at: self.services.now(),
        }
    }

    pub async fn subscription_settings(&self) -> ServiceResult<SubscriptionSettings> {
        Ok(self.services.remna()?.subscription_settings().await?)
    }

    pub async fn update_subscription_settings(
        &self,
        update: &SubscriptionSettingsUpdate,
        admin: AdminId,
    ) -> ServiceResult<SubscriptionSettings> {
        let remna = self.services.remna()?;
        if let Some(hwid) = &update.hwid {
            if !(0..=100).contains(&hwid.fallback_device_limit) {
                return Err(field_error("fallbackDeviceLimit", "от 0 до 100 устройств"));
            }
            if hwid
                .max_devices_announce
                .as_deref()
                .is_some_and(|text| text.chars().count() > MAX_REMARK_CHARS)
            {
                return Err(field_error("maxDevicesAnnounce", "не длиннее 200 символов"));
            }
        }
        if let Some(remarks) = &update.custom_remarks {
            validate_remarks(remarks)?;
        }
        let current = remna.subscription_settings().await?;
        let updated = remna
            .update_subscription_settings(&UpdateSubscriptionSettingsRequest {
                uuid: current.uuid,
                is_show_custom_remarks: update.show_custom_remarks,
                custom_remarks: update.custom_remarks.clone(),
                hwid_settings: update.hwid.clone(),
            })
            .await?;
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.remnawave_subscription",
                "settings",
                None,
                serde_json::json!({
                    "hwid": update.hwid.is_some(),
                    "remarks": update.custom_remarks.is_some(),
                    "show_remarks": update.show_custom_remarks,
                }),
            ))
            .await?;
        Ok(updated)
    }

    pub async fn bot(&self) -> ServiceResult<BotSettingsView> {
        let mut conn = self.services.store().conn().await?;
        let stored =
            read_secret(&mut conn, self.services.secrets(), secret_keys::BOT_TOKEN).await?;
        let username = conn
            .settings()
            .get_as::<String>(keys::BOT_USERNAME)
            .await?
            .filter(|u| !u.trim().is_empty());
        let env = self.services.env().bot_token.is_some();
        Ok(BotSettingsView {
            token_set: stored.is_some() || env,
            token_from_env: stored.is_none() && env,
            username,
        })
    }

    pub async fn update_bot(
        &self,
        update: &BotUpdate,
        admin: AdminId,
    ) -> ServiceResult<BotSettingsView> {
        let mut tx = self.services.store().begin().await?;
        let mut changed = Vec::new();
        if let Some(token) = optional_text(update.token.as_deref()) {
            let token = bot_token(&token)?;
            write_secret(
                &mut tx,
                self.services.secrets(),
                secret_keys::BOT_TOKEN,
                Some(&token),
                Some(admin),
            )
            .await?;
            changed.push("token");
        }
        if let Some(username) = &update.username {
            match optional_text(Some(username)) {
                Some(username) => {
                    let username = bot_username(&username)?;
                    tx.settings()
                        .set_as(keys::BOT_USERNAME, &username, Some(admin))
                        .await?;
                }
                None => {
                    tx.settings().delete(keys::BOT_USERNAME).await?;
                }
            }
            changed.push("username");
        }
        if !changed.is_empty() {
            tx.audit()
                .record(&Actor::Admin(admin).audit(
                    "settings.bot",
                    "settings",
                    None,
                    serde_json::json!({ "fields": changed }),
                ))
                .await?;
        }
        tx.commit().await?;
        if !changed.is_empty() {
            self.saved().await?;
        }
        self.bot().await
    }

    pub fn module_config(&self, id: ModuleId) -> ModuleConfigView {
        let modules = self.services.modules();
        let schema = modules.get(id).settings_schema();
        let config = public_config(&schema, &modules.config(id).0);
        ModuleConfigView {
            module: id,
            schema,
            config,
        }
    }

    pub async fn update_module_config(
        &self,
        id: ModuleId,
        update: &ModuleConfigUpdate,
        admin: AdminId,
    ) -> ServiceResult<ModuleConfigView> {
        let modules = self.services.modules();
        let module = modules.get(id);
        let schema = module.settings_schema();
        let secrets = self.services.secrets();
        let mut tx = self.services.store().begin().await?;
        let mut stored = tx
            .modules()
            .all()
            .await?
            .into_iter()
            .find(|row| row.module == id.as_str())
            .and_then(|row| row.config.as_object().cloned())
            .unwrap_or_default();
        let changes = apply_fields(&schema, &update.config, &update.clear, &mut stored)?;
        for field in schema.iter().filter(|f| f.is_secret()) {
            stored.remove(field.key);
        }
        let mut merged = stored.clone();
        for field in schema.iter().filter(|f| f.is_secret()) {
            if let Some(value) = read_secret(
                &mut tx,
                secrets,
                &secret_keys::module(id.as_str(), field.key),
            )
            .await?
            {
                merged.insert(field.key.to_string(), Value::String(value));
            }
        }
        for change in &changes {
            match change {
                SecretChange::Set(key, value) => {
                    merged.insert(key.clone(), Value::String(value.clone()));
                }
                SecretChange::Delete(key) => {
                    merged.remove(key);
                }
            }
        }
        module
            .validate_config(&ModuleConfig(merged))
            .map_err(|e| ServiceError::rejected("modules.config", e.to_string()))?;
        for change in &changes {
            match change {
                SecretChange::Set(key, value) => {
                    write_secret(
                        &mut tx,
                        secrets,
                        &secret_keys::module(id.as_str(), key),
                        Some(value),
                        Some(admin),
                    )
                    .await?;
                }
                SecretChange::Delete(key) => {
                    write_secret(
                        &mut tx,
                        secrets,
                        &secret_keys::module(id.as_str(), key),
                        None,
                        None,
                    )
                    .await?;
                }
            }
        }
        tx.modules()
            .set_config(id.as_str(), &Value::Object(stored), Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "modules.config",
                "module",
                None,
                serde_json::json!({
                    "module": id,
                    "fields": update.config.keys().collect::<Vec<_>>(),
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.module_config(id))
    }

    pub async fn toggle_module(
        &self,
        id: ModuleId,
        enabled: bool,
        admin: AdminId,
    ) -> ServiceResult<ModuleView> {
        let modules = self.services.modules();
        let view = modules
            .set_enabled(self.services.store(), id, enabled, Some(admin))
            .await?;
        let mut conn = self.services.store().conn().await?;
        let rows = module_secret_rows(&mut conn, self.services.secrets(), modules).await?;
        modules.apply_rows(&rows);
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "modules.toggle",
                "module",
                None,
                serde_json::json!({ "module": id, "enabled": enabled }),
            ))
            .await?;
        drop(conn);
        self.services.notify_settings_changed().await?;
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Vec<SettingField> {
        vec![
            SettingField::secret("api_key", "Ключ", ""),
            SettingField::text("shop_id", "Магазин", ""),
            SettingField::flag("testnet", "Тест", ""),
            SettingField::number("vat", "НДС", ""),
            SettingField::select("mode", "Режим", "", &["a", "b"]),
        ]
    }

    #[test]
    fn секрет_только_на_запись_пустое_значение_не_меняет() {
        let mut stored = Map::new();
        stored.insert("shop_id".into(), Value::String("old".into()));
        let incoming: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "api_key": "", "shop_id": "  new  ", "testnet": "true", "vat": 20, "mode": "b"
        }))
        .unwrap();
        let changes = apply_fields(&schema(), &incoming, &[], &mut stored).unwrap();
        assert!(changes.is_empty());
        assert_eq!(stored["shop_id"], "new");
        assert_eq!(stored["testnet"], true);
        assert_eq!(stored["vat"], "20");
        let incoming: Map<String, Value> =
            serde_json::from_value(serde_json::json!({ "api_key": "s3cr3t" })).unwrap();
        let changes = apply_fields(&schema(), &incoming, &["shop_id".into()], &mut stored).unwrap();
        assert_eq!(
            changes,
            vec![SecretChange::Set("api_key".into(), "s3cr3t".into())]
        );
        assert!(!stored.contains_key("shop_id"));
        assert!(!stored.contains_key("api_key"));
        let changes =
            apply_fields(&schema(), &Map::new(), &["api_key".into()], &mut stored).unwrap();
        assert_eq!(changes, vec![SecretChange::Delete("api_key".into())]);
    }

    #[test]
    fn неизвестное_поле_и_неверный_тип_отклоняются() {
        let mut stored = Map::new();
        for body in [
            serde_json::json!({ "unknown": 1 }),
            serde_json::json!({ "vat": "двадцать" }),
            serde_json::json!({ "mode": "c" }),
            serde_json::json!({ "testnet": [1] }),
        ] {
            let incoming: Map<String, Value> = serde_json::from_value(body).unwrap();
            assert!(apply_fields(&schema(), &incoming, &[], &mut stored).is_err());
        }
    }

    #[test]
    fn публичный_вид_не_показывает_секреты() {
        let mut config = Map::new();
        config.insert("api_key".into(), Value::String("s3cr3t".into()));
        config.insert("shop_id".into(), Value::String("42".into()));
        let view = public_config(&schema(), &config);
        assert_eq!(view["api_key"], serde_json::json!({ "filled": true }));
        assert_eq!(view["shop_id"], "42");
        assert!(!serde_json::to_string(&view).unwrap().contains("s3cr3t"));
    }

    #[test]
    fn блок_env_для_вебхуков_remnawave() {
        let secret = generate_webhook_secret();
        assert_eq!(secret.len(), 64);
        assert!(secret.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(sn_remnawave::webhook::validate_secret(&secret).is_ok());
        assert_eq!(
            webhook_env(Some("https://bot.example.com/"), "abc"),
            "WEBHOOK_ENABLED=true\nWEBHOOK_URL=https://bot.example.com/api/remnawave/webhook\nWEBHOOK_SECRET_HEADER=abc"
        );
        assert_eq!(mask_secret(&"7f3c".repeat(16)), "7f3c…7f3c");
        assert_eq!(mask_secret("short"), "…");
    }

    #[test]
    fn имя_и_токен_бота_проверяются() {
        assert_eq!(bot_username("@stealth_vpn_bot").unwrap(), "stealth_vpn_bot");
        assert!(bot_username("бот").is_err());
        assert!(bot_token("123456:AAEhBP0av28N6CLdRmP6gWBJr2cs2hZ1T9Q").is_ok());
        assert!(bot_token("not-a-token").is_err());
    }
}
