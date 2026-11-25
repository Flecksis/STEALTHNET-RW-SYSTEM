use crate::context::ModuleContext;
use crate::module::ModuleConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sn_domain::module::ModuleId;
use sn_store::{Db, Store, StoreError};
use std::time::Duration;

pub const TURNSTILE_VERIFY: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
pub const YANDEX_VERIFY: &str = "https://smartcaptcha.cloud.yandex.ru/validate";
pub const MAX_TOKEN: usize = 2048;
pub const MAX_KEY: usize = 200;
pub const HEALTH_KEY: &str = "antibot.captcha";
pub const MAX_TURNSTILE_SITE_KEY: usize = 32;
pub const MAX_DOMAINS: usize = 50;
const TURNSTILE_TEST_SECRETS: [&str; 3] = [
    "1x0000000000000000000000000000000AA",
    "2x0000000000000000000000000000000AA",
    "3x0000000000000000000000000000000AA",
];
const TURNSTILE_PREFIX: &str = "0x4";
const YANDEX_SITE_PREFIX: &str = "ysc1_";
const YANDEX_SECRET_PREFIX: &str = "ysc2_";
const BAD_SECRET: &str = "секретный ключ не подошёл";
const BROKEN: &str = "сервис проверки ответил ошибкой";
const UNCLEAR: &str = "сервис проверки ответил непонятно";
const WAVED: &str = "сервис пропустил посетителя без проверки";
const PROBE_TOKEN: &str = "stealthnet-key-check";
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

pub fn clean_host(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let bare = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    let bare = bare.split(['/', '?', '#']).next().unwrap_or_default();
    let bare = bare.strip_prefix("*.").unwrap_or(bare);
    let url = reqwest::Url::parse(&format!("https://{bare}")).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?.trim_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

pub fn parse_domains(raw: &str) -> Result<Vec<String>, String> {
    let mut domains: Vec<String> = Vec::new();
    for part in raw
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';'))
        .filter(|part| !part.is_empty())
    {
        let host = clean_host(part)
            .filter(|host| host.contains('.') || host == "localhost")
            .ok_or_else(|| {
                let shown: String = part.chars().take(60).collect();
                format!("«{shown}» — не домен")
            })?;
        if !domains.contains(&host) {
            domains.push(host);
        }
    }
    if domains.len() > MAX_DOMAINS {
        return Err(format!("не больше {MAX_DOMAINS} доменов"));
    }
    Ok(domains)
}

pub fn covered(domains: &[String], host: &str) -> bool {
    domains.is_empty()
        || clean_host(host).is_some_and(|host| {
            domains.iter().any(|domain| {
                host == *domain
                    || host
                        .strip_suffix(domain.as_str())
                        .is_some_and(|rest| rest.ends_with('.'))
            })
        })
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
    pub domains: Vec<String>,
}

impl std::fmt::Debug for Captcha {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Captcha")
            .field("provider", &self.provider)
            .field("site_key", &self.site_key)
            .field("trial", &self.trial)
            .field("signup", &self.signup)
            .field("domains", &self.domains)
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
            domains: settings.captcha_domains,
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

impl Endpoints {
    pub fn of(&self, provider: Provider) -> &str {
        match provider {
            Provider::Turnstile => &self.turnstile,
            Provider::Yandex => &self.yandex,
            Provider::None => "",
        }
    }
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
    Foreign(String),
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub at: DateTime<Utc>,
    pub provider: Provider,
    pub problem: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign: Option<String>,
}

pub async fn health(db: &mut impl Db) -> Result<Option<Health>, StoreError> {
    let saved = db.bot_state().get(HEALTH_KEY).await?;
    Ok(saved.and_then(|value| serde_json::from_value(value).ok()))
}

async fn remember(store: &Store, provider: Provider, outcome: &Outcome) -> Result<(), StoreError> {
    let mut conn = store.conn().await?;
    let (problem, foreign) = match outcome {
        Outcome::Unavailable(problem) => (problem.clone(), None),
        Outcome::Foreign(host) => (
            format!("ответ получен на домене {host}, его нет в поле «Домены»"),
            Some(host.clone()),
        ),
        Outcome::Passed => {
            sqlx::query("DELETE FROM bot_state WHERE key = $1")
                .bind(HEALTH_KEY)
                .execute(conn.conn())
                .await?;
            return Ok(());
        }
        Outcome::Rejected(_) => return recovered(&mut conn).await,
    };
    let health = Health {
        at: Utc::now(),
        provider,
        problem,
        foreign,
    };
    conn.bot_state()
        .set(HEALTH_KEY, &serde_json::json!(health))
        .await
}

pub async fn recovered(db: &mut impl Db) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM bot_state WHERE key = $1 AND value ->> 'foreign' IS NULL")
        .bind(HEALTH_KEY)
        .execute(db.conn())
        .await?;
    Ok(())
}

