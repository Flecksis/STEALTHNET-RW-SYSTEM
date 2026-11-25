use crate::Actor;
use crate::auth::{Auth, RequestMeta, check_password, token_hash};
use crate::error::{ServiceError, ServiceResult};
use crate::limits::{RateLimits, RateRule, rules};
use crate::login::{
    EMAIL_CODE_DIGITS, EMAIL_CODE_MAX_ATTEMPTS, EMAIL_CODE_TTL_SECONDS, code_invalid, same,
};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::Serialize;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_domain::client::{
    Identity, IdentityKind, LINKED_WITHOUT_CODE, NewIdentity, normalize_identity,
};
use sn_domain::ids::ClientId;
use sn_domain::mail_letters::{LetterId, Values};
use sn_store::{Db, Tx};

pub const EMAIL_CHANGE_OFF: &str = "auth.email_change_off";
pub const EMAIL_CODE_NEEDED: &str = "auth.email_code_needed";
pub const EMAIL_IN_USE: &str = "auth.email_in_use";
pub const EMAIL_SAME: &str = "auth.email_same";
pub const PASSWORD_REQUIRED: &str = "auth.password_required";
pub const APP_CODE_REQUIRED: &str = "auth.app_code_required";
pub const PASSWORD_INVALID: &str = "auth.password_invalid";
pub const CURRENT_CODE_REQUIRED: &str = "auth.current_email_code_required";
pub const CURRENT_CODE_NOT_NEEDED: &str = "auth.current_email_code_not_needed";
pub const EMAIL_CHANGED_AUDIT: &str = "client.email_changed";
pub const EMAIL_LINKED_AUDIT: &str = "client.email_linked";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailProof {
    Totp,
    Password,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailPending {
    pub email: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailState {
    pub email: Option<String>,
    pub available: bool,
    pub without_code: bool,
    pub unconfirmed: bool,
    pub proof: Option<EmailProof>,
    pub confirm_current: bool,
    pub pending: Option<EmailPending>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EmailProofInput<'r> {
    pub password: Option<&'r str>,
    pub code: Option<&'r str>,
}

#[derive(sqlx::FromRow)]
struct OldEmail {
    value: String,
    verified: bool,
    no_code: bool,
}

async fn drop_email(tx: &mut Tx, client_id: ClientId) -> ServiceResult<Vec<OldEmail>> {
    Ok(sqlx::query_as(&format!(
        "DELETE FROM client_identities WHERE client_id = $1 AND kind = 'email' \
         RETURNING value, verified, meta ? '{LINKED_WITHOUT_CODE}' AS no_code"
    ))
    .bind(client_id.0)
    .fetch_all(tx.conn())
    .await?)
}

#[derive(sqlx::FromRow)]
struct ChangeRow {
    id: i64,
    code_hash: Vec<u8>,
    attempts: i32,
    expires_at: DateTime<Utc>,
}

fn code_needed() -> ServiceError {
    ServiceError::rejected(
        EMAIL_CODE_NEEDED,
        "Почта привязывается по коду из письма. Откройте этот раздел заново и попробуйте ещё раз",
    )
}

fn change_off() -> ServiceError {
    ServiceError::rejected(
        EMAIL_CHANGE_OFF,
        "Письма сейчас не отправляются, поэтому сменить почту нельзя",
    )
}

fn in_use() -> ServiceError {
    ServiceError::rejected(EMAIL_IN_USE, "Эта почта уже привязана к другому аккаунту")
}

fn same_email() -> ServiceError {
    ServiceError::rejected(EMAIL_SAME, "Эта почта уже указана в вашем аккаунте")
}

fn change_hash(client_id: ClientId, email: &str, code: &str) -> Vec<u8> {
    token_hash(&format!("email_change:{}:{email}:{code}", client_id.0))
}

fn proof_hash(client_id: ClientId, email: &str, code: &str) -> Vec<u8> {
    token_hash(&format!("email_proof:{}:{email}:{code}", client_id.0))
}

async fn confirmed_email(db: &mut impl Db, client_id: ClientId) -> ServiceResult<Option<String>> {
    Ok(db
        .clients()
        .identities(client_id)
        .await?
        .into_iter()
        .find(Identity::takes_letters)
        .map(|i| i.value))
}

fn new_code() -> String {
    let mut rng = rand::rngs::OsRng;
    (0..EMAIL_CODE_DIGITS)
        .map(|_| char::from(b'0' + rng.gen_range(0..10u8)))
        .collect()
}

pub fn masked_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            let first: String = local.chars().take(1).collect();
            format!("{first}***@{domain}")
        }
        None => "***".to_string(),
    }
}

