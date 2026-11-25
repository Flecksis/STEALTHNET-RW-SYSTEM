use crate::context::ModuleContext;
use crate::module::ModuleConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sn_domain::module::ModuleId;
use sn_store::{Db, Store, StoreError};
use std::sync::OnceLock;
use std::time::Duration;

pub const TURNSTILE_VERIFY: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
pub const YANDEX_VERIFY: &str = "https://smartcaptcha.cloud.yandex.ru/validate";
pub const MAX_TOKEN: usize = 2048;
pub const MAX_KEY: usize = 200;
pub const HEALTH_KEY: &str = "antibot.captcha";
pub const MAX_TURNSTILE_SITE_KEY: usize = 32;
const TURNSTILE_TEST_SECRETS: [&str; 3] = [
    "1x0000000000000000000000000000000AA",
    "2x0000000000000000000000000000000AA",
    "3x0000000000000000000000000000000AA",
];
const TURNSTILE_PREFIX: &str = "0x4";
const YANDEX_SITE_PREFIX: &str = "ysc1_";
const YANDEX_SECRET_PREFIX: &str = "ysc2_";
const TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const PROVIDER_OPTIONS: &[&str] = &["none", "turnstile", "yandex"];
const SCOPE_OPTIONS: &[&str] = &["all", "suspicious"];

pub fn provider_options() -> &'static [&'static str] {
    PROVIDER_OPTIONS
}

pub fn scope_options() -> &'static [&'static str] {
    SCOPE_OPTIONS
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    None,
    Turnstile,
    Yandex,
}

impl Provider {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "" | "none" => Some(Provider::None),
            "turnstile" => Some(Provider::Turnstile),
            "yandex" => Some(Provider::Yandex),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Provider::None => "none",
            Provider::Turnstile => "turnstile",
            Provider::Yandex => "yandex",
        }
    }

    pub const fn origin(self) -> &'static str {
        match self {
            Provider::None => "",
            Provider::Turnstile => "https://challenges.cloudflare.com",
            Provider::Yandex => "https://smartcaptcha.cloud.yandex.ru",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Provider::None => "без внешней капчи",
            Provider::Turnstile => "Cloudflare Turnstile",
            Provider::Yandex => "Яндекс SmartCaptcha",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    All,
    Suspicious,
}

impl Scope {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "" | "all" => Some(Scope::All),
            "suspicious" => Some(Scope::Suspicious),
            _ => None,
        }
    }
}

pub fn clean_key(raw: &str) -> Option<String> {
    let key = raw.trim();
    (!key.is_empty()
        && key.len() <= MAX_KEY
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')))
    .then(|| key.to_string())
}

fn maker(key: &str) -> Provider {
    if key.starts_with(YANDEX_SITE_PREFIX) || key.starts_with(YANDEX_SECRET_PREFIX) {
        Provider::Yandex
    } else if key.starts_with(TURNSTILE_PREFIX) {
        Provider::Turnstile
    } else {
        Provider::None
    }
}

pub fn foreign(provider: Provider, key: &str) -> Option<Provider> {
    let made = maker(key);
    (provider != Provider::None && made != Provider::None && made != provider).then_some(made)
}

pub fn foreign_text(provider: Provider, made: Provider) -> String {
    format!(
        "Это ключ {}, а выбрана капча {}",
        made.title(),
        provider.title()
    )
}

pub fn check_keys(provider: Provider, site_key: &str, secret: &str) -> Result<(), String> {
    if let Some(made) = foreign(provider, site_key) {
        return Err(foreign_text(provider, made));
    }
    if provider == Provider::Yandex && site_key.starts_with(YANDEX_SECRET_PREFIX) {
        return Err(
            "В поле «Ключ сайта» вставлен серверный ключ SmartCaptcha (ysc2_…). Ключ сайта начинается с ysc1_"
                .into(),
        );
    }
    if provider == Provider::Yandex && secret.starts_with(YANDEX_SITE_PREFIX) {
        return Err(
            "В поле «Секретный ключ» вставлен ключ сайта SmartCaptcha (ysc1_…). Секретный начинается с ysc2_"
                .into(),
        );
    }
    if provider == Provider::Turnstile && site_key.len() > MAX_TURNSTILE_SITE_KEY {
        return Err(
            "В поле «Ключ сайта» вставлен секретный ключ Turnstile: ключ сайта короче. Поменяйте ключи местами"
                .into(),
        );
    }
    if site_key == secret {
        return Err(
            "В поля «Ключ сайта» и «Секретный ключ» вставлен один и тот же ключ: нужны оба ключа капчи"
                .into(),
        );
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq)]
pub struct Captcha {
    pub provider: Provider,
    pub site_key: String,
    secret: String,
    pub trial: Scope,
    pub signup: bool,
}

impl std::fmt::Debug for Captcha {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Captcha")
            .field("provider", &self.provider)
            .field("site_key", &self.site_key)
            .field("trial", &self.trial)
            .field("signup", &self.signup)
            .finish_non_exhaustive()
    }
}