fn http_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION")))
}

static HTTP: sn_egress::Profile = sn_egress::Profile {
    name: "captcha",
    build: http_builder,
};

pub fn client() -> sn_egress::Http {
    sn_egress::Http::new(&HTTP, sn_egress::Route::Captcha, None)
}

#[derive(Debug, Clone)]
pub struct Service {
    pub endpoints: Endpoints,
    pub http: sn_egress::Http,
}

impl Service {
    pub fn of(ctx: &ModuleContext) -> Self {
        Service {
            endpoints: ctx.core.captcha_endpoints(),
            http: ctx.core.captcha_http(),
        }
    }
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
    service: &Service,
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
        Provider::Turnstile => turnstile(service, captcha, token, ip, expect).await,
        Provider::Yandex => yandex(service, captcha, token, ip).await,
        Provider::None => Outcome::Unavailable("капча не настроена".into()),
    };
    match &outcome {
        Outcome::Unavailable(problem) => {
            tracing::warn!(provider = captcha.provider.as_str(), %problem, "капча: проверка не выполнена");
        }
        Outcome::Foreign(host) => {
            tracing::warn!(provider = captcha.provider.as_str(), %host, "капча: ответ получен на чужом домене");
        }
        Outcome::Passed | Outcome::Rejected(_) => {}
    }
    if let Err(err) = remember(store, captcha.provider, &outcome).await {
        tracing::warn!(error = %err, "капча: состояние сервиса проверки не сохранено");
    }
    Ok(outcome)
}

fn transport(err: &reqwest::Error, via_proxy: bool) -> String {
    let reason = if err.is_timeout() {
        "сервис проверки не ответил вовремя"
    } else if err.is_connect() {
        "не удалось соединиться с сервисом проверки"
    } else {
        "сервис проверки недоступен"
    };
    if via_proxy {
        format!("{reason} через исходящий прокси")
    } else {
        reason.into()
    }
}

