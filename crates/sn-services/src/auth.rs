use crate::error::{ServiceError, ServiceResult};
use crate::limits::{RateRule, Strike, address_key, ip_key, network_key, rules};
use crate::services::Services;
use crate::totp;
use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use chrono::{DateTime, Duration, TimeZone, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sn_domain::access::{AdminPrincipal, AdminRole, ClientChannel, ClientPrincipal};
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::{AdminId, ClientId};
use sn_store::{AdminRecord, Db, NewAdmin, NewSession, SealedSecret, SessionChannel, SessionScope};
use std::net::IpAddr;

pub const ADMIN_SESSION_DAYS: i64 = 14;
pub const CLIENT_SESSION_DAYS: i64 = 30;
pub const INIT_DATA_MAX_AGE_SECONDS: i64 = 3_600;
pub const MIN_PASSWORD_LEN: usize = 10;
pub const MAX_PASSWORD_LEN: usize = 256;
pub const KNOWN_ADDRESS_DAYS: i64 = 30;
pub const TOTP_REQUIRED: &str = "auth.totp_required";
pub const TOTP_INVALID: &str = "auth.totp_invalid";
pub const TOTP_LOCKED_AUDIT: &str = "admin.totp_locked";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestMeta {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdminLogin {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub principal: AdminPrincipal,
    pub must_change_password: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientLogin {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub principal: ClientPrincipal,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdminSecurity {
    pub email: String,
    pub totp_enabled: bool,
    pub must_change_password: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoginUnlock {
    pub admin_id: AdminId,
    pub email: String,
    pub password_failures: u32,
    pub password_locked: bool,
    pub totp_locked: bool,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TotpSetup {
    pub secret: String,
    pub uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramInitData {
    pub user: TelegramProfile,
    pub auth_date: DateTime<Utc>,
    pub start_param: Option<String>,
    pub query_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InitDataError {
    #[error("в initData нет подписи")]
    MissingHash,
    #[error("подпись initData не совпала")]
    BadHash,
    #[error("initData устарели")]
    Expired,
    #[error("initData не разобраны: {0}")]
    Malformed(String),
}

pub fn hash_password(password: &str) -> ServiceResult<String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ServiceError::rejected(
            "auth.weak_password",
            format!("пароль должен быть не короче {MIN_PASSWORD_LEN} символов"),
        ));
    }
    if password.chars().count() > MAX_PASSWORD_LEN {
        return Err(ServiceError::rejected(
            "auth.weak_password",
            format!("пароль должен быть не длиннее {MAX_PASSWORD_LEN} символов"),
        ));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| ServiceError::Internal(e.to_string()))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    if hash.starts_with("$argon2") {
        PasswordHash::new(hash)
            .map(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
            .unwrap_or(false)
    } else if hash.starts_with("$2a$") || hash.starts_with("$2b$") || hash.starts_with("$2y$") {
        bcrypt::verify(password, hash).unwrap_or(false)
    } else {
        false
    }
}

pub fn new_token() -> (String, Vec<u8>) {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let token = hex::encode(raw);
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.trim().as_bytes()).to_vec()
}

pub fn admin_totp_context(id: AdminId) -> String {
    format!("admins.{}.totp", id.0)
}

const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$ZmFrZWZha2VmYWtlZmFrZQ";

pub async fn check_password(password: &str, hash: Option<String>) -> ServiceResult<bool> {
    let password = password.to_string();
    tokio::task::spawn_blocking(move || match hash {
        Some(hash) => verify_password(&password, &hash),
        None => {
            verify_password(&password, DUMMY_HASH);
            false
        }
    })
    .await
    .map_err(|e| ServiceError::Internal(e.to_string()))
}

pub async fn hash_password_async(password: &str) -> ServiceResult<String> {
    let password = password.to_string();
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| ServiceError::Internal(e.to_string()))?
}

const ADDRESS_LOCKED: &str = "Слишком много неверных попыток входа.";
const ACCOUNT_LOCKED: &str = "Слишком много неверных попыток входа: для адресов, с которых вы недавно не входили, вход временно закрыт.";
const TOTP_LOCKED: &str =
    "Код из приложения слишком часто не подходил, вход по коду временно закрыт.";

fn account_identity(email: &str, ip: Option<&str>) -> String {
    format!("{}\n{}", email.trim().to_lowercase(), address_key(ip))
}

fn totp_invalid() -> ServiceError {
    ServiceError::rejected(TOTP_INVALID, "Код не подошёл. Проверьте время на телефоне")
}

fn wrong_password() -> ServiceError {
    ServiceError::rejected("auth.wrong_password", "Пароль не подошёл")
}

fn percent_decode(input: &str) -> Result<String, InitDataError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or_else(|| InitDataError::Malformed("неверное %-кодирование".into()))?;
                out.push(hex);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| InitDataError::Malformed("не UTF-8".into()))
}

fn webapp_secret(bot_token: &str) -> Option<Hmac<Sha256>> {
    let mut key = Hmac::<Sha256>::new_from_slice(b"WebAppData").ok()?;
    key.update(bot_token.as_bytes());
    Hmac::<Sha256>::new_from_slice(&key.finalize().into_bytes()).ok()
}

pub fn sign_init_data(pairs: &[(&str, &str)], bot_token: &str) -> String {
    let mut sorted: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
    sorted.sort();
    match webapp_secret(bot_token) {
        Some(mut mac) => {
            mac.update(sorted.join("\n").as_bytes());
            hex::encode(mac.finalize().into_bytes())
        }
        None => String::new(),
    }
}

#[derive(Deserialize)]
struct InitUser {
    id: i64,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    language_code: Option<String>,
    #[serde(default)]
    is_premium: bool,
}

pub fn verify_init_data(
    init_data: &str,
    bot_token: &str,
    now: DateTime<Utc>,
    max_age: Duration,
) -> Result<TelegramInitData, InitDataError> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for part in init_data.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        let key = percent_decode(key)?;
        if pairs.iter().any(|(k, _)| *k == key) {
            return Err(InitDataError::Malformed(format!("ключ {key} повторяется")));
        }
        pairs.push((key, percent_decode(value)?));
    }
    let hash = pairs
        .iter()
        .find(|(k, _)| k == "hash")
        .map(|(_, v)| v.clone())
        .ok_or(InitDataError::MissingHash)?;
    let expected = hex::decode(hash.trim()).map_err(|_| InitDataError::BadHash)?;
    let mut check: Vec<String> = pairs
        .iter()
        .filter(|(k, _)| k != "hash")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    check.sort();
    let mut mac = webapp_secret(bot_token).ok_or(InitDataError::BadHash)?;
    mac.update(check.join("\n").as_bytes());
    mac.verify_slice(&expected)
        .map_err(|_| InitDataError::BadHash)?;
    let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let auth_date = value("auth_date")
        .and_then(|v| v.parse::<i64>().ok())
        .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
        .ok_or_else(|| InitDataError::Malformed("нет auth_date".into()))?;
    if now - auth_date > max_age || auth_date - now > Duration::seconds(60) {
        return Err(InitDataError::Expired);
    }
    let user: InitUser = value("user")
        .ok_or_else(|| InitDataError::Malformed("нет user".into()))
        .and_then(|raw| {
            serde_json::from_str(&raw).map_err(|e| InitDataError::Malformed(e.to_string()))
        })?;
    Ok(TelegramInitData {
        user: TelegramProfile {
            id: user.id,
            username: user.username,
            first_name: user.first_name,
            last_name: user.last_name,
            language_code: user.language_code,
            is_premium: user.is_premium,
        },
        auth_date,
        start_param: value("start_param"),
        query_id: value("query_id"),
    })
}