fn source(principal: &ClientPrincipal, meta: &RequestMeta) -> String {
    match principal.channel {
        ClientChannel::Bot => format!("bot:{}", principal.client_id.0),
        ClientChannel::Cabinet | ClientChannel::Miniapp => {
            crate::limits::address_key(meta.ip.as_deref())
        }
    }
}

fn start_rule(limits: &RateLimits<'_>, principal: &ClientPrincipal) -> RateRule {
    match principal.channel {
        ClientChannel::Bot => rules::EMAIL_START_IP,
        ClientChannel::Cabinet | ClientChannel::Miniapp => limits.email_start(),
    }
}

async fn email_owner(db: &mut impl Db, email: &str) -> ServiceResult<Option<ClientId>> {
    let owner: Option<i64> = sqlx::query_scalar(
        "SELECT client_id FROM client_identities WHERE kind = 'email' AND value = $1",
    )
    .bind(email)
    .fetch_optional(db.conn())
    .await?;
    Ok(owner.map(ClientId))
}

async fn password_hash(db: &mut impl Db, client_id: ClientId) -> ServiceResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT password_hash FROM client_passwords WHERE client_id = $1")
            .bind(client_id.0)
            .fetch_optional(db.conn())
            .await?,
    )
}

impl<'a> Auth<'a> {
    fn links_without_code(&self, identities: &[Identity]) -> bool {
        self.services.settings().client_access.site_login
            && self.services.mail().link_without_code()
            && identities.iter().any(|i| i.kind != IdentityKind::Email)
            && !identities.iter().any(Identity::takes_letters)
    }

    pub fn email_editable(&self, identities: &[Identity]) -> bool {
        self.services.mail().available() || self.links_without_code(identities)
    }

    pub async fn email_proof(&self, client_id: ClientId) -> ServiceResult<Option<EmailProof>> {
        let mut conn = self.services.store().conn().await?;
        if conn.clients().totp(client_id).await?.is_some() {
            return Ok(Some(EmailProof::Totp));
        }
        if self.services.mail().code_login() {
            return Ok(None);
        }
        Ok(password_hash(&mut conn, client_id)
            .await?
            .map(|_| EmailProof::Password))
    }

    pub async fn email_state(&self, client_id: ClientId) -> ServiceResult<EmailState> {
        let available = self.services.mail().available();
        let proof = self.email_proof(client_id).await?;
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        let identities = conn.clients().identities(client_id).await?;
        let without_code = self.links_without_code(&identities);
        let unconfirmed = identities.iter().any(Identity::linked_without_code);
        let email = identities
            .into_iter()
            .find(|i| i.kind == IdentityKind::Email)
            .map(|i| i.value);
        let pending: Option<(String, DateTime<Utc>)> = if available {
            sqlx::query_as(
                "SELECT email, expires_at FROM client_login_requests \
                 WHERE method = 'email_change' AND client_id = $1 AND status = 'pending' \
                 AND expires_at > $2 AND attempts < $3 ORDER BY id DESC LIMIT 1",
            )
            .bind(client_id.0)
            .bind(now)
            .bind(EMAIL_CODE_MAX_ATTEMPTS)
            .fetch_optional(conn.conn())
            .await?
        } else {
            None
        };
        let confirm_current = proof.is_none() && email.is_some() && !unconfirmed;
        Ok(EmailState {
            email,
            available,
            without_code,
            unconfirmed,
            proof,
            confirm_current,
            pending: pending.map(|(email, expires_at)| EmailPending { email, expires_at }),
        })
    }

