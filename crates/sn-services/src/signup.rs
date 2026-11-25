use crate::auth::{Auth, RequestMeta, hash_password_async, new_token, token_hash};
use crate::error::{ServiceError, ServiceResult};
use crate::limits::{Strike, rules};
use crate::login::{
    EMAIL_CODE_DIGITS, EMAIL_CODE_MAX_ATTEMPTS, EMAIL_CODE_TTL_SECONDS, LOGIN_LETTERS,
    code_invalid, code_strike, code_values, email_code, referral_code, same,
};
use crate::mail::SignupRule;
use crate::site_signup;
use crate::two_factor::{CabinetLogin, cabinet_session};
use crate::utm_tracking::site_start_param;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::client::{
    IdentityKind, Locale, NewIdentity, RegistrationSource, normalize_identity,
};
use sn_domain::events::DomainEvent;
use sn_domain::lang;
use sn_domain::mail_letters::{LetterId, Values};
use sn_modules::catalog::antibot::signups::{self, Refusal, Via};
use sn_modules::catalog::start_param::Utm;
use sn_store::{Db, NewClient};

pub const SIGNUP_TTL_SECONDS: i64 = 1800;

#[derive(Debug, Clone, Copy, Default)]
pub struct Arrival<'a> {
    pub utm: Option<&'a Utm>,
    pub referral: Option<&'a str>,
}

impl<'a> Arrival<'a> {
    pub fn site(utm: Option<&'a Utm>, referral: Option<&'a str>) -> Self {
        Arrival { utm, referral }
    }

    pub fn start_param(&self) -> Option<String> {
        let referral = self.referral.and_then(referral_code);
        site_start_param(self.utm, referral.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename = "code_sent")]
pub struct SignupStart {
    pub email: String,
    pub signup_token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Registration {
    Login(CabinetLogin),
    CodeSent(SignupStart),
}

#[derive(sqlx::FromRow)]
struct Pending {
    id: i64,
    code_hash: Vec<u8>,
    password_hash: Option<String>,
    taken: bool,
    locale: String,
    start_param: Option<String>,
    attempts: i32,
    code_expires_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

const PENDING_COLUMNS: &str = "id, code_hash, password_hash, taken, locale, start_param, attempts, code_expires_at, expires_at";

pub(crate) fn confirm_required() -> ServiceError {
    ServiceError::rejected(
        "auth.email_confirm",
        "Регистрация по почте — с подтверждением кодом из письма",
    )
}

pub(crate) fn signup_closed() -> ServiceError {
    ServiceError::rejected(
        "auth.signup_closed",
        "Регистрация по почте сейчас недоступна: письма с кодом не отправляются. Войдите через Telegram или напишите в поддержку.",
    )
}

fn signup_expired() -> ServiceError {
    ServiceError::rejected(
        "auth.signup_expired",
        "Время на подтверждение вышло или регистрацию начали заново. Зарегистрируйтесь ещё раз.",
    )
}

fn code_attempts() -> ServiceError {
    ServiceError::rejected(
        "auth.code_attempts",
        "Слишком много неверных попыток, запросите новый код",
    )
}

fn code_digest(email: &str, code: &str) -> Vec<u8> {
    token_hash(&format!("{email}:{code}"))
}

fn locale_of(raw: &str) -> Locale {
    Locale::parse(raw).unwrap_or_default()
}

fn digits(code: &str) -> Option<String> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    (code.len() == EMAIL_CODE_DIGITS && code.bytes().all(|b| b.is_ascii_digit())).then_some(code)
}

impl Auth<'_> {
    pub async fn email_signup(
        &self,
        email: &str,
        password: &str,
        meta: &RequestMeta,
        from: Arrival<'_>,
    ) -> ServiceResult<Registration> {
        self.email_signup_with(email, password, meta, from, None)
            .await
    }

    pub async fn email_signup_with(
        &self,
        email: &str,
        password: &str,
        meta: &RequestMeta,
        from: Arrival<'_>,
        captcha: Option<&str>,
    ) -> ServiceResult<Registration> {
        self.site_open()?;
        self.email_by_password()?;
        let rule = self.services.mail().signup_rule();
        if rule == SignupRule::Closed {
            return Err(signup_closed());
        }
        self.captcha_gate(sn_modules::catalog::antibot::SIGNUP, captcha, meta)
            .await?;
        match rule {
            SignupRule::Open => self
                .register_now(email, password, meta, from.start_param(), captcha)
                .await
                .map(Registration::Login),
            SignupRule::Confirm => self
                .signup_start(email, password, meta, from)
                .await
                .map(Registration::CodeSent),
            SignupRule::Closed => Err(signup_closed()),
        }
    }