impl Captcha {
    pub fn of(config: &ModuleConfig) -> Option<Self> {
        let settings = super::AntibotSettings::lenient(config);
        if settings.captcha == Provider::None {
            return None;
        }
        let site_key = clean_key(&settings.captcha_site_key)?;
        let secret = config.text("captcha_secret").and_then(clean_key)?;
        Some(Captcha {
            provider: settings.captcha,
            site_key,
            secret,
            trial: settings.captcha_trial,
            signup: settings.captcha_signup,
        })
    }

    pub fn widget(&self, action: &str, cdata: Option<String>) -> Widget {
        Widget {
            provider: self.provider,
            site_key: self.site_key.clone(),
            action: action.to_string(),
            cdata,
        }
    }
}

pub fn active(ctx: &ModuleContext) -> Option<Captcha> {
    if !ctx.core.module_enabled(ModuleId::Antibot) {
        return None;
    }
    Captcha::of(&ctx.core.module_config(ModuleId::Antibot))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Widget {
    pub provider: Provider,
    pub site_key: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdata: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub turnstile: String,
    pub yandex: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            turnstile: TURNSTILE_VERIFY.to_string(),
            yandex: YANDEX_VERIFY.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expect<'a> {
    pub action: &'a str,
    pub cdata: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Rejected(String),
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub at: DateTime<Utc>,
    pub provider: Provider,
    pub problem: String,
}

pub async fn health(db: &mut impl Db) -> Result<Option<Health>, StoreError> {
    let saved = db.bot_state().get(HEALTH_KEY).await?;
    Ok(saved.and_then(|value| serde_json::from_value(value).ok()))
}

async fn remember(store: &Store, provider: Provider, outcome: &Outcome) -> Result<(), StoreError> {
    let mut conn = store.conn().await?;
    match outcome {
        Outcome::Unavailable(problem) => {
            let health = Health {
                at: Utc::now(),
                provider,
                problem: problem.clone(),
            };
            conn.bot_state()
                .set(HEALTH_KEY, &serde_json::json!(health))
                .await
        }
        Outcome::Passed | Outcome::Rejected(_) => {
            sqlx::query("DELETE FROM bot_state WHERE key = $1")
                .bind(HEALTH_KEY)
                .execute(conn.conn())
                .await?;
            Ok(())
        }
    }
}

fn http() -> &'static reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        sn_egress::build(
            reqwest::Client::builder()
                .timeout(TIMEOUT)
                .connect_timeout(CONNECT_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION"))),
            None,
        )
    })
}

pub fn token_digest(token: &str) -> Vec<u8> {
    Sha256::digest(format!("captcha:{token}").as_bytes()).to_vec()
}

async fn spend(db: &mut impl Db, token: &str) -> Result<bool, StoreError> {
    let fresh: Option<i32> = sqlx::query_scalar(
        "INSERT INTO antibot_captcha_spent (token_hash) VALUES ($1) ON CONFLICT DO NOTHING RETURNING 1",
    )
    .bind(token_digest(token))
    .fetch_optional(db.conn())
    .await?;
    Ok(fresh.is_some())
}

pub async fn sweep(db: &mut impl Db) -> Result<u64, StoreError> {
    let spent =
        sqlx::query("DELETE FROM antibot_captcha_spent WHERE spent_at < now() - interval '1 hour'")
            .execute(db.conn())
            .await?
            .rows_affected();
    let checks =
        sqlx::query("DELETE FROM antibot_checks WHERE expires_at < now() - interval '1 day'")
            .execute(db.conn())
            .await?
            .rows_affected();
    Ok(spent + checks)
}