    pub async fn email_proof_start(
        &self,
        principal: &ClientPrincipal,
        meta: &RequestMeta,
    ) -> ServiceResult<EmailPending> {
        let client_id = principal.client_id;
        if !self.services.mail().available() {
            return Err(change_off());
        }
        let current = {
            let mut conn = self.services.store().conn().await?;
            confirmed_email(&mut conn, client_id).await?
        };
        let Some(current) = current.filter(|_| principal.channel != ClientChannel::Bot) else {
            return Err(ServiceError::rejected(
                CURRENT_CODE_NOT_NEEDED,
                "Подтверждать текущую почту не нужно",
            ));
        };
        if self.email_proof(client_id).await?.is_some() {
            return Err(ServiceError::rejected(
                CURRENT_CODE_NOT_NEEDED,
                "Подтверждать текущую почту не нужно",
            ));
        }
        let limits = self.services.limits();
        limits
            .hit(rules::EMAIL_CHANGE_CLIENT, &client_id.to_string())
            .await?;
        limits
            .hit(start_rule(&limits, principal), &source(principal, meta))
            .await?;
        limits.hit(rules::EMAIL_START_ADDRESS, &current).await?;
        limits
            .letters_sent(
                crate::login::CHANGE_LETTERS,
                self.services.mail().codes_per_hour(),
            )
            .await?;
        let code = new_code();
        let now = self.services.now();
        let expires_at = now + Duration::seconds(EMAIL_CODE_TTL_SECONDS);
        let mut tx = self.services.store().begin().await?;
        sqlx::query(
            "UPDATE client_login_requests SET status = 'rejected' \
             WHERE method = 'email_proof' AND client_id = $1 AND status = 'pending'",
        )
        .bind(client_id.0)
        .execute(tx.conn())
        .await?;
        sqlx::query(
            "INSERT INTO client_login_requests (method, code_hash, email, client_id, ip, user_agent, created_at, expires_at) \
             VALUES ('email_proof', $1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(proof_hash(client_id, &current, &code))
        .bind(&current)
        .bind(client_id.0)
        .bind(&meta.ip)
        .bind(&meta.user_agent)
        .bind(now)
        .bind(expires_at)
        .execute(tx.conn())
        .await?;
        tx.commit().await?;
        self.services
            .letters()
            .send(
                LetterId::EmailCode,
                &current,
                crate::login::code_values(code),
            )
            .await?;
        Ok(EmailPending {
            email: current,
            expires_at,
        })
    }

    async fn check_current_code(
        &self,
        client_id: ClientId,
        current: &str,
        code: Option<&str>,
    ) -> ServiceResult<()> {
        let code: String = code
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if code.is_empty() {
            return Err(ServiceError::rejected(
                CURRENT_CODE_REQUIRED,
                "Сначала подтвердите текущую почту: введите код из письма, которое пришло на неё",
            ));
        }
        if code.len() != EMAIL_CODE_DIGITS || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(code_invalid());
        }
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: Option<ChangeRow> = sqlx::query_as(
            "SELECT id, code_hash, attempts, expires_at FROM client_login_requests \
             WHERE method = 'email_proof' AND client_id = $1 AND email = $2 AND status = 'pending' \
             ORDER BY id DESC LIMIT 1 FOR UPDATE",
        )
        .bind(client_id.0)
        .bind(current)
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
        if !same(&row.code_hash, &proof_hash(client_id, current, &code)) {
            sqlx::query("UPDATE client_login_requests SET attempts = attempts + 1 WHERE id = $1")
                .bind(row.id)
                .execute(tx.conn())
                .await?;
            tx.commit().await?;
            return Err(code_invalid());
        }
        sqlx::query(
            "UPDATE client_login_requests SET status = 'consumed', confirmed_at = $2, consumed_at = $2 \
             WHERE id = $1",
        )
        .bind(row.id)
        .bind(now)
        .execute(tx.conn())
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn check_email_proof(
        &self,
        principal: &ClientPrincipal,
        input: EmailProofInput<'_>,
    ) -> ServiceResult<()> {
        let client_id = principal.client_id;
        match self.email_proof(client_id).await? {
            None if principal.channel == ClientChannel::Bot => Ok(()),
            None => {
                let current = {
                    let mut conn = self.services.store().conn().await?;
                    confirmed_email(&mut conn, client_id).await?
                };
                match current {
                    Some(current) => {
                        self.check_current_code(client_id, &current, input.code)
                            .await
                    }
                    None => Ok(()),
                }
            }
            Some(EmailProof::Totp) => {
                let code = input
                    .code
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| {
                        ServiceError::rejected(
                            APP_CODE_REQUIRED,
                            "Введите код из приложения-аутентификатора",
                        )
                    })?;
                self.verify_client_totp(client_id, code).await
            }
            Some(EmailProof::Password) => {
                let password = input.password.filter(|p| !p.is_empty()).ok_or_else(|| {
                    ServiceError::rejected(PASSWORD_REQUIRED, "Введите пароль для сайта")
                })?;
                let hash = {
                    let mut conn = self.services.store().conn().await?;
                    password_hash(&mut conn, client_id).await?
                };
                if check_password(password, hash).await? {
                    Ok(())
                } else {
                    Err(ServiceError::rejected(
                        PASSWORD_INVALID,
                        "Пароль для сайта не подошёл",
                    ))
                }
            }
        }
    }

