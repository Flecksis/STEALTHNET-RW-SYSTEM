use crate::Actor;
use crate::auth::{
    Auth, CLIENT_SESSION_DAYS, ClientLogin, RequestMeta, check_password, hash_password_async,
    token_hash,
};
use crate::error::{ServiceError, ServiceResult};
use crate::limits::rules;
use crate::two_factor::{CabinetLogin, cabinet_session};
use chrono::{DateTime, Duration, Utc};
use rand::{Rng, RngCore};
use serde::Serialize;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_domain::client::{
    IdentityKind, Locale, NewIdentity, RegistrationSource, TelegramProfile, normalize_identity,
};
use sn_domain::events::DomainEvent;
use sn_domain::ids::ClientId;
use sn_domain::settings::keys;
use sn_store::{Db, NewClient, NewSession, SessionChannel, SessionScope, Tx};

pub const TELEGRAM_LOGIN_TTL_SECONDS: i64 = 300;
pub const TELEGRAM_START_PREFIX: &str = "auth_";
pub const EMAIL_CODE_TTL_SECONDS: i64 = 900;
pub const EMAIL_CODE_MAX_ATTEMPTS: i32 = 5;
pub const EMAIL_CODE_DIGITS: usize = 6;
pub const LOGIN_RETENTION_DAYS: i64 = 7;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelegramLoginStart {
    pub poll_token: String,
    pub link: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelegramLoginPrompt {
    pub account: String,
    pub ip: Option<String>,
    pub device: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl TelegramLoginPrompt {
    pub fn text(&self) -> String {
        let place = match (&self.ip, &self.device) {
            (Some(ip), Some(device)) => format!("{ip}, {device}"),
            (Some(ip), None) => ip.clone(),
            (None, Some(device)) => device.clone(),
            (None, None) => "неизвестно".to_string(),
        };
        format!(
            "Вход в личный кабинет\nАккаунт: {}\nОткуда: {}\nЗапрос создан в {} UTC.\n\nЕсли входите не вы, нажмите «Это не я».",
            self.account,
            place,
            self.requested_at.format("%H:%M"),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TelegramLoginConfirmed {
    pub client_id: ClientId,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TelegramLoginStatus {
    Pending {
        expires_at: DateTime<Utc>,
    },
    Confirmed {
        login: ClientLogin,
    },
    TotpRequired {
        challenge: String,
        expires_at: DateTime<Utc>,
    },
    Rejected,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailLoginStart {
    pub email: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct LoginRow {
    id: i64,
    status: String,
    client_id: Option<i64>,
    code_hash: Vec<u8>,
    attempts: i32,
    ip: Option<String>,
    user_agent: Option<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

const LOGIN_COLUMNS: &str =
    "id, status, client_id, code_hash, attempts, ip, user_agent, created_at, expires_at";

fn random_hex(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    hex::encode(raw)
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn link_code(raw: &str) -> Option<String> {
    let code = raw.trim();
    let code = code.strip_prefix(TELEGRAM_START_PREFIX).unwrap_or(code);
    (code.len() == 32 && code.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| code.to_ascii_lowercase())
}

pub fn referral_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    ((4..=16).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then_some(code)
}

fn expired_link() -> ServiceError {
    ServiceError::rejected(
        "auth.login_expired",
        "Ссылка для входа устарела. Начните вход на сайте заново.",
    )
}

fn code_invalid() -> ServiceError {
    ServiceError::rejected("auth.code_invalid", "Код не подошёл или устарел")
}

pub fn describe_user_agent(user_agent: &str) -> Option<String> {
    let browser = if user_agent.contains("YaBrowser") {
        Some("Яндекс Браузер")
    } else if user_agent.contains("Edg/") {
        Some("Edge")
    } else if user_agent.contains("OPR/") || user_agent.contains("Opera") {
        Some("Opera")
    } else if user_agent.contains("Firefox/") || user_agent.contains("FxiOS") {
        Some("Firefox")
    } else if user_agent.contains("Chrome/") || user_agent.contains("CriOS") {
        Some("Chrome")
    } else if user_agent.contains("Safari/") {
        Some("Safari")
    } else {
        None
    };
    let os = if user_agent.contains("iPhone") || user_agent.contains("iPad") {
        Some("iOS")
    } else if user_agent.contains("Android") {
        Some("Android")
    } else if user_agent.contains("Windows") {
        Some("Windows")
    } else if user_agent.contains("Mac OS X") || user_agent.contains("Macintosh") {
        Some("macOS")
    } else if user_agent.contains("Linux") {
        Some("Linux")
    } else {
        None
    };
    match (browser, os) {
        (Some(browser), Some(os)) => Some(format!("{browser}, {os}")),
        (Some(single), None) | (None, Some(single)) => Some(single.to_string()),
        (None, None) => None,
    }
}

fn account_title(profile: &TelegramProfile) -> String {
    let name = profile.display_name();
    match &profile.username {
        Some(username) if !name.starts_with('@') => format!("{name} (@{username})"),
        _ => name,
    }
}

pub(crate) async fn issue_session(
    tx: &mut Tx,
    client_id: ClientId,
    now: DateTime<Utc>,
    meta: &RequestMeta,
    created: bool,
) -> ServiceResult<ClientLogin> {
    let (token, hash) = crate::auth::new_token();
    let expires_at = now + Duration::days(CLIENT_SESSION_DAYS);
    let session_id = tx
        .sessions()
        .create_client(
            client_id,
            SessionChannel::Cabinet,
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
            channel: ClientChannel::Cabinet,
            full_access: true,
            session_id: Some(session_id),
        },
        created,
    })
}

impl<'a> Auth<'a> {
    async fn bot_username(&self) -> ServiceResult<String> {
        let mut conn = self.services.store().conn().await?;
        conn.settings()
            .get_as::<String>(keys::BOT_USERNAME)
            .await?
            .map(|name| name.trim().trim_start_matches('@').to_string())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                ServiceError::rejected(
                    "bot.not_configured",
                    "Вход через Telegram недоступен: в настройках не указан бот",
                )
            })
    }

    pub async fn telegram_login_start(
        &self,
        meta: &RequestMeta,
    ) -> ServiceResult<TelegramLoginStart> {
        self.telegram_login_start_with(meta, None).await
    }

    fn site_open(&self) -> ServiceResult<()> {
        if self.services.settings().client_access.site_login {
            Ok(())
        } else {
            Err(ServiceError::rejected(
                "auth.site_off",
                "Вход на сайт выключен: пользуйтесь ботом",
            ))
        }
    }

    pub async fn telegram_login_start_with(
        &self,
        meta: &RequestMeta,
        referral: Option<&str>,
    ) -> ServiceResult<TelegramLoginStart> {
        self.site_open()?;
        self.services
            .limits()
            .hit_ip(rules::TELEGRAM_START_IP, meta.ip.as_deref())
            .await?;
        let bot = self.bot_username().await?;
        let code = random_hex(16);
        let poll_token = random_hex(32);
        let now = self.services.now();
        let expires_at = now + Duration::seconds(TELEGRAM_LOGIN_TTL_SECONDS);
        let mut conn = self.services.store().conn().await?;
        sqlx::query(
            "INSERT INTO client_login_requests (method, code_hash, poll_hash, ip, user_agent, created_at, expires_at) \
             VALUES ('telegram', $1, $2, $3, $4, $5, $6)",
        )
        .bind(token_hash(&code))
        .bind(token_hash(&poll_token))
        .bind(&meta.ip)
        .bind(&meta.user_agent)
        .bind(now)
        .bind(expires_at)
        .execute(conn.conn())
        .await?;
        sqlx::query("DELETE FROM client_login_requests WHERE expires_at < $1")
            .bind(now - Duration::days(LOGIN_RETENTION_DAYS))
            .execute(conn.conn())
            .await?;
        Ok(TelegramLoginStart {
            poll_token,
            link: format!(
                "https://t.me/{bot}?start={TELEGRAM_START_PREFIX}{code}{}",
                referral
                    .and_then(referral_code)
                    .map(|c| format!("__ref_{c}"))
                    .unwrap_or_default()
            ),
            expires_at,
        })
    }

    async fn pending_telegram_request(&self, code: &str) -> ServiceResult<LoginRow> {
        let code = link_code(code).ok_or(ServiceError::NotFound("запрос входа"))?;
        let mut conn = self.services.store().conn().await?;
        let row: LoginRow = sqlx::query_as(&format!(
            "SELECT {LOGIN_COLUMNS} FROM client_login_requests WHERE method = 'telegram' AND code_hash = $1"
        ))
        .bind(token_hash(&code))
        .fetch_optional(conn.conn())
        .await?
        .ok_or(ServiceError::NotFound("запрос входа"))?;
        if row.status != "pending" || row.expires_at <= self.services.now() {
            return Err(expired_link());
        }
        Ok(row)
    }

    pub async fn telegram_login_prompt(
        &self,
        code: &str,
        profile: &TelegramProfile,
    ) -> ServiceResult<TelegramLoginPrompt> {
        let row = self.pending_telegram_request(code).await?;
        Ok(TelegramLoginPrompt {
            account: account_title(profile),
            ip: row.ip,
            device: row.user_agent.as_deref().and_then(describe_user_agent),
            requested_at: row.created_at,
            expires_at: row.expires_at,
        })
    }

    pub async fn confirm_telegram_login(
        &self,
        code: &str,
        profile: &TelegramProfile,
    ) -> ServiceResult<TelegramLoginConfirmed> {
        let pending = self.pending_telegram_request(code).await?;
        let (client, created) = self
            .services
            .clients()
            .ensure_telegram(profile, RegistrationSource::Web, None)
            .await?;
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: LoginRow = sqlx::query_as(&format!(
            "SELECT {LOGIN_COLUMNS} FROM client_login_requests WHERE id = $1 FOR UPDATE"
        ))
        .bind(pending.id)
        .fetch_one(tx.conn())
        .await?;
        if row.status != "pending" || row.expires_at <= now {
            return Err(expired_link());
        }
        sqlx::query(
            "UPDATE client_login_requests SET status = 'confirmed', client_id = $2, confirmed_at = $3 WHERE id = $1",
        )
        .bind(row.id)
        .bind(client.id.0)
        .bind(now)
        .execute(tx.conn())
        .await?;
        tx.audit()
            .record(&Actor::Client(client.id).audit(
                "client.telegram_login_confirmed",
                "client",
                Some(client.id.0),
                serde_json::json!({ "ip": row.ip }),
            ))
            .await?;
        tx.commit().await?;
        Ok(TelegramLoginConfirmed {
            client_id: client.id,
            created,
        })
    }

    pub async fn reject_telegram_login(&self, code: &str) -> ServiceResult<()> {
        let code = link_code(code).ok_or(ServiceError::NotFound("запрос входа"))?;
        let mut conn = self.services.store().conn().await?;
        sqlx::query(
            "UPDATE client_login_requests SET status = 'rejected' \
             WHERE method = 'telegram' AND code_hash = $1 AND status = 'pending'",
        )
        .bind(token_hash(&code))
        .execute(conn.conn())
        .await?;
        Ok(())
    }

    pub async fn telegram_login_status(
        &self,
        poll_token: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<TelegramLoginStatus> {
        self.services
            .limits()
            .hit_ip(rules::TELEGRAM_POLL_IP, meta.ip.as_deref())
            .await?;
        if poll_token.trim().is_empty() {
            return Err(ServiceError::NotFound("запрос входа"));
        }
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: LoginRow = sqlx::query_as(&format!(
            "SELECT {LOGIN_COLUMNS} FROM client_login_requests \
             WHERE method = 'telegram' AND poll_hash = $1 FOR UPDATE"
        ))
        .bind(token_hash(poll_token))
        .fetch_optional(tx.conn())
        .await?
        .ok_or(ServiceError::NotFound("запрос входа"))?;
        let grace = Duration::seconds(TELEGRAM_LOGIN_TTL_SECONDS);
        let status = match row.status.as_str() {
            "pending" if row.expires_at > now => TelegramLoginStatus::Pending {
                expires_at: row.expires_at,
            },
            "rejected" => TelegramLoginStatus::Rejected,
            "confirmed" if row.expires_at + grace > now => {
                let client_id = ClientId(row.client_id.ok_or(ServiceError::Internal(
                    "подтверждённый вход без клиента".into(),
                ))?);
                let client = tx.clients().get(client_id).await?;
                if client.is_blocked() {
                    return Err(ServiceError::Forbidden);
                }
                sqlx::query(
                    "UPDATE client_login_requests SET status = 'consumed', consumed_at = $2 WHERE id = $1",
                )
                .bind(row.id)
                .bind(now)
                .execute(tx.conn())
                .await?;
                let status = match cabinet_session(&mut tx, client_id, now, meta, false).await? {
                    CabinetLogin::Session(login) => TelegramLoginStatus::Confirmed { login },
                    CabinetLogin::TotpRequired(totp) => TelegramLoginStatus::TotpRequired {
                        challenge: totp.challenge,
                        expires_at: totp.expires_at,
                    },
                };
                tx.commit().await?;
                return Ok(status);
            }
            _ => TelegramLoginStatus::Expired,
        };
        tx.commit().await?;
        Ok(status)
    }

    pub async fn email_login_start(
        &self,
        email: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<EmailLoginStart> {
        self.site_open()?;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_START_IP, meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_START_ADDRESS, &email).await?;
        let mail = self.services.mail();
        if !mail.available() {
            return Err(ServiceError::rejected(
                "auth.email_password_mode",
                "Вход по почте — по паролю, без кода",
            ));
        }
        let (code, start) = self.issue_email_code(&email, meta).await?;
        let brand = self.services.settings().brand_name.clone();
        let minutes = EMAIL_CODE_TTL_SECONDS / 60;
        mail.send(&crate::mail::Letter {
            to: start.email.clone(),
            subject: format!("Код для входа в {brand}: {code}"),
            text: format!(
                "Код для входа в кабинет {brand}: {code}\n\nОн действует {minutes} минут. Если вход запрашивали не вы, просто не используйте код."
            ),
        })
        .await?;
        Ok(start)
    }

    fn email_by_password(&self) -> ServiceResult<()> {
        if self.services.mail().available() {
            Err(ServiceError::rejected(
                "auth.email_code_mode",
                "Вход по почте — кодом из письма",
            ))
        } else {
            Ok(())
        }
    }

    pub async fn email_register(
        &self,
        email: &str,
        password: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<CabinetLogin> {
        self.site_open()?;
        self.email_by_password()?;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_START_IP, meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_START_ADDRESS, &email).await?;
        let hash = hash_password_async(password).await?;
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        if tx
            .clients()
            .find_by_identity(IdentityKind::Email, &email)
            .await?
            .is_some()
        {
            return Err(ServiceError::rejected(
                "auth.email_taken",
                "Эта почта уже зарегистрирована: войдите с паролем",
            ));
        }
        let client = tx
            .clients()
            .create(&NewClient {
                locale: Locale::Ru,
                registration_source: RegistrationSource::Web,
                note: None,
            })
            .await?;
        tx.clients()
            .add_identity(
                client.id,
                &NewIdentity::new(IdentityKind::Email, &email, false)?,
            )
            .await?;
        sqlx::query("INSERT INTO client_passwords (client_id, password_hash) VALUES ($1, $2)")
            .bind(client.id.0)
            .bind(&hash)
            .execute(tx.conn())
            .await?;
        tx.events()
            .push(&DomainEvent::ClientRegistered {
                client_id: client.id,
                source: RegistrationSource::Web,
                start_param: None,
            })
            .await?;
        let login = cabinet_session(&mut tx, client.id, now, meta, true).await?;
        tx.commit().await?;
        Ok(login)
    }

    pub async fn email_password_login(
        &self,
        email: &str,
        password: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<CabinetLogin> {
        self.site_open()?;
        self.email_by_password()?;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_VERIFY_IP, meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_VERIFY_ADDRESS, &email).await?;
        let (client, hash) = {
            let mut conn = self.services.store().conn().await?;
            let client = conn
                .clients()
                .find_by_identity(IdentityKind::Email, &email)
                .await?;
            let hash: Option<String> = match &client {
                Some(client) => {
                    sqlx::query_scalar(
                        "SELECT password_hash FROM client_passwords WHERE client_id = $1",
                    )
                    .bind(client.id.0)
                    .fetch_optional(conn.conn())
                    .await?
                }
                None => None,
            };
            (client, hash)
        };
        let valid = check_password(password, hash).await?;
        let Some(client) = client.filter(|_| valid) else {
            return Err(ServiceError::rejected(
                "auth.wrong_password",
                "Почта или пароль не подошли",
            ));
        };
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let login = cabinet_session(&mut tx, client.id, now, meta, false).await?;
        tx.commit().await?;
        Ok(login)
    }

    pub async fn issue_email_code(
        &self,
        email: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<(String, EmailLoginStart)> {
        let email = normalize_identity(IdentityKind::Email, email)?;
        let code: String = {
            let mut rng = rand::rngs::OsRng;
            (0..EMAIL_CODE_DIGITS)
                .map(|_| char::from(b'0' + rng.gen_range(0..10u8)))
                .collect()
        };
        let now = self.services.now();
        let expires_at = now + Duration::seconds(EMAIL_CODE_TTL_SECONDS);
        let mut tx = self.services.store().begin().await?;
        sqlx::query(
            "UPDATE client_login_requests SET status = 'rejected' \
             WHERE method = 'email' AND email = $1 AND status = 'pending'",
        )
        .bind(&email)
        .execute(tx.conn())
        .await?;
        sqlx::query(
            "INSERT INTO client_login_requests (method, code_hash, email, ip, user_agent, created_at, expires_at) \
             VALUES ('email', $1, $2, $3, $4, $5, $6)",
        )
        .bind(token_hash(&format!("{email}:{code}")))
        .bind(&email)
        .bind(&meta.ip)
        .bind(&meta.user_agent)
        .bind(now)
        .bind(expires_at)
        .execute(tx.conn())
        .await?;
        tx.commit().await?;
        Ok((code, EmailLoginStart { email, expires_at }))
    }

    pub async fn email_login_verify(
        &self,
        email: &str,
        code: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<CabinetLogin> {
        self.site_open()?;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_VERIFY_IP, meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_VERIFY_ADDRESS, &email).await?;
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        if code.len() != EMAIL_CODE_DIGITS || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(code_invalid());
        }
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: Option<LoginRow> = sqlx::query_as(&format!(
            "SELECT {LOGIN_COLUMNS} FROM client_login_requests \
             WHERE method = 'email' AND email = $1 AND status = 'pending' ORDER BY id DESC LIMIT 1 FOR UPDATE"
        ))
        .bind(&email)
        .fetch_optional(tx.conn())
        .await?;
        let Some(row) = row.filter(|r| r.expires_at > now) else {
            return Err(code_invalid());
        };
        if row.attempts >= EMAIL_CODE_MAX_ATTEMPTS {
            return Err(ServiceError::rejected(
                "auth.code_attempts",
                "Слишком много неверных попыток, запросите новый код",
            ));
        }
        if !same(&row.code_hash, &token_hash(&format!("{email}:{code}"))) {
            sqlx::query("UPDATE client_login_requests SET attempts = attempts + 1 WHERE id = $1")
                .bind(row.id)
                .execute(tx.conn())
                .await?;
            tx.commit().await?;
            return Err(code_invalid());
        }
        let (client_id, created) = match tx
            .clients()
            .find_by_identity(IdentityKind::Email, &email)
            .await?
        {
            Some(client) if client.is_blocked() => return Err(ServiceError::Forbidden),
            Some(client) => (client.id, false),
            None => {
                let client = tx
                    .clients()
                    .create(&NewClient {
                        locale: Locale::Ru,
                        registration_source: RegistrationSource::Web,
                        note: None,
                    })
                    .await?;
                tx.clients()
                    .add_identity(
                        client.id,
                        &NewIdentity::new(IdentityKind::Email, &email, true)?,
                    )
                    .await?;
                tx.events()
                    .push(&DomainEvent::ClientRegistered {
                        client_id: client.id,
                        source: RegistrationSource::Web,
                        start_param: None,
                    })
                    .await?;
                (client.id, true)
            }
        };
        sqlx::query(
            "UPDATE client_login_requests SET status = 'consumed', client_id = $2, confirmed_at = $3, consumed_at = $3 \
             WHERE id = $1",
        )
        .bind(row.id)
        .bind(client_id.0)
        .bind(now)
        .execute(tx.conn())
        .await?;
        let login = cabinet_session(&mut tx, client_id, now, meta, created).await?;
        tx.commit().await?;
        Ok(login)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn устройство_входа_описывается_по_user_agent() {
        assert_eq!(
            describe_user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36"
            )
            .as_deref(),
            Some("Chrome, Windows")
        );
        assert_eq!(
            describe_user_agent(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 Version/18.1 Mobile/15E148 Safari/604.1"
            )
            .as_deref(),
            Some("Safari, iOS")
        );
        assert_eq!(describe_user_agent("curl/8.0"), None);
    }

    #[test]
    fn код_из_ссылки_принимается_с_префиксом_и_без() {
        let code = "0123456789abcdef0123456789ABCDEF";
        assert_eq!(
            link_code(&format!("auth_{code}")).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            link_code(code).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(link_code("auth_short"), None);
        assert_eq!(link_code("auth_zz23456789abcdef0123456789abcdef"), None);
    }

    #[test]
    fn текст_подтверждения_называет_аккаунт_и_место() {
        let at = DateTime::parse_from_rfc3339("2026-11-16T14:52:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let prompt = TelegramLoginPrompt {
            account: account_title(&TelegramProfile {
                id: 1,
                username: Some("morozov_a".into()),
                first_name: Some("Алексей".into()),
                last_name: None,
                language_code: None,
                is_premium: false,
            }),
            ip: Some("203.0.113.7".into()),
            device: Some("Chrome, Windows".into()),
            requested_at: at,
            expires_at: at + Duration::minutes(5),
        };
        let text = prompt.text();
        assert!(text.contains("Аккаунт: Алексей (@morozov_a)"));
        assert!(text.contains("Откуда: 203.0.113.7, Chrome, Windows"));
        assert!(text.contains("14:52"));
    }
}