pub async fn verify(
    store: &Store,
    endpoints: &Endpoints,
    captcha: &Captcha,
    token: &str,
    ip: Option<&str>,
    expect: Expect<'_>,
) -> Result<Outcome, StoreError> {
    let token = token.trim();
    if token.is_empty() || token.len() > MAX_TOKEN {
        return Ok(Outcome::Rejected("empty".into()));
    }
    let fresh = {
        let mut conn = store.conn().await?;
        spend(&mut conn, token).await?
    };
    if !fresh {
        return Ok(Outcome::Rejected("duplicate".into()));
    }
    let ip = ip.map(str::trim).filter(|ip| !ip.is_empty());
    let outcome = match captcha.provider {
        Provider::Turnstile => turnstile(&endpoints.turnstile, captcha, token, ip, expect).await,
        Provider::Yandex => yandex(&endpoints.yandex, captcha, token, ip).await,
        Provider::None => Outcome::Unavailable("капча не настроена".into()),
    };
    if let Outcome::Unavailable(problem) = &outcome {
        tracing::warn!(provider = captcha.provider.as_str(), %problem, "капча: проверка не выполнена");
    }
    if let Err(err) = remember(store, captcha.provider, &outcome).await {
        tracing::warn!(error = %err, "капча: состояние сервиса проверки не сохранено");
    }
    Ok(outcome)
}

fn transport(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "сервис проверки не ответил вовремя".into()
    } else if err.is_connect() {
        "не удалось соединиться с сервисом проверки".into()
    } else {
        "сервис проверки недоступен".into()
    }
}

#[derive(Deserialize)]
struct TurnstileReply {
    success: bool,
    #[serde(default, rename = "error-codes")]
    errors: Option<Vec<String>>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    cdata: Option<String>,
}

async fn turnstile(
    url: &str,
    captcha: &Captcha,
    token: &str,
    ip: Option<&str>,
    expect: Expect<'_>,
) -> Outcome {
    let mut form = vec![
        ("secret", captcha.secret.clone()),
        ("response", token.to_string()),
        ("idempotency_key", uuid::Uuid::new_v4().to_string()),
    ];
    if let Some(ip) = ip {
        form.push(("remoteip", ip.to_string()));
    }
    let response = match http().post(url).form(&form).send().await {
        Ok(response) => response,
        Err(err) => return Outcome::Unavailable(transport(&err)),
    };
    let status = response.status();
    let reply: TurnstileReply = match response.json().await {
        Ok(reply) => reply,
        Err(_) => {
            return Outcome::Unavailable(format!(
                "сервис проверки ответил HTTP {}",
                status.as_u16()
            ));
        }
    };
    let errors = reply.errors.unwrap_or_default();
    if !reply.success {
        if errors
            .iter()
            .any(|e| matches!(e.as_str(), "missing-input-secret" | "invalid-input-secret"))
        {
            return Outcome::Unavailable("секретный ключ не подошёл".into());
        }
        if errors.iter().any(|e| e == "internal-error") || errors.is_empty() {
            return Outcome::Unavailable("сервис проверки ответил ошибкой".into());
        }
        return Outcome::Rejected(errors.join(","));
    }
    let test_keys = TURNSTILE_TEST_SECRETS.contains(&captcha.secret.as_str());
    let bound = |got: Option<&str>, want: &str| match got {
        Some(got) => got == want,
        None => test_keys,
    };
    if !bound(reply.action.as_deref(), expect.action) {
        return Outcome::Rejected("action".into());
    }
    if expect
        .cdata
        .is_some_and(|want| !bound(reply.cdata.as_deref(), want))
    {
        return Outcome::Rejected("cdata".into());
    }
    Outcome::Passed
}

#[derive(Deserialize)]
struct YandexReply {
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    host: Option<String>,
}

async fn yandex(url: &str, captcha: &Captcha, token: &str, ip: Option<&str>) -> Outcome {
    let mut form = vec![
        ("secret", captcha.secret.clone()),
        ("token", token.to_string()),
    ];
    if let Some(ip) = ip {
        form.push(("ip", ip.to_string()));
    }
    let response = match http().post(url).form(&form).send().await {
        Ok(response) => response,
        Err(err) => return Outcome::Unavailable(transport(&err)),
    };
    let status = response.status();
    let reply: Option<YandexReply> = response.json().await.ok();
    let Some(reply) = reply else {
        return Outcome::Unavailable(format!("сервис проверки ответил HTTP {}", status.as_u16()));
    };
    let message = reply.message.unwrap_or_default().to_lowercase();
    if reply.status == "failed"
        && (message.contains("secret") || message.contains("authentication"))
    {
        return Outcome::Unavailable("секретный ключ не подошёл".into());
    }
    if status != reqwest::StatusCode::OK {
        return Outcome::Unavailable(format!("сервис проверки ответил HTTP {}", status.as_u16()));
    }
    match reply.status.as_str() {
        "ok" if reply.host.as_deref().is_some_and(|h| !h.trim().is_empty()) => Outcome::Passed,
        "ok" => Outcome::Unavailable("сервис пропустил посетителя без проверки".into()),
        "failed" => Outcome::Rejected("failed".into()),
        _ => Outcome::Unavailable("сервис проверки ответил непонятно".into()),
    }
}