    pub async fn email_change_start(
        &self,
        principal: &ClientPrincipal,
        email: &str,
        proof: EmailProofInput<'_>,
        meta: &RequestMeta,
    ) -> ServiceResult<EmailPending> {
        let client_id = principal.client_id;
        let mail = self.services.mail();
        if !mail.available() {
            return Err(change_off());
        }
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit(rules::EMAIL_CHANGE_CLIENT, &client_id.to_string())
            .await?;
        limits
            .hit(start_rule(&limits, principal), &source(principal, meta))
            .await?;
        limits.hit(rules::EMAIL_START_ADDRESS, &email).await?;
        let confirms = {
            let mut conn = self.services.store().conn().await?;
            let current = conn.clients().identities(client_id).await?;
            let same = current
                .iter()
                .find(|i| i.kind == IdentityKind::Email && i.value == email);
            if same.is_some_and(Identity::takes_letters) {
                return Err(same_email());
            }
            if email_owner(&mut conn, &email)
                .await?
                .is_some_and(|owner| owner != client_id)
            {
                return Err(in_use());
            }
            same.is_some()
        };
        if !confirms {
            self.check_email_proof(principal, proof).await?;
        }
        limits
            .letters_sent(crate::login::CHANGE_LETTERS, mail.codes_per_hour())
            .await?;
        let code = new_code();
        let now = self.services.now();
        let expires_at = now + Duration::seconds(EMAIL_CODE_TTL_SECONDS);
        let mut tx = self.services.store().begin().await?;
        sqlx::query(
            "UPDATE client_login_requests SET status = 'rejected' \
             WHERE method = 'email_change' AND client_id = $1 AND status = 'pending'",
        )
        .bind(client_id.0)
        .execute(tx.conn())
        .await?;
        let request_id: i64 = sqlx::query_scalar(
            "INSERT INTO client_login_requests (method, code_hash, email, client_id, ip, user_agent, created_at, expires_at) \
             VALUES ('email_change', $1, $2, $3, $4, $5, $6, $7) RETURNING id",
        )
        .bind(change_hash(client_id, &email, &code))
        .bind(&email)
        .bind(client_id.0)
        .bind(&meta.ip)
        .bind(&meta.user_agent)
        .bind(now)
        .bind(expires_at)
        .fetch_one(tx.conn())
        .await?;
        tx.commit().await?;
        let sent = self
            .services
            .letters()
            .send(LetterId::EmailCode, &email, crate::login::code_values(code))
            .await;
        if let Err(err) = sent {
            let mut conn = self.services.store().conn().await?;
            sqlx::query(
                "UPDATE client_login_requests SET status = 'rejected' WHERE id = $1 AND status = 'pending'",
            )
            .bind(request_id)
            .execute(conn.conn())
            .await?;
            return Err(err);
        }
        Ok(EmailPending { email, expires_at })
    }

