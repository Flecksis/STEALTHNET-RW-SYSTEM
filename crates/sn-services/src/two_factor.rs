use crate::Actor;
use crate::auth::{Auth, ClientLogin, RequestMeta, TotpSetup, token_hash};
use crate::clients::ClientOps;
use crate::error::{ServiceError, ServiceResult};
use crate::limits::{Strike, rules};
use crate::login::issue_session;
use crate::totp;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::Serialize;
use sn_domain::client::{Identity, IdentityKind};
use sn_domain::ids::ClientId;
use sn_store::{ClientTotp, Db, SealedSecret, SessionChannel, Tx};

pub const TOTP_CHALLENGE_TTL_SECONDS: i64 = 300;
pub const TOTP_CHALLENGE_ATTEMPTS: i32 = 5;
pub const TOTP_NOT_ENABLED: &str = "client.totp_not_enabled";

const LOCKED: &str = "Код из приложения слишком часто не подходил, вход по коду временно закрыт.";
const WRONG_CODE: &str = "Код не подошёл. Проверьте время на телефоне";
const USED_CODE: &str = "Этот код уже использован, дождитесь следующего";

pub fn client_totp_context(id: ClientId) -> String {
    format!("clients.{}.totp", id.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TwoFactor {
    pub enabled: bool,
    pub enabled_at: Option<DateTime<Utc>>,
}

impl TwoFactor {
    pub fn of(totp: Option<&ClientTotp>) -> Self {
        TwoFactor {
            enabled: totp.is_some(),
            enabled_at: totp.and_then(|t| t.enabled_at),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename = "totp_required")]
pub struct TotpChallenge {
    pub challenge: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum CabinetLogin {
    Session(ClientLogin),
    TotpRequired(TotpChallenge),
}

fn code_rejected(message: &'static str) -> ServiceError {
    ServiceError::rejected("auth.totp_code", message)
}

fn challenge_expired() -> ServiceError {
    ServiceError::rejected(
        "auth.login_expired",
        "Время на ввод кода вышло. Войдите заново.",
    )
}

fn already_enabled() -> ServiceError {
    ServiceError::rejected("auth.totp_enabled", "Вход по коду уже включён")
}

fn not_enabled() -> ServiceError {
    ServiceError::rejected("auth.totp_disabled", "Вход по коду не включён")
}

fn challenge_token() -> String {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    hex::encode(raw)
}

fn account_label(identities: &[Identity], id: ClientId) -> String {
    let telegram = identities.iter().find(|i| i.kind == IdentityKind::Telegram);
    let username = telegram
        .and_then(|i| i.meta.get("username"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|u| !u.is_empty());
    if let Some(username) = username {
        return format!("@{username}");
    }
    if let Some(email) = identities.iter().find(|i| i.kind == IdentityKind::Email) {
        return email.value.clone();
    }
    let provider_email = identities
        .iter()
        .filter(|i| matches!(i.kind, IdentityKind::Google | IdentityKind::Apple))
        .find_map(|i| i.meta.get("email").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|e| !e.is_empty());
    if let Some(email) = provider_email {
        return email.to_string();
    }
    match telegram {
        Some(identity) => format!("Telegram {}", identity.value),
        None => format!("клиент {}", id.0),
    }
}

fn normalize_secret(raw: &str) -> ServiceResult<String> {
    let secret: String = raw
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
    Ok(secret)
}

pub(crate) async fn reject_challenges(
    tx: &mut Tx,
    client_id: ClientId,
    now: DateTime<Utc>,
) -> ServiceResult<u64> {
    let done = sqlx::query(
        "UPDATE client_login_requests SET status = 'rejected' \
         WHERE method = 'totp' AND client_id = $1 AND status = 'pending' AND expires_at > $2",
    )
    .bind(client_id.0)
    .bind(now)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

pub(crate) async fn cabinet_session(
    tx: &mut Tx,
    client_id: ClientId,
    now: DateTime<Utc>,
    meta: &RequestMeta,
    created: bool,
) -> ServiceResult<CabinetLogin> {
    if tx.clients().totp(client_id).await?.is_none() {
        return Ok(CabinetLogin::Session(
            issue_session(tx, client_id, now, meta, created).await?,
        ));
    }
    let challenge = challenge_token();
    let expires_at = now + Duration::seconds(TOTP_CHALLENGE_TTL_SECONDS);
    sqlx::query(
        "INSERT INTO client_login_requests (method, code_hash, client_id, ip, user_agent, created_at, expires_at) \
         VALUES ('totp', $1, $2, $3, $4, $5, $6)",
    )
    .bind(token_hash(&challenge))
    .bind(client_id.0)
    .bind(&meta.ip)
    .bind(&meta.user_agent)
    .bind(now)
    .bind(expires_at)
    .execute(tx.conn())
    .await?;
    Ok(CabinetLogin::TotpRequired(TotpChallenge {
        challenge,
        expires_at,
    }))
}

#[derive(sqlx::FromRow)]
struct ChallengeRow {
    id: i64,
    status: String,
    client_id: Option<i64>,
    attempts: i32,
    expires_at: DateTime<Utc>,
}

impl<'a> Auth<'a> {
    fn open_client_totp(&self, id: ClientId, sealed: &SealedSecret) -> ServiceResult<String> {
        let secrets = self.services.secrets().ok_or(ServiceError::NoSecretKey)?;
        Ok(secrets.open_string(&client_totp_context(id), sealed)?)
    }

    async fn client_totp_attempt(&self, client_id: ClientId) -> ServiceResult<[Strike; 2]> {
        let id = client_id.to_string();
        self.strike_both(
            (rules::CLIENT_TOTP_FAILURES, &id),
            (rules::CLIENT_TOTP_FAILURES_DAY, &id),
            LOCKED,
        )
        .await
    }

    async fn settle_client_totp(
        &self,
        client_id: ClientId,
        strikes: &[Strike],
        accepted: bool,
        ip: Option<&str>,
    ) -> ServiceResult<()> {
        if accepted {
            return self.forgive(strikes).await;
        }
        if let Some(strike) = strikes.iter().find(|s| s.reached_limit()) {
            tracing::warn!(
                client_id = client_id.0,
                ip = ip.unwrap_or("unknown"),
                failures = strike.failures,
                rule = strike.rule.scope,
                "вход клиента по коду временно закрыт: серия неверных кодов"
            );
        }
        Ok(())
    }

    async fn client_security_hit(&self, client_id: ClientId) -> ServiceResult<()> {
        self.services
            .limits()
            .hit(rules::CLIENT_SECURITY, &client_id.to_string())
            .await?;
        Ok(())
    }

    pub async fn client_two_factor(&self, client_id: ClientId) -> ServiceResult<TwoFactor> {
        let mut conn = self.services.store().conn().await?;
        Ok(TwoFactor::of(
            conn.clients().totp(client_id).await?.as_ref(),
        ))
    }

    pub async fn client_totp_setup(&self, client_id: ClientId) -> ServiceResult<TotpSetup> {
        self.client_security_hit(client_id).await?;
        let mut conn = self.services.store().conn().await?;
        if conn.clients().totp(client_id).await?.is_some() {
            return Err(already_enabled());
        }
        let identities = conn.clients().identities(client_id).await?;
        let secret = totp::generate_secret();
        let uri = totp::provisioning_uri(
            &secret,
            &account_label(&identities, client_id),
            &self.services.settings().brand_name,
        );
        Ok(TotpSetup { secret, uri })
    }

    pub async fn client_totp_enable(
        &self,
        client_id: ClientId,
        secret: &str,
        code: &str,
    ) -> ServiceResult<TwoFactor> {
        self.client_security_hit(client_id).await?;
        let secret = normalize_secret(secret)?;
        let now = self.services.now();
        let step = totp::verify(&secret, code, now.timestamp())
            .ok_or_else(|| code_rejected(WRONG_CODE))?;
        let secrets = self.services.secrets().ok_or(ServiceError::NoSecretKey)?;
        let sealed = secrets.seal_str(&client_totp_context(client_id), &secret)?;
        let mut tx = self.services.store().begin().await?;
        tx.clients().lock(client_id).await?;
        if tx.clients().totp(client_id).await?.is_some() {
            return Err(already_enabled());
        }
        tx.clients().enable_totp(client_id, &sealed, now).await?;
        tx.clients().use_totp_step(client_id, step).await?;
        tx.audit()
            .record(&Actor::Client(client_id).audit(
                "client.totp_enabled",
                "client",
                Some(client_id.0),
                serde_json::json!({}),
            ))
            .await?;
        tx.commit().await?;
        Ok(TwoFactor {
            enabled: true,
            enabled_at: Some(now),
        })
    }

    pub async fn client_totp_disable(
        &self,
        client_id: ClientId,
        code: &str,
    ) -> ServiceResult<TwoFactor> {
        let totp = {
            let mut conn = self.services.store().conn().await?;
            conn.clients()
                .totp(client_id)
                .await?
                .ok_or_else(not_enabled)?
        };
        let secret = self.open_client_totp(client_id, &totp.secret)?;
        let attempt = self.client_totp_attempt(client_id).await?;
        let Some(step) = totp::verify(&secret, code, self.services.now().timestamp()) else {
            self.settle_client_totp(client_id, &attempt, false, None)
                .await?;
            return Err(code_rejected(WRONG_CODE));
        };
        let mut tx = self.services.store().begin().await?;
        if !tx.clients().use_totp_step(client_id, step).await? {
            drop(tx);
            self.settle_client_totp(client_id, &attempt, false, None)
                .await?;
            return Err(code_rejected(USED_CODE));
        }
        tx.clients().clear_totp(client_id).await?;
        reject_challenges(&mut tx, client_id, self.services.now()).await?;
        tx.audit()
            .record(&Actor::Client(client_id).audit(
                "client.totp_disabled",
                "client",
                Some(client_id.0),
                serde_json::json!({}),
            ))
            .await?;
        tx.commit().await?;
        self.settle_client_totp(client_id, &attempt, true, None)
            .await?;
        Ok(TwoFactor {
            enabled: false,
            enabled_at: None,
        })
    }

    pub async fn cabinet_totp_login(
        &self,
        challenge: &str,
        code: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<ClientLogin> {
        self.services
            .limits()
            .hit_ip(rules::CLIENT_TOTP_IP, meta.ip.as_deref())
            .await?;
        let challenge = challenge.trim();
        if challenge.is_empty() {
            return Err(challenge_expired());
        }
        let now = self.services.now();
        let hash = token_hash(challenge);
        let (row, client_id, totp) = {
            let mut conn = self.services.store().conn().await?;
            let row: Option<ChallengeRow> = sqlx::query_as(
                "SELECT id, status, client_id, attempts, expires_at FROM client_login_requests \
                 WHERE method = 'totp' AND code_hash = $1",
            )
            .bind(&hash)
            .fetch_optional(conn.conn())
            .await?;
            let Some(row) = row.filter(|r| r.status == "pending" && r.expires_at > now) else {
                return Err(challenge_expired());
            };
            let client_id = ClientId(
                row.client_id
                    .ok_or(ServiceError::Internal("запрос кода без клиента".into()))?,
            );
            let totp = conn
                .clients()
                .totp(client_id)
                .await?
                .ok_or_else(challenge_expired)?;
            (row, client_id, totp)
        };
        if row.attempts >= TOTP_CHALLENGE_ATTEMPTS {
            return Err(ServiceError::rejected(
                "auth.code_attempts",
                "Слишком много неверных кодов. Войдите заново.",
            ));
        }
        let secret = self.open_client_totp(client_id, &totp.secret)?;
        let attempt = self.client_totp_attempt(client_id).await?;
        let step = totp::verify(&secret, code, now.timestamp());
        let mut tx = self.services.store().begin().await?;
        let locked: ChallengeRow = sqlx::query_as(
            "SELECT id, status, client_id, attempts, expires_at FROM client_login_requests \
             WHERE id = $1 FOR UPDATE",
        )
        .bind(row.id)
        .fetch_one(tx.conn())
        .await?;
        if locked.status != "pending" || locked.expires_at <= now {
            drop(tx);
            self.forgive(&attempt).await?;
            return Err(challenge_expired());
        }
        let client = tx.clients().get(client_id).await?;
        if client.is_blocked() {
            drop(tx);
            self.forgive(&attempt).await?;
            return Err(ServiceError::Forbidden);
        }
        let accepted = match step {
            Some(step) => tx.clients().use_totp_step(client_id, step).await?,
            None => false,
        };
        if !accepted {
            sqlx::query("UPDATE client_login_requests SET attempts = attempts + 1 WHERE id = $1")
                .bind(locked.id)
                .execute(tx.conn())
                .await?;
            tx.commit().await?;
            self.settle_client_totp(client_id, &attempt, false, meta.ip.as_deref())
                .await?;
            return Err(code_rejected(if step.is_some() {
                USED_CODE
            } else {
                WRONG_CODE
            }));
        }
        sqlx::query(
            "UPDATE client_login_requests SET status = 'consumed', confirmed_at = $2, consumed_at = $2 \
             WHERE id = $1",
        )
        .bind(locked.id)
        .bind(now)
        .execute(tx.conn())
        .await?;
        let login = issue_session(&mut tx, client_id, now, meta, false).await?;
        tx.commit().await?;
        self.settle_client_totp(client_id, &attempt, true, meta.ip.as_deref())
            .await?;
        Ok(login)
    }
}

impl<'a> ClientOps<'a> {
    pub async fn reset_two_factor(&self, id: ClientId, actor: Actor) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        tx.clients().lock(id).await?;
        if !tx.clients().clear_totp(id).await? {
            return Err(ServiceError::rejected(
                TOTP_NOT_ENABLED,
                "У клиента не включена 2FA",
            ));
        }
        let challenges = reject_challenges(&mut tx, id, self.services.now()).await?;
        let sessions = tx
            .sessions()
            .revoke_client_channel(id, SessionChannel::Cabinet)
            .await?;
        tx.audit()
            .record(&actor.audit(
                "client.totp_reset",
                "client",
                Some(id.0),
                serde_json::json!({
                    "revoked_sessions": sessions,
                    "rejected_logins": challenges,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