    async fn email_known(&self, email: &str) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .clients()
            .find_by_identity(IdentityKind::Email, email)
            .await?
            .is_some())
    }

    async fn signup_letter(
        &self,
        email: &str,
        taken: bool,
        code: String,
        locale: Locale,
    ) -> ServiceResult<()> {
        let letters = self.services.letters();
        if taken {
            lang::scope(
                locale,
                letters.send(LetterId::SignupTaken, email, Values::new()),
            )
            .await
        } else {
            lang::scope(
                locale,
                letters.send(LetterId::SignupCode, email, code_values(code)),
            )
            .await
        }
    }

    async fn signup_start(
        &self,
        email: &str,
        password: &str,
        meta: &RequestMeta,
        from: Arrival<'_>,
    ) -> ServiceResult<SignupStart> {
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_START_IP, meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_START_ADDRESS, &email).await?;
        limits
            .hit_wide(rules::EMAIL_START_WIDE, meta.ip.as_deref())
            .await?;
        limits
            .letters_sent(LOGIN_LETTERS, self.services.mail().codes_per_hour())
            .await?;
        let hash = hash_password_async(password).await?;
        let taken = self.email_known(&email).await?;
        let locale = self.services.settings().languages.pick(lang::hint());
        let (token, token_digest) = new_token();
        let code = email_code();
        let guard = new_token().0;
        let now = self.services.now();
        let code_expires_at = now + Duration::seconds(EMAIL_CODE_TTL_SECONDS);
        let mut tx = self.services.store().begin().await?;
        sqlx::query("DELETE FROM email_signups WHERE email = $1 OR expires_at < $2")
            .bind(&email)
            .bind(now)
            .execute(tx.conn())
            .await?;
        sqlx::query(
            "INSERT INTO email_signups (email, token_hash, code_hash, password_hash, taken, locale, start_param, \
             ip, user_agent, created_at, code_expires_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(&email)
        .bind(&token_digest)
        .bind(code_digest(&email, if taken { &guard } else { &code }))
        .bind((!taken).then_some(&hash))
        .bind(taken)
        .bind(locale.as_str())
        .bind(from.start_param())
        .bind(&meta.ip)
        .bind(&meta.user_agent)
        .bind(now)
        .bind(code_expires_at)
        .bind(now + Duration::seconds(SIGNUP_TTL_SECONDS))
        .execute(tx.conn())
        .await?;
        tx.commit().await?;
        self.signup_letter(&email, taken, code, locale).await?;
        Ok(SignupStart {
            email,
            signup_token: token,
            expires_at: code_expires_at,
        })
    }

    pub async fn email_signup_resend(
        &self,
        email: &str,
        token: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<SignupStart> {
        self.site_open()?;
        self.email_by_password()?;
        if !self.services.mail().available() {
            return Err(signup_closed());
        }
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_START_IP, meta.ip.as_deref())
            .await?;
        let digest = token_hash(token);
        let now = self.services.now();
        let alive: Option<i64> = {
            let mut conn = self.services.store().conn().await?;
            sqlx::query_scalar(
                "SELECT id FROM email_signups WHERE token_hash = $1 AND email = $2 AND expires_at > $3",
            )
            .bind(&digest)
            .bind(&email)
            .bind(now)
            .fetch_optional(conn.conn())
            .await?
        };
        if alive.is_none() {
            return Err(signup_expired());
        }
        limits.hit(rules::EMAIL_START_ADDRESS, &email).await?;
        limits
            .hit_wide(rules::EMAIL_START_WIDE, meta.ip.as_deref())
            .await?;
        limits
            .letters_sent(LOGIN_LETTERS, self.services.mail().codes_per_hour())
            .await?;
        let known = self.email_known(&email).await?;
        let code = email_code();
        let guard = new_token().0;
        let now = self.services.now();
        let code_expires_at = now + Duration::seconds(EMAIL_CODE_TTL_SECONDS);
        let mut tx = self.services.store().begin().await?;
        let row: Option<Pending> = sqlx::query_as(&format!(
            "SELECT {PENDING_COLUMNS} FROM email_signups WHERE token_hash = $1 AND email = $2 FOR UPDATE"
        ))
        .bind(&digest)
        .bind(&email)
        .fetch_optional(tx.conn())
        .await?;
        let Some(row) = row.filter(|r| r.expires_at > now) else {
            return Err(signup_expired());
        };
        let taken = row.taken || known;
        sqlx::query(
            "UPDATE email_signups SET code_hash = $2, attempts = 0, taken = $3, \
             password_hash = CASE WHEN $3 THEN NULL ELSE password_hash END, \
             code_expires_at = $4, expires_at = $5 WHERE id = $1",
        )
        .bind(row.id)
        .bind(code_digest(&email, if taken { &guard } else { &code }))
        .bind(taken)
        .bind(code_expires_at)
        .bind(now + Duration::seconds(SIGNUP_TTL_SECONDS))
        .execute(tx.conn())
        .await?;
        tx.commit().await?;
        self.signup_letter(&email, taken, code, locale_of(&row.locale))
            .await?;
        Ok(SignupStart {
            email,
            signup_token: token.trim().to_string(),
            expires_at: code_expires_at,
        })
    }

    pub async fn email_signup_confirm(
        &self,
        email: &str,
        token: &str,
        code: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<CabinetLogin> {
        self.email_signup_confirm_with(email, token, code, meta, None)
            .await
    }

    pub async fn email_signup_confirm_with(
        &self,
        email: &str,
        token: &str,
        code: &str,
        meta: &RequestMeta,
        captcha: Option<&str>,
    ) -> ServiceResult<CabinetLogin> {
        self.site_open()?;
        self.email_by_password()?;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_VERIFY_IP, meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_VERIFY_ADDRESS, &email).await?;
        let Some(code) = digits(code) else {
            return Err(code_invalid());
        };
        let mut strike = Some(code_strike(&limits, &email).await?);
        let mut human = site_signup::asked_before(self.services);
        loop {
            match self
                .create_by_code(&email, token, &code, meta, strike.as_ref(), human)
                .await?
            {
                Ok(login) => return Ok(login),
                Err(refusal) => {
                    if let Some(strike) = strike.take() {
                        limits.forgive(&strike).await?;
                    }
                    site_signup::over_limit(self.services, refusal, captcha, meta).await?;
                    human = true;
                }
            }
        }
    }

    async fn create_by_code(
        &self,
        email: &str,
        token: &str,
        code: &str,
        meta: &RequestMeta,
        strike: Option<&Strike>,
        human: bool,
    ) -> ServiceResult<Result<CabinetLogin, Refusal>> {
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: Option<Pending> = sqlx::query_as(&format!(
            "SELECT {PENDING_COLUMNS} FROM email_signups WHERE token_hash = $1 AND email = $2 FOR UPDATE"
        ))
        .bind(token_hash(token))
        .bind(email)
        .fetch_optional(tx.conn())
        .await?;
        let Some(row) = row.filter(|r| r.expires_at > now) else {
            return Err(signup_expired());
        };
        if row.attempts >= EMAIL_CODE_MAX_ATTEMPTS {
            return Err(code_attempts());
        }
        let proven = !row.taken
            && row.code_expires_at > now
            && same(&row.code_hash, &code_digest(email, code));
        let password_hash = match row.password_hash {
            Some(hash) if proven => hash,
            _ => {
                sqlx::query("UPDATE email_signups SET attempts = attempts + 1 WHERE id = $1")
                    .bind(row.id)
                    .execute(tx.conn())
                    .await?;
                tx.commit().await?;
                return Err(code_invalid());
            }
        };
        if let Some(strike) = strike {
            self.services.limits().release(&mut tx, strike).await?;
        }
        sqlx::query("DELETE FROM email_signups WHERE email = $1")
            .bind(email)
            .execute(tx.conn())
            .await?;
        if tx
            .clients()
            .find_by_identity(IdentityKind::Email, email)
            .await?
            .is_some()
        {
            tx.commit().await?;
            return Err(site_signup::email_taken());
        }
        let admission = match signups::admit(&mut tx, meta.ip.as_deref(), human).await? {
            Ok(admission) => admission,
            Err(refusal) => {
                tx.rollback().await?;
                return Ok(Err(refusal));
            }
        };
        let client = tx
            .clients()
            .create(&NewClient {
                locale: self
                    .services
                    .settings()
                    .languages
                    .effective(locale_of(&row.locale)),
                registration_source: RegistrationSource::Web,
                note: None,
            })
            .await?;
        tx.clients()
            .add_identity(
                client.id,
                &NewIdentity::new(IdentityKind::Email, email, true)?,
            )
            .await?;
        sqlx::query("INSERT INTO client_passwords (client_id, password_hash) VALUES ($1, $2)")
            .bind(client.id.0)
            .bind(&password_hash)
            .execute(tx.conn())
            .await?;
        signups::record(&mut tx, admission, client.id, Via::Password).await?;
        tx.events()
            .push(&DomainEvent::ClientRegistered {
                client_id: client.id,
                source: RegistrationSource::Web,
                start_param: row.start_param,
            })
            .await?;
        let login = cabinet_session(&mut tx, client.id, now, meta, true).await?;
        tx.commit().await?;
        Ok(Ok(login))
    }
}