    pub async fn email_change_confirm(
        &self,
        principal: &ClientPrincipal,
        email: &str,
        code: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<EmailState> {
        let client_id = principal.client_id;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit(rules::EMAIL_VERIFY_IP, &source(principal, meta))
            .await?;
        limits.hit(rules::EMAIL_VERIFY_ADDRESS, &email).await?;
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        if code.len() != EMAIL_CODE_DIGITS || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(code_invalid());
        }
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: Option<ChangeRow> = sqlx::query_as(
            "SELECT id, code_hash, attempts, expires_at FROM client_login_requests \
             WHERE method = 'email_change' AND client_id = $1 AND email = $2 AND status = 'pending' \
             ORDER BY id DESC LIMIT 1 FOR UPDATE",
        )
        .bind(client_id.0)
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
        if !same(&row.code_hash, &change_hash(client_id, &email, &code)) {
            sqlx::query("UPDATE client_login_requests SET attempts = attempts + 1 WHERE id = $1")
                .bind(row.id)
                .execute(tx.conn())
                .await?;
            tx.commit().await?;
            return Err(code_invalid());
        }
        let client = tx.clients().lock(client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        if email_owner(&mut tx, &email)
            .await?
            .is_some_and(|owner| owner != client_id)
        {
            sqlx::query("UPDATE client_login_requests SET status = 'rejected' WHERE id = $1")
                .bind(row.id)
                .execute(tx.conn())
                .await?;
            tx.commit().await?;
            return Err(in_use());
        }
        let removed = drop_email(&mut tx, client_id).await?;
        let unconfirmed = removed.iter().any(|old| !old.verified && !old.no_code);
        if !tx
            .clients()
            .add_identity_if_free(
                client_id,
                &NewIdentity::new(IdentityKind::Email, &email, true)?,
            )
            .await?
        {
            return Err(in_use());
        }
        if unconfirmed {
            crate::oauth::unlink_unconfirmed(&mut tx, client_id).await?;
        }
        sqlx::query(
            "UPDATE client_login_requests SET status = 'consumed', confirmed_at = $2, consumed_at = $2 \
             WHERE id = $1",
        )
        .bind(row.id)
        .bind(now)
        .execute(tx.conn())
        .await?;
        let (same, previous): (Vec<OldEmail>, Vec<OldEmail>) =
            removed.into_iter().partition(|old| old.value == email);
        let mut entry = Actor::Client(client_id).audit(
            EMAIL_CHANGED_AUDIT,
            "client",
            Some(client_id.0),
            serde_json::json!({
                "email": email,
                "previous": previous.first().map(|old| &old.value),
                "confirmed": !same.is_empty(),
                "channel": principal.channel,
            }),
        );
        entry.ip = meta.ip.clone();
        tx.audit().record(&entry).await?;
        tx.commit().await?;
        for old in previous.iter().filter(|old| old.verified || !old.no_code) {
            self.notify_email_changed(&old.value, &email).await;
        }
        self.email_state(client_id).await
    }

    pub async fn email_link(
        &self,
        principal: &ClientPrincipal,
        email: &str,
        proof: EmailProofInput<'_>,
        meta: &RequestMeta,
    ) -> ServiceResult<EmailState> {
        let client_id = principal.client_id;
        if self.services.mail().available() {
            return Err(code_needed());
        }
        if !self.services.mail().link_without_code() {
            return Err(change_off());
        }
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit(rules::EMAIL_CHANGE_CLIENT, &client_id.to_string())
            .await?;
        limits
            .hit(start_rule(&limits, principal), &source(principal, meta))
            .await?;
        limits.hit(rules::EMAIL_START_ADDRESS, &email).await?;
        {
            let mut conn = self.services.store().conn().await?;
            let identities = conn.clients().identities(client_id).await?;
            self.check_link(&mut conn, client_id, &identities, &email)
                .await?;
        }
        self.check_email_proof(principal, proof).await?;
        let mut tx = self.services.store().begin().await?;
        let client = tx.clients().lock(client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let identities = tx.clients().identities(client_id).await?;
        self.check_link(&mut tx, client_id, &identities, &email)
            .await?;
        let removed = drop_email(&mut tx, client_id).await?;
        if !tx
            .clients()
            .add_identity_if_free(client_id, &NewIdentity::email_without_code(&email)?)
            .await?
        {
            return Err(in_use());
        }
        let mut entry = Actor::Client(client_id).audit(
            EMAIL_LINKED_AUDIT,
            "client",
            Some(client_id.0),
            serde_json::json!({
                "email": email,
                "previous": removed.first().map(|old| &old.value),
                "channel": principal.channel,
            }),
        );
        entry.ip = meta.ip.clone();
        tx.audit().record(&entry).await?;
        tx.commit().await?;
        self.email_state(client_id).await
    }

    async fn check_link(
        &self,
        db: &mut impl Db,
        client_id: ClientId,
        identities: &[Identity],
        email: &str,
    ) -> ServiceResult<()> {
        if !self.links_without_code(identities) {
            return Err(change_off());
        }
        if identities
            .iter()
            .any(|i| i.kind == IdentityKind::Email && i.value == email)
        {
            return Err(same_email());
        }
        if email_owner(db, email)
            .await?
            .is_some_and(|owner| owner != client_id)
        {
            return Err(in_use());
        }
        Ok(())
    }

    async fn notify_email_changed(&self, old: &str, new: &str) {
        let sent = self
            .services
            .letters()
            .send(
                LetterId::EmailChanged,
                old,
                Values::new().with("new_email", masked_email(new)),
            )
            .await;
        if let Err(err) = sent {
            tracing::warn!(error = %err, "уведомление о смене почты на прежний адрес не отправилось");
        }
    }
}