pub struct Auth<'a> {
    pub(crate) services: &'a Services,
}

impl<'a> Auth<'a> {
    pub fn new(services: &'a Services) -> Self {
        Auth { services }
    }

    pub async fn upsert_owner(
        &self,
        email: &str,
        password: &str,
    ) -> ServiceResult<(AdminId, bool)> {
        let hash = hash_password(password)?;
        let mut tx = self.services.store().begin().await?;
        let existing = tx.admins().by_email(email).await?;
        let result = match existing {
            Some(admin) => {
                tx.admins().set_password(admin.id, &hash, false).await?;
                tx.admins()
                    .set_role(admin.id, AdminRole::Owner, &[])
                    .await?;
                tx.admins().set_totp(admin.id, None).await?;
                tx.sessions().revoke_all_admin(admin.id).await?;
                (admin.id, false)
            }
            None => {
                let admin = tx
                    .admins()
                    .create(&NewAdmin {
                        email: email.trim().to_string(),
                        password_hash: hash,
                        role: AdminRole::Owner,
                        must_change_password: false,
                    })
                    .await?;
                (admin.id, true)
            }
        };
        tx.commit().await?;
        self.forget_totp_failures(result.0).await?;
        Ok(result)
    }

    pub async fn unlock_admin_login(
        &self,
        email: &str,
        addresses: &[IpAddr],
    ) -> ServiceResult<LoginUnlock> {
        let admin = {
            let mut conn = self.services.store().conn().await?;
            conn.admins().by_email(email).await?
        };
        let Some(admin) = admin else {
            return Err(ServiceError::rejected(
                "auth.unknown_admin",
                format!("Администратора с почтой {} нет", email.trim()),
            ));
        };
        if admin.disabled_at.is_some() {
            return Err(ServiceError::rejected(
                "auth.admin_disabled",
                format!(
                    "Администратор {} отключён: войти он сможет, когда его включат в «Настройки → Администраторы»",
                    admin.email
                ),
            ));
        }
        let limits = self.services.limits();
        let id = admin.id.to_string();
        let password_failures = limits
            .failures(rules::ADMIN_LOGIN_ACCOUNT, &admin.email)
            .await?;
        let totp_locked = limits.failures(rules::ADMIN_TOTP_FAILURES, &id).await?
            >= rules::ADMIN_TOTP_FAILURES.limit
            || limits.failures(rules::ADMIN_TOTP_FAILURES_DAY, &id).await?
                >= rules::ADMIN_TOTP_FAILURES_DAY.limit;
        limits
            .forget(rules::ADMIN_LOGIN_ACCOUNT, &admin.email)
            .await?;
        self.forget_totp_failures(admin.id).await?;
        let mut networks: Vec<String> = Vec::new();
        for ip in addresses {
            let raw = ip.to_string();
            limits
                .forget(
                    rules::ADMIN_LOGIN_ACCOUNT_IP,
                    &account_identity(&admin.email, Some(&raw)),
                )
                .await?;
            limits
                .forget(rules::ADMIN_LOGIN_IP, &address_key(Some(&raw)))
                .await?;
            let network = ip_key(*ip);
            if !networks.contains(&network) {
                networks.push(network);
            }
        }
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        for network in &networks {
            tx.admins()
                .remember_login_address(
                    admin.id,
                    network,
                    now,
                    now - Duration::days(KNOWN_ADDRESS_DAYS),
                )
                .await?;
        }
        tx.audit()
            .record(&crate::Actor::System.audit(
                "admin.login_unlocked",
                "admin",
                Some(admin.id.0),
                serde_json::json!({
                    "password_failures": password_failures,
                    "totp_locked": totp_locked,
                    "addresses": networks,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(LoginUnlock {
            admin_id: admin.id,
            email: admin.email,
            password_failures,
            password_locked: password_failures >= rules::ADMIN_LOGIN_ACCOUNT.limit,
            totp_locked,
            addresses: networks,
        })
    }

    pub(crate) async fn forgive(&self, strikes: &[Strike]) -> ServiceResult<()> {
        let limits = self.services.limits();
        for strike in strikes {
            limits.forgive(strike).await?;
        }
        Ok(())
    }

    pub(crate) async fn strike_both(
        &self,
        first: (RateRule, &str),
        second: (RateRule, &str),
        reason: &str,
    ) -> ServiceResult<[Strike; 2]> {
        let limits = self.services.limits();
        let one = limits.strike(first.0, first.1, reason).await?;
        match limits.strike(second.0, second.1, reason).await {
            Ok(two) => Ok([one, two]),
            Err(err) => {
                limits.forgive(&one).await?;
                Err(err)
            }
        }
    }

    async fn known_address(&self, email: &str, network: Option<&str>) -> ServiceResult<bool> {
        let Some(network) = network else {
            return Ok(false);
        };
        let since = self.services.now() - Duration::days(KNOWN_ADDRESS_DAYS);
        let mut conn = self.services.store().conn().await?;
        Ok(conn.admins().logged_in_from(email, network, since).await?)
    }

    async fn password_attempt(
        &self,
        email: &str,
        meta: &RequestMeta,
        known_address: bool,
    ) -> ServiceResult<Vec<Strike>> {
        let limits = self.services.limits();
        let pair = account_identity(email, meta.ip.as_deref());
        let own = limits
            .strike(rules::ADMIN_LOGIN_ACCOUNT_IP, &pair, ADDRESS_LOCKED)
            .await?;
        if known_address {
            return Ok(vec![own]);
        }
        match limits
            .strike(rules::ADMIN_LOGIN_ACCOUNT, email, ACCOUNT_LOCKED)
            .await
        {
            Ok(shared) => Ok(vec![own, shared]),
            Err(err) => {
                limits.forgive(&own).await?;
                Err(err)
            }
        }
    }

    async fn totp_attempt(&self, admin_id: AdminId) -> ServiceResult<[Strike; 2]> {
        let id = admin_id.to_string();
        self.strike_both(
            (rules::ADMIN_TOTP_FAILURES, &id),
            (rules::ADMIN_TOTP_FAILURES_DAY, &id),
            TOTP_LOCKED,
        )
        .await
    }

    async fn settle_totp(
        &self,
        admin_id: AdminId,
        strikes: &[Strike],
        accepted: bool,
        ip: Option<&str>,
    ) -> ServiceResult<()> {
        if accepted {
            return self.forgive(strikes).await;
        }
        if let Some(strike) = strikes.iter().find(|s| s.reached_limit()) {
            tracing::warn!(
                admin_id = admin_id.0,
                ip = ip.unwrap_or("unknown"),
                failures = strike.failures,
                rule = strike.rule.scope,
                "вход администратора по коду временно закрыт: серия неверных кодов при верном пароле"
            );
            let mut entry = crate::Actor::System.audit(
                TOTP_LOCKED_AUDIT,
                "admin",
                Some(admin_id.0),
                serde_json::json!({
                    "failures": strike.failures,
                    "rule": strike.rule.scope,
                    "window_seconds": strike.rule.window_seconds,
                }),
            );
            entry.ip = ip.map(str::to_string);
            let mut conn = self.services.store().conn().await?;
            conn.audit().record(&entry).await?;
        }
        Ok(())
    }

    async fn forget_totp_failures(&self, admin_id: AdminId) -> ServiceResult<()> {
        let limits = self.services.limits();
        let id = admin_id.to_string();
        limits.forget(rules::ADMIN_TOTP_FAILURES, &id).await?;
        limits.forget(rules::ADMIN_TOTP_FAILURES_DAY, &id).await
    }

    pub async fn admin_login(
        &self,
        email: &str,
        password: &str,
        totp_code: Option<&str>,
        meta: &RequestMeta,
    ) -> ServiceResult<AdminLogin> {
        self.services
            .limits()
            .hit_ip(rules::ADMIN_LOGIN_IP, meta.ip.as_deref())
            .await?;
        let network = network_key(meta.ip.as_deref());
        let known_address = self.known_address(email, network.as_deref()).await?;
        let attempt = self.password_attempt(email, meta, known_address).await?;
        let admin = {
            let mut conn = self.services.store().conn().await?;
            conn.admins()
                .by_email(email)
                .await?
                .filter(|a| a.disabled_at.is_none())
        };
        let valid =
            check_password(password, admin.as_ref().map(|a| a.password_hash.clone())).await?;
        let Some(admin) = admin.filter(|_| valid) else {
            return Err(ServiceError::Unauthorized);
        };
        self.forgive(&attempt).await?;
        if let Some(sealed) = &admin.totp {
            let Some(code) = totp_code.map(str::trim).filter(|c| !c.is_empty()) else {
                return Err(ServiceError::rejected(
                    TOTP_REQUIRED,
                    "Введите код из приложения-аутентификатора",
                ));
            };
            let secret = self.open_admin_totp(admin.id, sealed)?;
            let attempt = self.totp_attempt(admin.id).await?;
            let accepted = match totp::verify(&secret, code, self.services.now().timestamp()) {
                Some(step) => {
                    let mut conn = self.services.store().conn().await?;
                    conn.admins().use_totp_step(admin.id, step).await?
                }
                None => false,
            };
            self.settle_totp(admin.id, &attempt, accepted, meta.ip.as_deref())
                .await?;
            if !accepted {
                return Err(totp_invalid());
            }
        }
        let now = self.services.now();
        let (token, hash) = new_token();
        let expires_at = now + Duration::days(ADMIN_SESSION_DAYS);
        let mut tx = self.services.store().begin().await?;
        let session_id = tx
            .sessions()
            .create_admin(
                admin.id,
                &NewSession {
                    token_hash: hash,
                    expires_at,
                    ip: meta.ip.clone(),
                    user_agent: meta.user_agent.clone(),
                },
            )
            .await?;
        if let Some(network) = &network {
            tx.admins()
                .remember_login_address(
                    admin.id,
                    network,
                    now,
                    now - Duration::days(KNOWN_ADDRESS_DAYS),
                )
                .await?;
        }
        tx.commit().await?;
        Ok(AdminLogin {
            token,
            expires_at,
            principal: AdminPrincipal {
                admin_id: admin.id,
                role: admin.role,
                permissions: admin.permissions,
                session_id,
                api_key: None,
            },
            must_change_password: admin.must_change_password,
        })
    }

    fn open_admin_totp(&self, id: AdminId, sealed: &SealedSecret) -> ServiceResult<String> {
        let secrets = self.services.secrets().ok_or(ServiceError::NoSecretKey)?;
        Ok(secrets.open_string(&admin_totp_context(id), sealed)?)
    }

    async fn admin_for_security(&self, admin_id: AdminId) -> ServiceResult<AdminRecord> {
        self.services
            .limits()
            .hit(rules::ADMIN_SECURITY, &admin_id.to_string())
            .await?;
        let mut conn = self.services.store().conn().await?;
        let admin = conn.admins().get(admin_id).await?;
        if admin.disabled_at.is_some() {
            return Err(ServiceError::Unauthorized);
        }
        Ok(admin)
    }

    pub async fn admin_totp_setup(&self, admin_id: AdminId) -> ServiceResult<TotpSetup> {
        let mut conn = self.services.store().conn().await?;
        let admin = conn.admins().get(admin_id).await?;
        if admin.totp.is_some() {
            return Err(ServiceError::rejected(
                "auth.totp_enabled",
                "Вход по коду уже включён",
            ));
        }
        let secret = totp::generate_secret();
        let uri =
            totp::provisioning_uri(&secret, &admin.email, &self.services.settings().brand_name);
        Ok(TotpSetup { secret, uri })
    }

    pub async fn admin_totp_enable(
        &self,
        admin_id: AdminId,
        secret: &str,
        code: &str,
        password: &str,
    ) -> ServiceResult<()> {
        let admin = self.admin_for_security(admin_id).await?;
        if admin.totp.is_some() {
            return Err(ServiceError::rejected(
                "auth.totp_enabled",
                "Вход по коду уже включён",
            ));
        }
        if !check_password(password, Some(admin.password_hash.clone())).await? {
            return Err(wrong_password());
        }
        let secret: String = secret
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        if totp::base32_decode(&secret).is_none_or(|key| key.len() < totp::MIN_SECRET_BYTES) {
            return Err(ServiceError::rejected(
                "auth.totp_secret",
                "Секрет повреждён, начните настройку заново",
            ));
        }
        let step =
            totp::verify(&secret, code, self.services.now().timestamp()).ok_or_else(|| {
                ServiceError::rejected(
                    "auth.totp_code",
                    "Код не подошёл. Проверьте время на телефоне",
                )
            })?;
        let secrets = self.services.secrets().ok_or(ServiceError::NoSecretKey)?;
        let sealed = secrets.seal_str(&admin_totp_context(admin_id), &secret)?;
        let mut tx = self.services.store().begin().await?;
        tx.admins().set_totp(admin_id, Some(&sealed)).await?;
        tx.admins().use_totp_step(admin_id, step).await?;
        tx.audit()
            .record(&crate::Actor::Admin(admin_id).audit(
                "admin.totp_enabled",
                "admin",
                Some(admin_id.0),
                serde_json::json!({}),
            ))
            .await?;
        tx.commit().await?;
        self.forget_totp_failures(admin_id).await
    }

    pub async fn admin_totp_disable(
        &self,
        admin_id: AdminId,
        password: &str,
        code: &str,
    ) -> ServiceResult<()> {
        let admin = self.admin_for_security(admin_id).await?;
        let Some(sealed) = admin.totp.clone() else {
            return Err(ServiceError::rejected(
                "auth.totp_disabled",
                "Вход по коду не включён",
            ));
        };
        if !check_password(password, Some(admin.password_hash.clone())).await? {
            return Err(wrong_password());
        }
        let secret = self.open_admin_totp(admin_id, &sealed)?;
        let attempt = self.totp_attempt(admin_id).await?;
        let Some(step) = totp::verify(&secret, code, self.services.now().timestamp()) else {
            self.settle_totp(admin_id, &attempt, false, None).await?;
            return Err(ServiceError::rejected(
                "auth.totp_code",
                "Код не подошёл. Проверьте время на телефоне",
            ));
        };
        let mut tx = self.services.store().begin().await?;
        if !tx.admins().use_totp_step(admin_id, step).await? {
            drop(tx);
            self.settle_totp(admin_id, &attempt, false, None).await?;
            return Err(ServiceError::rejected(
                "auth.totp_code",
                "Этот код уже использован, дождитесь следующего",
            ));
        }
        tx.admins().set_totp(admin_id, None).await?;
        tx.audit()
            .record(&crate::Actor::Admin(admin_id).audit(
                "admin.totp_disabled",
                "admin",
                Some(admin_id.0),
                serde_json::json!({}),
            ))
            .await?;
        tx.commit().await?;
        self.forget_totp_failures(admin_id).await
    }

    pub async fn change_admin_password(
        &self,
        admin_id: AdminId,
        current: &str,
        new_password: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<AdminLogin> {
        let admin = self.admin_for_security(admin_id).await?;
        if !check_password(current, Some(admin.password_hash.clone())).await? {
            return Err(wrong_password());
        }
        if current == new_password {
            return Err(ServiceError::rejected(
                "auth.same_password",
                "Новый пароль совпадает со старым",
            ));
        }
        let hash = hash_password_async(new_password).await?;
        let (token, token_hash) = new_token();
        let expires_at = self.services.now() + Duration::days(ADMIN_SESSION_DAYS);
        let mut tx = self.services.store().begin().await?;
        tx.admins().set_password(admin_id, &hash, false).await?;
        tx.sessions().revoke_all_admin(admin_id).await?;
        let session_id = tx
            .sessions()
            .create_admin(
                admin_id,
                &NewSession {
                    token_hash,
                    expires_at,
                    ip: meta.ip.clone(),
                    user_agent: meta.user_agent.clone(),
                },
            )
            .await?;
        tx.audit()
            .record(&crate::Actor::Admin(admin_id).audit(
                "admin.password_changed",
                "admin",
                Some(admin_id.0),
                serde_json::json!({}),
            ))
            .await?;
        tx.commit().await?;
        self.forget_totp_failures(admin_id).await?;
        Ok(AdminLogin {
            token,
            expires_at,
            principal: AdminPrincipal {
                admin_id,
                role: admin.role,
                permissions: admin.permissions,
                session_id,
                api_key: None,
            },
            must_change_password: false,
        })
    }

    pub async fn confirm_admin(
        &self,
        admin_id: AdminId,
        password: &str,
        code: Option<&str>,
    ) -> ServiceResult<()> {
        let admin = self.admin_for_security(admin_id).await?;
        if !check_password(password, Some(admin.password_hash.clone())).await? {
            return Err(wrong_password());
        }
        let Some(sealed) = admin.totp.clone() else {
            return Ok(());
        };
        let Some(code) = code.map(str::trim).filter(|c| !c.is_empty()) else {
            return Err(ServiceError::rejected(
                TOTP_REQUIRED,
                "Введите код из приложения-аутентификатора",
            ));
        };
        let secret = self.open_admin_totp(admin_id, &sealed)?;
        let attempt = self.totp_attempt(admin_id).await?;
        let accepted = match totp::verify(&secret, code, self.services.now().timestamp()) {
            Some(step) => {
                let mut conn = self.services.store().conn().await?;
                conn.admins().use_totp_step(admin_id, step).await?
            }
            None => false,
        };
        self.settle_totp(admin_id, &attempt, accepted, None).await?;
        if accepted {
            Ok(())
        } else {
            Err(totp_invalid())
        }
    }

    pub async fn admin_security(&self, admin_id: AdminId) -> ServiceResult<AdminSecurity> {
        let mut conn = self.services.store().conn().await?;
        let admin = conn.admins().get(admin_id).await?;
        Ok(AdminSecurity {
            email: admin.email,
            totp_enabled: admin.totp.is_some(),
            must_change_password: admin.must_change_password,
        })
    }

    pub async fn admin_session(&self, token: &str) -> ServiceResult<AdminPrincipal> {
        let mut conn = self.services.store().conn().await?;
        let session = conn
            .sessions()
            .admin_by_token(&token_hash(token))
            .await?
            .ok_or(ServiceError::Unauthorized)?;
        let admin = conn.admins().get(session.admin_id).await?;
        if self.services.now() - session.last_seen_at > Duration::minutes(5) {
            conn.sessions().touch_admin(session.id).await?;
        }
        Ok(AdminPrincipal {
            admin_id: admin.id,
            role: admin.role,
            permissions: admin.permissions,
            session_id: session.id,
            api_key: None,
        })
    }

    pub async fn admin_logout(&self, token: &str) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.sessions().revoke_admin(&token_hash(token)).await?;
        Ok(())
    }

    pub async fn client_logout(&self, token: &str) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.sessions().revoke_client(&token_hash(token)).await?;
        Ok(())
    }

    pub async fn miniapp_login(
        &self,
        init_data: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<ClientLogin> {
        if !self.services.settings().client_access.miniapp {
            return Err(ServiceError::rejected(
                "auth.miniapp_off",
                "Мини-приложение выключено: пользуйтесь ботом",
            ));
        }
        self.services
            .limits()
            .hit_ip(rules::MINIAPP_IP, meta.ip.as_deref())
            .await?;
        let bot_token = self
            .services
            .bot_token()
            .ok_or_else(|| ServiceError::rejected("bot.not_configured", "бот не настроен"))?;
        let data = verify_init_data(
            init_data,
            &bot_token,
            self.services.now(),
            Duration::seconds(INIT_DATA_MAX_AGE_SECONDS),
        )
        .map_err(|e| ServiceError::rejected("auth.init_data", e.to_string()))?;
        let (client, created) = self
            .services
            .clients()
            .ensure_telegram(
                &data.user,
                RegistrationSource::Miniapp,
                data.start_param.as_deref(),
            )
            .await?;
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        self.client_session_for(client.id, SessionChannel::Miniapp, meta, created)
            .await
    }

    pub async fn client_session_for(
        &self,
        client_id: ClientId,
        channel: SessionChannel,
        meta: &RequestMeta,
        created: bool,
    ) -> ServiceResult<ClientLogin> {
        let (token, hash) = new_token();
        let expires_at = self.services.now() + Duration::days(CLIENT_SESSION_DAYS);
        let mut conn = self.services.store().conn().await?;
        let session_id = conn
            .sessions()
            .create_client(
                client_id,
                channel,
                SessionScope::Full,
                &NewSession {
                    token_hash: hash,
                    expires_at,
                    ip: meta.ip.clone(),
                    user_agent: meta.user_agent.clone(),
                },
            )
            .await?;
        Ok(ClientLogin {
            token,
            expires_at,
            principal: ClientPrincipal {
                client_id,
                channel: match channel {
                    SessionChannel::Cabinet => ClientChannel::Cabinet,
                    SessionChannel::Miniapp => ClientChannel::Miniapp,
                },
                full_access: true,
                session_id: Some(session_id),
            },
            created,
        })
    }

    pub async fn client_session(&self, token: &str) -> ServiceResult<ClientPrincipal> {
        let mut conn = self.services.store().conn().await?;
        let session = conn
            .sessions()
            .client_by_token(&token_hash(token))
            .await?
            .ok_or(ServiceError::Unauthorized)?;
        let client = conn.clients().get(session.client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let access = self.services.settings().client_access;
        let open = match session.channel {
            SessionChannel::Cabinet => access.site_login,
            SessionChannel::Miniapp => access.miniapp,
        };
        if !open {
            return Err(ServiceError::Unauthorized);
        }
        Ok(ClientPrincipal {
            client_id: session.client_id,
            channel: match session.channel {
                SessionChannel::Cabinet => ClientChannel::Cabinet,
                SessionChannel::Miniapp => ClientChannel::Miniapp,
            },
            full_access: session.scope == SessionScope::Full,
            session_id: Some(session.id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOT: &str = "123456:TEST-token";

    fn init_data(auth_date: i64, tamper: bool) -> String {
        let user = r#"{"id":5093118422,"first_name":"Алексей","username":"morozov_a","language_code":"ru"}"#;
        let auth = auth_date.to_string();
        let pairs = [
            ("auth_date", auth.as_str()),
            ("query_id", "AAE1"),
            ("user", user),
        ];
        let hash = sign_init_data(&pairs, BOT);
        let encoded_user: String = user
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let auth_value = if tamper { auth_date + 1 } else { auth_date };
        format!("query_id=AAE1&user={encoded_user}&auth_date={auth_value}&hash={hash}")
    }

    #[test]
    fn подпись_мини_приложения_проверяется() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let data = verify_init_data(
            &init_data(1_800_000_000 - 30, false),
            BOT,
            now,
            Duration::hours(1),
        )
        .unwrap();
        assert_eq!(data.user.id, 5_093_118_422);
        assert_eq!(data.user.first_name.as_deref(), Some("Алексей"));
        assert_eq!(
            verify_init_data(
                &init_data(1_800_000_000 - 30, true),
                BOT,
                now,
                Duration::hours(1)
            ),
            Err(InitDataError::BadHash)
        );
        assert_eq!(
            verify_init_data(
                &init_data(1_800_000_000 - 30, false),
                "other:token",
                now,
                Duration::hours(1)
            ),
            Err(InitDataError::BadHash)
        );
        assert_eq!(
            verify_init_data(
                &init_data(1_800_000_000 - 7_200, false),
                BOT,
                now,
                Duration::hours(1)
            ),
            Err(InitDataError::Expired)
        );
        assert_eq!(
            verify_init_data("user=%7B%7D&auth_date=1", BOT, now, Duration::hours(1)),
            Err(InitDataError::MissingHash)
        );
    }

    #[test]
    fn пароли_argon2_и_старые_bcrypt() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong password!", &hash));
        let legacy = bcrypt::hash("legacy-password", 4).unwrap();
        assert!(verify_password("legacy-password", &legacy));
        assert!(!verify_password("legacy-password", "plain"));
        assert!(hash_password("short").is_err());
    }

    #[test]
    fn токен_сессии_хранится_только_хешем() {
        let (token, hash) = new_token();
        assert_eq!(token.len(), 64);
        assert_eq!(hash, token_hash(&token));
        assert_ne!(hash, token.as_bytes());
    }

    const EMAIL: &str = "owner@example.com";
    const PASSWORD: &str = "очень-надёжный-пароль";

    struct Env {
        db: sn_store::testing::TestDb,
        services: Services,
        clock: std::sync::Arc<crate::FixedClock>,
    }

    fn start() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap()
    }

    async fn env() -> Option<Env> {
        let db = sn_store::testing::test_store().await?;
        let clock = std::sync::Arc::new(crate::FixedClock::new(start()));
        let services = Services::new(crate::ServiceDeps {
            store: db.store.clone(),
            payments: std::sync::Arc::new(sn_payments::ProviderRegistry::builtin()),
            modules: std::sync::Arc::new(sn_modules::ModuleRegistry::catalog_only()),
            secrets: Some(
                sn_store::SecretBox::from_base64(&sn_store::SecretBox::generate_key_base64())
                    .unwrap(),
            ),
            clock: clock.clone(),
            env: crate::EnvFallbacks::default(),
        });
        services.reload().await.unwrap();
        Some(Env {
            db,
            services,
            clock,
        })
    }

    fn from(ip: &str) -> RequestMeta {
        RequestMeta {
            ip: Some(ip.to_string()),
            user_agent: None,
        }
    }

    fn code_at(secret: &str, at: DateTime<Utc>) -> String {
        let key = totp::base32_decode(secret).unwrap();
        totp::code_at(
            &key,
            u64::try_from(at.timestamp() / totp::STEP_SECONDS).unwrap(),
            totp::DIGITS,
        )
        .unwrap()
    }

    fn wrong_code(secret: &str, at: DateTime<Utc>) -> String {
        (0..10u32)
            .map(|d| d.to_string().repeat(6))
            .find(|c| totp::verify(secret, c, at.timestamp()).is_none())
            .unwrap()
    }

    fn rejected_code<T: std::fmt::Debug>(result: ServiceResult<T>) -> &'static str {
        result.unwrap_err().code()
    }

    async fn totp_owner(env: &Env) -> (AdminId, String) {
        let auth = env.services.auth();
        let admin = auth.upsert_owner(EMAIL, PASSWORD).await.unwrap().0;
        let setup = auth.admin_totp_setup(admin).await.unwrap();
        auth.admin_totp_enable(
            admin,
            &setup.secret,
            &code_at(&setup.secret, env.services.now()),
            PASSWORD,
        )
        .await
        .unwrap();
        (admin, setup.secret)
    }

    async fn audit_actions(env: &Env, admin: AdminId) -> Vec<String> {
        let mut conn = env.services.store().conn().await.unwrap();
        conn.audit()
            .for_entity("admin", admin.0, 50)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.action)
            .collect()
    }

    #[tokio::test]
    async fn успешные_входы_не_расходуют_лимит_аккаунта() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        let office = from("203.0.113.7");
        for _ in 0..21 {
            auth.admin_login(EMAIL, PASSWORD, None, &office)
                .await
                .unwrap();
        }
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn неверные_пароли_с_чужого_адреса_не_закрывают_вход_владельцу() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        let attacker = from("198.51.100.66");
        let mut unauthorized = 0;
        for _ in 0..21 {
            match auth
                .admin_login(EMAIL, "подобранный-пароль", None, &attacker)
                .await
            {
                Err(ServiceError::Unauthorized) => unauthorized += 1,
                other => assert_eq!(rejected_code(other), crate::limits::RATE_LIMITED),
            }
        }
        assert_eq!(unauthorized, rules::ADMIN_LOGIN_ACCOUNT_IP.limit);
        auth.admin_login(EMAIL, PASSWORD, None, &from("203.0.113.7"))
            .await
            .unwrap();
        assert_eq!(
            rejected_code(auth.admin_login(EMAIL, PASSWORD, None, &attacker).await),
            crate::limits::RATE_LIMITED
        );
        env.clock
            .set(start() + Duration::seconds(2 * rules::ADMIN_LOGIN_ACCOUNT_IP.window_seconds));
        auth.admin_login(EMAIL, PASSWORD, None, &attacker)
            .await
            .unwrap();
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn общий_лимит_аккаунта_закрывает_перебор_с_многих_адресов() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        let limits = env.services.limits();
        for _ in 0..rules::ADMIN_LOGIN_ACCOUNT.limit {
            limits
                .strike(rules::ADMIN_LOGIN_ACCOUNT, EMAIL, "перебор")
                .await
                .unwrap();
        }
        let owner = from("203.0.113.7");
        for _ in 0..rules::ADMIN_LOGIN_ACCOUNT_IP.limit + 2 {
            assert_eq!(
                rejected_code(auth.admin_login(EMAIL, PASSWORD, None, &owner).await),
                crate::limits::RATE_LIMITED
            );
        }
        let pair = limits
            .strike(
                rules::ADMIN_LOGIN_ACCOUNT_IP,
                &account_identity(EMAIL, Some("203.0.113.7")),
                "перебор",
            )
            .await
            .unwrap();
        assert_eq!(pair.failures, 1);
        limits.forgive(&pair).await.unwrap();
        env.clock
            .set(start() + Duration::seconds(2 * rules::ADMIN_LOGIN_ACCOUNT.window_seconds));
        auth.admin_login(EMAIL, PASSWORD, None, &owner)
            .await
            .unwrap();
        env.db.cleanup().await;
    }

    async fn guess_passwords(env: &Env, ip: &str, attempts: u32) -> u32 {
        let auth = env.services.auth();
        let mut unauthorized = 0;
        for _ in 0..attempts {
            match auth
                .admin_login(EMAIL, "подбираемый-пароль", None, &from(ip))
                .await
            {
                Err(ServiceError::Unauthorized) => unauthorized += 1,
                other => assert_eq!(rejected_code(other), crate::limits::RATE_LIMITED),
            }
        }
        unauthorized
    }

    async fn fill_account_limit(env: &Env) {
        let limits = env.services.limits();
        for _ in 0..rules::ADMIN_LOGIN_ACCOUNT.limit {
            limits
                .strike(rules::ADMIN_LOGIN_ACCOUNT, EMAIL, "перебор")
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn перебор_с_десяти_адресов_одной_ipv6_сети_не_закрывает_вход_владельцу() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        let home = from("203.0.113.7");
        auth.admin_login(EMAIL, PASSWORD, None, &home)
            .await
            .unwrap();
        let mut handles = Vec::new();
        for host in 1..=10u32 {
            let services = env.services.clone();
            handles.push(tokio::spawn(async move {
                let meta = from(&format!("2001:db8:1:2::{host:x}"));
                let mut unauthorized = 0;
                for _ in 0..10 {
                    match services
                        .auth()
                        .admin_login(EMAIL, "подбираемый-пароль", None, &meta)
                        .await
                    {
                        Err(ServiceError::Unauthorized) => unauthorized += 1,
                        other => assert_eq!(rejected_code(other), crate::limits::RATE_LIMITED),
                    }
                }
                unauthorized
            }));
        }
        let mut unauthorized = 0;
        for handle in handles {
            unauthorized += handle.await.unwrap();
        }
        assert_eq!(unauthorized, rules::ADMIN_LOGIN_ACCOUNT_IP.limit);
        auth.admin_login(EMAIL, PASSWORD, None, &home)
            .await
            .unwrap();
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn общий_лимит_двадцать_неудач_и_не_действует_на_адрес_недавнего_входа() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        let home = from("203.0.113.7");
        auth.admin_login(EMAIL, PASSWORD, None, &home)
            .await
            .unwrap();
        let mut unauthorized = 0;
        for attacker in ["198.51.100.1", "198.51.100.2", "2001:db8:ffff:1::1"] {
            unauthorized += guess_passwords(&env, attacker, 10).await;
        }
        assert_eq!(unauthorized, 20);
        assert_eq!(rules::ADMIN_LOGIN_ACCOUNT.limit, 20);
        assert_eq!(rules::ADMIN_LOGIN_ACCOUNT.window_seconds, 900);
        let stranger = auth
            .admin_login(EMAIL, PASSWORD, None, &from("192.0.2.50"))
            .await
            .unwrap_err();
        assert_eq!(stranger.code(), crate::limits::RATE_LIMITED);
        assert!(
            stranger
                .to_string()
                .contains("с которых вы недавно не входили"),
            "{stranger}"
        );
        auth.admin_login(EMAIL, PASSWORD, None, &home)
            .await
            .unwrap();
        assert_eq!(
            guess_passwords(&env, "203.0.113.7", 12).await,
            rules::ADMIN_LOGIN_ACCOUNT_IP.limit
        );
        let locked = auth
            .admin_login(EMAIL, PASSWORD, None, &home)
            .await
            .unwrap_err();
        assert_eq!(locked.code(), crate::limits::RATE_LIMITED);
        assert!(
            locked
                .to_string()
                .starts_with("Слишком много неверных попыток входа. Попробуйте снова через"),
            "{locked}"
        );
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn неудачи_с_известного_адреса_не_расходуют_общий_лимит() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        let limits = env.services.limits();
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        auth.admin_login(EMAIL, PASSWORD, None, &from("203.0.113.7"))
            .await
            .unwrap();
        assert_eq!(guess_passwords(&env, "203.0.113.7", 3).await, 3);
        let shared = limits
            .strike(rules::ADMIN_LOGIN_ACCOUNT, EMAIL, "перебор")
            .await
            .unwrap();
        assert_eq!(shared.failures, 1);
        assert_eq!(guess_passwords(&env, "198.51.100.1", 3).await, 3);
        let shared = limits
            .strike(rules::ADMIN_LOGIN_ACCOUNT, EMAIL, "перебор")
            .await
            .unwrap();
        assert_eq!(shared.failures, 5);
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn адрес_запоминается_только_после_полного_входа_с_кодом() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        let (_, secret) = totp_owner(&env).await;
        let office = from("203.0.113.7");
        let home = from("198.51.100.20");
        assert_eq!(
            rejected_code(auth.admin_login(EMAIL, PASSWORD, None, &office).await),
            TOTP_REQUIRED
        );
        let later = start() + Duration::seconds(30);
        env.clock.set(later);
        auth.admin_login(EMAIL, PASSWORD, Some(&code_at(&secret, later)), &home)
            .await
            .unwrap();
        fill_account_limit(&env).await;
        let next = later + Duration::seconds(30);
        env.clock.set(next);
        assert_eq!(
            rejected_code(
                auth.admin_login(EMAIL, PASSWORD, Some(&code_at(&secret, next)), &office)
                    .await
            ),
            crate::limits::RATE_LIMITED
        );
        auth.admin_login(EMAIL, PASSWORD, Some(&code_at(&secret, next)), &home)
            .await
            .unwrap();
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn адрес_входа_помнится_тридцать_дней() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        let home = from("203.0.113.7");
        auth.admin_login(EMAIL, PASSWORD, None, &home)
            .await
            .unwrap();
        let recent = start() + Duration::days(29) + Duration::hours(23);
        env.clock.set(recent);
        fill_account_limit(&env).await;
        auth.admin_login(EMAIL, PASSWORD, None, &home)
            .await
            .unwrap();
        env.clock.set(recent + Duration::days(31));
        fill_account_limit(&env).await;
        assert_eq!(
            rejected_code(auth.admin_login(EMAIL, PASSWORD, None, &home).await),
            crate::limits::RATE_LIMITED
        );
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn известный_адрес_ipv6_сверяется_по_сети_64() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        auth.admin_login(EMAIL, PASSWORD, None, &from("2001:db8:aaaa:1::10"))
            .await
            .unwrap();
        auth.admin_login(EMAIL, PASSWORD, None, &RequestMeta::default())
            .await
            .unwrap();
        fill_account_limit(&env).await;
        auth.admin_login(
            EMAIL,
            PASSWORD,
            None,
            &from("2001:db8:aaaa:1:1234:5678:9abc:def0"),
        )
        .await
        .unwrap();
        assert_eq!(
            rejected_code(
                auth.admin_login(EMAIL, PASSWORD, None, &from("2001:db8:aaaa:2::10"))
                    .await
            ),
            crate::limits::RATE_LIMITED
        );
        assert_eq!(
            rejected_code(
                auth.admin_login(EMAIL, PASSWORD, None, &RequestMeta::default())
                    .await
            ),
            crate::limits::RATE_LIMITED
        );
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn лимиты_по_адресу_считают_ipv6_по_сети_64() {
        let Some(env) = env().await else { return };
        let limits = env.services.limits();
        let rule = rules::TELEGRAM_START_IP;
        for host in 1..=rule.limit {
            assert_eq!(
                limits
                    .hit_ip(rule, Some(&format!("2001:db8:1:2::{host:x}")))
                    .await
                    .unwrap(),
                host
            );
        }
        assert_eq!(
            rejected_code(
                limits
                    .hit_ip(rule, Some("2001:db8:1:2:ffff:ffff:ffff:ffff"))
                    .await
            ),
            crate::limits::RATE_LIMITED
        );
        assert_eq!(
            limits.hit_ip(rule, Some("2001:db8:1:3::1")).await.unwrap(),
            1
        );
        assert_eq!(limits.hit_ip(rule, Some("203.0.113.7")).await.unwrap(), 1);
        assert_eq!(limits.hit_ip(rule, Some("203.0.113.8")).await.unwrap(), 1);
        assert_eq!(
            limits
                .hit_ip(rule, Some("::ffff:203.0.113.7"))
                .await
                .unwrap(),
            2
        );
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn неверные_коды_totp_временно_закрывают_вход_по_коду() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        let (admin, secret) = totp_owner(&env).await;
        let wrong = wrong_code(&secret, start());
        for i in 0..rules::ADMIN_TOTP_FAILURES.limit {
            assert_eq!(
                rejected_code(
                    auth.admin_login(
                        EMAIL,
                        PASSWORD,
                        Some(&wrong),
                        &from(&format!("198.51.100.{i}"))
                    )
                    .await
                ),
                TOTP_INVALID
            );
        }
        let later = start() + Duration::seconds(30);
        env.clock.set(later);
        let locked = auth
            .admin_login(
                EMAIL,
                PASSWORD,
                Some(&code_at(&secret, later)),
                &from("203.0.113.7"),
            )
            .await
            .unwrap_err();
        assert_eq!(locked.code(), crate::limits::RATE_LIMITED);
        assert!(locked.to_string().contains("вход по коду"), "{locked}");
        assert_eq!(
            audit_actions(&env, admin)
                .await
                .iter()
                .filter(|a| *a == "admin.totp_locked")
                .count(),
            1
        );
        assert!(matches!(
            auth.admin_login(
                EMAIL,
                "подобранный-пароль",
                Some(&wrong),
                &from("203.0.113.7")
            )
            .await,
            Err(ServiceError::Unauthorized)
        ));
        let still = start() + Duration::minutes(20);
        env.clock.set(still);
        assert_eq!(
            rejected_code(
                auth.admin_login(
                    EMAIL,
                    PASSWORD,
                    Some(&code_at(&secret, still)),
                    &from("203.0.113.7")
                )
                .await
            ),
            crate::limits::RATE_LIMITED
        );
        let unlocked = start() + Duration::minutes(31);
        env.clock.set(unlocked);
        auth.admin_login(
            EMAIL,
            PASSWORD,
            Some(&code_at(&secret, unlocked)),
            &from("203.0.113.7"),
        )
        .await
        .unwrap();
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn неверные_коды_при_отключении_2fa_идут_в_тот_же_счётчик() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        let (admin, secret) = totp_owner(&env).await;
        let wrong = wrong_code(&secret, start());
        for _ in 0..rules::ADMIN_TOTP_FAILURES.limit {
            assert_eq!(
                rejected_code(auth.admin_totp_disable(admin, PASSWORD, &wrong).await),
                "auth.totp_code"
            );
        }
        let later = start() + Duration::seconds(30);
        env.clock.set(later);
        assert_eq!(
            rejected_code(
                auth.admin_totp_disable(admin, PASSWORD, &code_at(&secret, later))
                    .await
            ),
            crate::limits::RATE_LIMITED
        );
        assert!(auth.admin_security(admin).await.unwrap().totp_enabled);
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn дневной_предел_кодов_и_сброс_при_восстановлении_владельца() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        let (admin, secret) = totp_owner(&env).await;
        let limits = env.services.limits();
        for _ in 0..rules::ADMIN_TOTP_FAILURES_DAY.limit {
            limits
                .strike(
                    rules::ADMIN_TOTP_FAILURES_DAY,
                    &admin.to_string(),
                    "перебор",
                )
                .await
                .unwrap();
        }
        let later = start() + Duration::hours(3);
        env.clock.set(later);
        let locked = auth
            .admin_login(
                EMAIL,
                PASSWORD,
                Some(&code_at(&secret, later)),
                &from("203.0.113.7"),
            )
            .await
            .unwrap_err();
        assert_eq!(locked.code(), crate::limits::RATE_LIMITED);
        assert!(locked.to_string().contains(" ч."), "{locked}");
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        let setup = auth.admin_totp_setup(admin).await.unwrap();
        auth.admin_totp_enable(
            admin,
            &setup.secret,
            &code_at(&setup.secret, later),
            PASSWORD,
        )
        .await
        .unwrap();
        let next = later + Duration::seconds(30);
        env.clock.set(next);
        auth.admin_login(
            EMAIL,
            PASSWORD,
            Some(&code_at(&setup.secret, next)),
            &from("203.0.113.7"),
        )
        .await
        .unwrap();
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn неудачи_считаются_скользящим_окном_и_прощаются() {
        let Some(env) = env().await else { return };
        let limits = env.services.limits();
        let rule = rules::ADMIN_TOTP_FAILURES;
        let mut strikes = Vec::new();
        for n in 1..=rule.limit {
            let strike = limits.strike(rule, "7", "перебор").await.unwrap();
            assert_eq!(strike.failures, n);
            strikes.push(strike);
        }
        assert!(strikes.last().unwrap().reached_limit());
        assert_eq!(
            rejected_code(limits.strike(rule, "7", "перебор").await),
            crate::limits::RATE_LIMITED
        );
        limits.forgive(&strikes[0]).await.unwrap();
        assert_eq!(
            limits.strike(rule, "7", "перебор").await.unwrap().failures,
            rule.limit
        );
        env.clock
            .set(start() + Duration::seconds(rule.window_seconds));
        assert_eq!(
            rejected_code(limits.strike(rule, "7", "перебор").await),
            crate::limits::RATE_LIMITED
        );
        env.clock
            .set(start() + Duration::seconds(2 * rule.window_seconds));
        assert_eq!(
            limits.strike(rule, "7", "перебор").await.unwrap().failures,
            1
        );
        limits.forget(rule, "7").await.unwrap();
        assert_eq!(
            limits.strike(rule, "7", "перебор").await.unwrap().failures,
            1
        );
        assert_eq!(
            limits.strike(rule, "8", "перебор").await.unwrap().failures,
            1
        );
        env.db.cleanup().await;
    }

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        list.iter().map(|ip| ip.parse().unwrap()).collect()
    }

    #[tokio::test]
    async fn команда_снимает_блокировку_входа_с_новых_адресов() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        let admin = auth.upsert_owner(EMAIL, PASSWORD).await.unwrap().0;
        let mut unauthorized = 0;
        for attacker in ["198.51.100.1", "198.51.100.2"] {
            unauthorized += guess_passwords(&env, attacker, 10).await;
        }
        assert_eq!(unauthorized, rules::ADMIN_LOGIN_ACCOUNT.limit);
        let laptop = from("192.0.2.50");
        assert_eq!(
            rejected_code(auth.admin_login(EMAIL, PASSWORD, None, &laptop).await),
            crate::limits::RATE_LIMITED
        );
        let unlock = auth
            .unlock_admin_login("  OWNER@example.com ", &[])
            .await
            .unwrap();
        assert_eq!(unlock.admin_id, admin);
        assert_eq!(unlock.email, EMAIL);
        assert_eq!(unlock.password_failures, rules::ADMIN_LOGIN_ACCOUNT.limit);
        assert!(unlock.password_locked);
        assert!(!unlock.totp_locked);
        assert!(unlock.addresses.is_empty());
        auth.admin_login(EMAIL, PASSWORD, None, &laptop)
            .await
            .unwrap();
        assert!(
            audit_actions(&env, admin)
                .await
                .contains(&"admin.login_unlocked".to_string())
        );
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn адрес_из_команды_не_закрывается_пока_идёт_подбор() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        auth.upsert_owner(EMAIL, PASSWORD).await.unwrap();
        assert_eq!(
            guess_passwords(&env, "203.0.113.7", 12).await,
            rules::ADMIN_LOGIN_ACCOUNT_IP.limit
        );
        let unlock = auth
            .unlock_admin_login(
                EMAIL,
                &ips(&["203.0.113.7", "2001:db8:aaaa:1::10", "2001:db8:aaaa:1::20"]),
            )
            .await
            .unwrap();
        assert_eq!(unlock.addresses, ["203.0.113.7", "2001:db8:aaaa:1::/64"]);
        assert_eq!(unlock.password_failures, 10);
        assert!(!unlock.password_locked);
        fill_account_limit(&env).await;
        auth.admin_login(EMAIL, PASSWORD, None, &from("203.0.113.7"))
            .await
            .unwrap();
        auth.admin_login(EMAIL, PASSWORD, None, &from("2001:db8:aaaa:1::99"))
            .await
            .unwrap();
        assert_eq!(
            rejected_code(
                auth.admin_login(EMAIL, PASSWORD, None, &from("192.0.2.50"))
                    .await
            ),
            crate::limits::RATE_LIMITED
        );
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn команда_снимает_блокировку_входа_по_коду() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        let (_, secret) = totp_owner(&env).await;
        let office = from("203.0.113.7");
        let wrong = wrong_code(&secret, start());
        for _ in 0..rules::ADMIN_TOTP_FAILURES.limit {
            assert_eq!(
                rejected_code(
                    auth.admin_login(EMAIL, PASSWORD, Some(&wrong), &office)
                        .await
                ),
                TOTP_INVALID
            );
        }
        let later = start() + Duration::seconds(30);
        env.clock.set(later);
        let code = code_at(&secret, later);
        assert_eq!(
            rejected_code(
                auth.admin_login(EMAIL, PASSWORD, Some(&code), &office)
                    .await
            ),
            crate::limits::RATE_LIMITED
        );
        let unlock = auth.unlock_admin_login(EMAIL, &[]).await.unwrap();
        assert!(unlock.totp_locked);
        assert!(!unlock.password_locked);
        assert_eq!(unlock.password_failures, 0);
        auth.admin_login(EMAIL, PASSWORD, Some(&code), &office)
            .await
            .unwrap();
        env.db.cleanup().await;
    }

    #[tokio::test]
    async fn снять_блокировку_нельзя_неизвестному_и_отключённому_админу() {
        let Some(env) = env().await else { return };
        let auth = env.services.auth();
        let admin = auth.upsert_owner(EMAIL, PASSWORD).await.unwrap().0;
        assert_eq!(
            rejected_code(auth.unlock_admin_login("nobody@example.com", &[]).await),
            "auth.unknown_admin"
        );
        {
            let mut conn = env.services.store().conn().await.unwrap();
            sqlx::query("UPDATE admins SET disabled_at = now() WHERE id = $1")
                .bind(admin.0)
                .execute(conn.conn())
                .await
                .unwrap();
        }
        assert_eq!(
            rejected_code(auth.unlock_admin_login(EMAIL, &[]).await),
            "auth.admin_disabled"
        );
        assert!(
            !audit_actions(&env, admin)
                .await
                .contains(&"admin.login_unlocked".to_string())
        );
        env.db.cleanup().await;
    }
}