async fn ask<T: serde::de::DeserializeOwned>(
    service: &Service,
    provider: Provider,
    form: &[(&str, String)],
) -> Result<(reqwest::StatusCode, T), String> {
    let response = service
        .http
        .post(service.endpoints.of(provider))
        .form(form)
        .send()
        .await
        .map_err(|err| transport(&err, service.http.proxy().is_some()))?;
    let status = response.status();
    match response.json().await {
        Ok(reply) => Ok((status, reply)),
        Err(_) => Err(format!("сервис проверки ответил HTTP {}", status.as_u16())),
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
    #[serde(default)]
    hostname: Option<String>,
}

fn bad_secret(errors: &[String]) -> bool {
    errors
        .iter()
        .any(|e| matches!(e.as_str(), "missing-input-secret" | "invalid-input-secret"))
}

fn broken(errors: &[String]) -> bool {
    errors.is_empty() || errors.iter().any(|e| e == "internal-error")
}

fn passed_on(captcha: &Captcha, host: Option<&str>) -> Outcome {
    let host = host.unwrap_or_default();
    if covered(&captcha.domains, host) {
        return Outcome::Passed;
    }
    Outcome::Foreign(clean_host(host).unwrap_or_else(|| "без имени".into()))
}

async fn turnstile(
    service: &Service,
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
    let reply: TurnstileReply = match ask(service, Provider::Turnstile, &form).await {
        Ok((_, reply)) => reply,
        Err(problem) => return Outcome::Unavailable(problem),
    };
    let errors = reply.errors.unwrap_or_default();
    if !reply.success {
        if bad_secret(&errors) {
            return Outcome::Unavailable(BAD_SECRET.into());
        }
        if broken(&errors) {
            return Outcome::Unavailable(BROKEN.into());
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
    if test_keys {
        return Outcome::Passed;
    }
    passed_on(captcha, reply.hostname.as_deref())
}

#[derive(Deserialize)]
struct YandexReply {
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    host: Option<String>,
}

impl YandexReply {
    fn bad_secret(&self) -> bool {
        let message = self.message.as_deref().unwrap_or_default().to_lowercase();
        self.status == "failed"
            && (message.contains("secret") || message.contains("authentication"))
    }
}

async fn yandex(service: &Service, captcha: &Captcha, token: &str, ip: Option<&str>) -> Outcome {
    let mut form = vec![
        ("secret", captcha.secret.clone()),
        ("token", token.to_string()),
    ];
    if let Some(ip) = ip {
        form.push(("ip", ip.to_string()));
    }
    let (status, reply): (_, YandexReply) = match ask(service, Provider::Yandex, &form).await {
        Ok(answer) => answer,
        Err(problem) => return Outcome::Unavailable(problem),
    };
    if reply.bad_secret() {
        return Outcome::Unavailable(BAD_SECRET.into());
    }
    if status != reqwest::StatusCode::OK {
        return Outcome::Unavailable(format!("сервис проверки ответил HTTP {}", status.as_u16()));
    }
    match reply.status.as_str() {
        "ok" if reply.host.as_deref().is_some_and(|h| !h.trim().is_empty()) => {
            passed_on(captcha, reply.host.as_deref())
        }
        "ok" => Outcome::Unavailable(WAVED.into()),
        "failed" => Outcome::Rejected("failed".into()),
        _ => Outcome::Unavailable(UNCLEAR.into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Accepted,
    Refused,
    Test,
    Unavailable(String),
}

pub async fn probe(service: &Service, provider: Provider, secret: &str) -> Probe {
    match provider {
        Provider::Turnstile if TURNSTILE_TEST_SECRETS.contains(&secret) => Probe::Test,
        Provider::Turnstile => {
            let form = [
                ("secret", secret.to_string()),
                ("response", PROBE_TOKEN.to_string()),
            ];
            let reply: TurnstileReply = match ask(service, provider, &form).await {
                Ok((_, reply)) => reply,
                Err(problem) => return Probe::Unavailable(problem),
            };
            let errors = reply.errors.unwrap_or_default();
            if reply.success {
                Probe::Unavailable(UNCLEAR.into())
            } else if bad_secret(&errors) {
                Probe::Refused
            } else if errors.iter().any(|e| {
                matches!(
                    e.as_str(),
                    "invalid-input-response" | "timeout-or-duplicate"
                )
            }) {
                Probe::Accepted
            } else {
                Probe::Unavailable(BROKEN.into())
            }
        }
        Provider::Yandex => {
            let form = [
                ("secret", secret.to_string()),
                ("token", PROBE_TOKEN.to_string()),
            ];
            let (status, reply): (_, YandexReply) = match ask(service, provider, &form).await {
                Ok(answer) => answer,
                Err(problem) => return Probe::Unavailable(problem),
            };
            if reply.bad_secret() {
                Probe::Refused
            } else if status != reqwest::StatusCode::OK {
                Probe::Unavailable(format!("сервис проверки ответил HTTP {}", status.as_u16()))
            } else if reply.status == "failed" {
                Probe::Accepted
            } else {
                Probe::Unavailable(WAVED.into())
            }
        }
        Provider::None => Probe::Unavailable("капча не выбрана".into()),
    }
}
