pub mod protocol;
#[cfg(test)]
mod tests;

pub use protocol::{Endpoints, IdClaims, OauthEndpoints, ProtocolError};

use crate::Actor;
use crate::auth::{ClientLogin, RequestMeta, token_hash};
use crate::error::{ServiceError, ServiceResult};
use crate::limits::rules;
use crate::login::referral_code;
use crate::services::Services;
use crate::two_factor::{CabinetLogin, cabinet_session};
use crate::utm_tracking::site_param;
use chrono::{DateTime, Duration, Utc};
use protocol::{AppleName, AuthorizeRequest, Credentials};
use rand::RngCore;
use serde::Serialize;
use sn_domain::access::ClientPrincipal;
use sn_domain::client::{
    Identity, IdentityKind, NewIdentity, RegistrationSource, normalize_identity,
};
use sn_domain::events::DomainEvent;
use sn_domain::ids::ClientId;
use sn_domain::jobs::JobPayload;
use sn_domain::module::ModuleId;
use sn_modules::catalog::oauth::{OauthSettings, Provider, TEMPLATE_LINKED, TEMPLATE_UNLINKED};
use sn_modules::catalog::start_param::{Utm, with_referral};
use sn_store::{Db, NewClient, Tx};

pub const COOKIE: &str = "sn_oauth";
pub const COOKIE_PATH: &str = "/api/client/auth/oauth";
pub const REQUEST_TTL_SECONDS: i64 = 600;
pub const FINISH_SECONDS: i64 = 300;
const RETENTION_DAYS: i64 = 7;
const LOCK_SPACE: i32 = 0x6f61;
pub const EMAIL_UNVERIFIED: &str = "oauth.email_unverified";
pub const UNLINKED_ON_CONFIRM_AUDIT: &str = "client.oauth_unlinked_on_confirm";

const CODES: [&str; 16] = [
    "oauth.canceled",
    "oauth.expired",
    "oauth.refused",
    "oauth.unavailable",
    "oauth.bad_token",
    "oauth.bad_reply",
    "oauth.blocked",
    "oauth.email_taken",
    "oauth.other_account",
    "oauth.taken",
    "oauth.already_linked",
    "oauth.session_gone",
    "oauth.not_configured",
    "oauth.no_request",
    "oauth.failed",
    EMAIL_UNVERIFIED,
];

fn unconfirmed_only(identities: &[Identity]) -> bool {
    let other = identities.iter().any(|i| {
        matches!(
            i.kind,
            IdentityKind::Telegram | IdentityKind::Google | IdentityKind::Apple
        )
    });
    !other
        && identities
            .iter()
            .any(|i| i.kind == IdentityKind::Email && !i.verified)
}

fn unconfirmed_text(provider: Provider) -> String {
    format!(
        "Сначала подтвердите почту: войдите по коду из письма или восстановите пароль через «Забыли пароль». После этого привяжите вход через {}.",
        provider.title()
    )
}

pub(crate) async fn unlink_unconfirmed(tx: &mut Tx, client_id: ClientId) -> ServiceResult<usize> {
    let removed: Vec<String> = sqlx::query_scalar(
        "DELETE FROM client_identities WHERE client_id = $1 AND kind IN ('google', 'apple') RETURNING kind",
    )
    .bind(client_id.0)
    .fetch_all(tx.conn())
    .await?;
    if removed.is_empty() {
        return Ok(0);
    }
    let revoked = tx.sessions().revoke_all_client(client_id).await?;
    tx.jobs()
        .enqueue(
            &JobPayload::Notify {
                client_id,
                template: TEMPLATE_UNLINKED.into(),
                params: serde_json::json!({ "providers": removed }),
            },
            None,
        )
        .await?;
    tx.audit()
        .record(&Actor::Client(client_id).audit(
            UNLINKED_ON_CONFIRM_AUDIT,
            "client",
            Some(client_id.0),
            serde_json::json!({ "providers": removed, "revoked_sessions": revoked }),
        ))
        .await?;
    tracing::warn!(client = %client_id, providers = ?removed, "почту подтвердили кодом: вход через Google или Apple, привязанный до этого, отвязан");
    Ok(removed.len())
}

pub(crate) async fn confirm_email(
    tx: &mut Tx,
    client_id: ClientId,
    email: &str,
) -> ServiceResult<usize> {
    let confirmed: Option<String> = sqlx::query_scalar(
        "UPDATE client_identities SET verified = true \
         WHERE client_id = $1 AND kind = 'email' AND value = $2 AND NOT verified RETURNING value",
    )
    .bind(client_id.0)
    .bind(email)
    .fetch_optional(tx.conn())
    .await?;
    if confirmed.is_none() {
        return Ok(0);
    }
    unlink_unconfirmed(tx, client_id).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Login,
    Link,
}

impl Purpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Purpose::Login => "login",
            Purpose::Link => "link",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OauthStart {
    pub url: String,
    #[serde(skip)]
    pub browser: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub user: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallbackOutcome {
    pub purpose: Purpose,
    pub provider: Provider,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OauthFinish {
    Confirmed {
        login: ClientLogin,
    },
    TotpRequired {
        challenge: String,
        expires_at: DateTime<Utc>,
    },
    Linked {
        provider: Provider,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Availability {
    pub google: bool,
    pub apple: bool,
}

impl Availability {
    pub const fn has(self, provider: Provider) -> bool {
        match provider {
            Provider::Google => self.google,
            Provider::Apple => self.apple,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoginMethod {
    pub kind: IdentityKind,
    pub account: String,
    pub linked_at: DateTime<Utc>,
    pub removable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoginMethods {
    pub methods: Vec<LoginMethod>,
    pub can_link: Vec<Provider>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Refusal {
    code: &'static str,
    message: String,
}

impl Refusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Refusal {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Done {
    client_id: ClientId,
    created: bool,
}

#[derive(sqlx::FromRow)]
struct RequestRow {
    id: i64,
    provider: String,
    purpose: String,
    browser_hash: Vec<u8>,
    client_id: Option<i64>,
    session_id: Option<i64>,
    referral: Option<String>,
    utm: Option<String>,
    status: String,
    result_client_id: Option<i64>,
    created_client: bool,
    error_code: Option<String>,
    error_text: Option<String>,
    expires_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

const REQUEST_COLUMNS: &str = "id, provider, purpose, browser_hash, client_id, session_id, referral, utm, status, \
     result_client_id, created_client, error_code, error_text, expires_at, finished_at";

impl RequestRow {
    fn purpose(&self) -> Purpose {
        if self.purpose == "link" {
            Purpose::Link
        } else {
            Purpose::Login
        }
    }

    fn provider(&self) -> Option<Provider> {
        Provider::parse(&self.provider)
    }
}

fn random_hex() -> String {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    hex::encode(raw)
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn stored_code(code: Option<&str>) -> &'static str {
    code.and_then(|code| CODES.into_iter().find(|known| *known == code))
        .unwrap_or("oauth.failed")
}

fn no_request() -> ServiceError {
    ServiceError::rejected(
        "oauth.no_request",
        "Вход не завершён. Нажмите кнопку входа ещё раз.",
    )
}

fn not_configured(provider: Provider) -> ServiceError {
    ServiceError::rejected(
        "oauth.not_configured",
        format!("Вход через {} сейчас недоступен", provider.title()),
    )
}

fn blocked() -> Refusal {
    Refusal::new(
        "oauth.blocked",
        "Аккаунт заблокирован. Если это ошибка, напишите в поддержку.",
    )
}

fn public_base(url: Option<&str>) -> Option<(String, bool)> {
    let base = url?.trim().trim_end_matches('/').to_string();
    if base.starts_with("https://") {
        return Some((base, true));
    }
    let host = base.strip_prefix("http://")?.split(['/', ':']).next()?;
    matches!(host, "localhost" | "127.0.0.1" | "[::1]").then_some((base, false))
}

fn protocol_refusal(provider: Provider, error: &ProtocolError) -> Refusal {
    let title = provider.title();
    match error {
        ProtocolError::Unavailable(_) => Refusal::new(
            "oauth.unavailable",
            format!("{title} сейчас не отвечает. Попробуйте чуть позже."),
        ),
        ProtocolError::Refused(_) => Refusal::new(
            "oauth.refused",
            format!("{title} не подтвердил вход. Попробуйте ещё раз."),
        ),
        ProtocolError::BadToken(_) => Refusal::new(
            "oauth.bad_token",
            format!("Не удалось проверить ответ {title}. Начните вход заново."),
        ),
    }
}

fn identity_meta(
    claims: &IdClaims,
    name: &AppleName,
    previous: Option<&serde_json::Value>,
) -> serde_json::Value {
    let keep = |key: &str| {
        previous
            .and_then(|meta| meta.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let first = name.first_name.clone().or_else(|| keep("first_name"));
    let last = name.last_name.clone().or_else(|| keep("last_name"));
    let full = claims.name.clone().or_else(|| {
        let joined = [first.as_deref(), last.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        (!joined.is_empty()).then_some(joined)
    });
    serde_json::json!({
        "email": claims.email.clone().or_else(|| keep("email")),
        "email_verified": claims.email_verified,
        "private_email": claims.relay(),
        "name": full.or_else(|| keep("name")),
        "first_name": first,
        "last_name": last,
    })
}

async fn notify_linked(tx: &mut Tx, client_id: ClientId, claims: &IdClaims) -> ServiceResult<()> {
    let account = if claims.relay() {
        "почта скрыта".to_string()
    } else {
        claims
            .email
            .clone()
            .unwrap_or_else(|| format!("аккаунт {}", claims.provider.title()))
    };
    tx.jobs()
        .enqueue(
            &JobPayload::Notify {
                client_id,
                template: TEMPLATE_LINKED.to_string(),
                params: serde_json::json!({
                    "provider": claims.provider.title(),
                    "account": account,
                }),
            },
            None,
        )
        .await?;
    Ok(())
}

fn account_title(identity: &Identity) -> String {
    let meta = |key: &str| {
        identity
            .meta
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    match identity.kind {
        IdentityKind::Telegram => meta("username")
            .map(|u| format!("@{u}"))
            .unwrap_or_else(|| format!("Telegram {}", identity.value)),
        IdentityKind::Email => identity.value.clone(),
        IdentityKind::Google => {
            meta("email").map_or_else(|| "аккаунт Google".into(), str::to_string)
        }
        IdentityKind::Apple => {
            let hidden = identity
                .meta
                .get("private_email")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            match meta("email") {
                Some(email) if !hidden => email.to_string(),
                _ if hidden => "почта скрыта".into(),
                _ => "Apple ID".into(),
            }
        }
    }
}

async fn lock_subject(tx: &mut Tx, provider: Provider, sub: &str) -> ServiceResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(LOCK_SPACE)
        .bind(format!("{}:{sub}", provider.as_str()))
        .execute(tx.conn())
        .await?;
    Ok(())
}

async fn has_password(db: &mut impl Db, client_id: ClientId) -> ServiceResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM client_passwords WHERE client_id = $1)")
            .bind(client_id.0)
            .fetch_one(db.conn())
            .await?,
    )
}

pub struct OauthOps<'a> {
    services: &'a Services,
}

impl<'a> OauthOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        OauthOps { services }
    }

    fn configured(&self) -> Option<(OauthSettings, String, bool)> {
        let modules = self.services.modules();
        if !modules.is_enabled(ModuleId::Oauth) {
            return None;
        }
        let settings = OauthSettings::parse(&modules.config(ModuleId::Oauth)).ok()?;
        let (base, https) = public_base(self.services.settings().public_url.as_deref())?;
        Some((settings, base, https))
    }

    pub fn availability(&self) -> Availability {
        if !self.services.settings().client_access.site_login {
            return Availability::default();
        }
        match self.configured() {
            Some((settings, _, https)) => Availability {
                google: settings.google.is_some(),
                apple: settings.apple.is_some() && https,
            },
            None => Availability::default(),
        }
    }

    fn redirect_uri(base: &str, provider: Provider) -> String {
        format!("{base}{}", provider.callback_path())
    }

    async fn begin(
        &self,
        provider: Provider,
        purpose: Purpose,
        owner: Option<(ClientId, i64)>,
        referral: Option<String>,
        utm: Option<String>,
        meta: &RequestMeta,
    ) -> ServiceResult<OauthStart> {
        if !self.availability().has(provider) {
            return Err(not_configured(provider));
        }
        let (settings, base, _) = self.configured().ok_or_else(|| not_configured(provider))?;
        self.services
            .limits()
            .hit_ip(rules::OAUTH_START_IP, meta.ip.as_deref())
            .await?;
        let client_id = match provider {
            Provider::Google => settings.google.as_ref().map(|g| g.client_id.clone()),
            Provider::Apple => settings.apple.as_ref().map(|a| a.services_id.clone()),
        }
        .ok_or_else(|| not_configured(provider))?;
        let state = random_hex();
        let browser = random_hex();
        let endpoints = self.services.oauth_endpoints();
        let redirect_uri = Self::redirect_uri(&base, provider);
        let url = protocol::authorize_url(&AuthorizeRequest {
            provider,
            endpoints: endpoints.of(provider),
            client_id: &client_id,
            redirect_uri: &redirect_uri,
            state: &state,
            browser: &browser,
        })
        .map_err(|err| ServiceError::Internal(err.to_string()))?;
        let now = self.services.now();
        let expires_at = now + Duration::seconds(REQUEST_TTL_SECONDS);
        let mut conn = self.services.store().conn().await?;
        sqlx::query(
            "INSERT INTO oauth_requests (provider, purpose, state_hash, browser_hash, client_id, session_id, referral, utm, ip, user_agent, created_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(provider.as_str())
        .bind(purpose.as_str())
        .bind(token_hash(&state))
        .bind(token_hash(&browser))
        .bind(owner.map(|(client, _)| client.0))
        .bind(owner.map(|(_, session)| session))
        .bind(referral)
        .bind(utm)
        .bind(&meta.ip)
        .bind(&meta.user_agent)
        .bind(now)
        .bind(expires_at)
        .execute(conn.conn())
        .await?;
        sqlx::query("DELETE FROM oauth_requests WHERE expires_at < $1")
            .bind(now - Duration::days(RETENTION_DAYS))
            .execute(conn.conn())
            .await?;
        Ok(OauthStart {
            url,
            browser,
            expires_at,
        })
    }

    pub async fn start_login(
        &self,
        provider: Provider,
        referral: Option<&str>,
        meta: &RequestMeta,
    ) -> ServiceResult<OauthStart> {
        self.start_login_with(provider, referral, None, meta).await
    }

    pub async fn start_login_with(
        &self,
        provider: Provider,
        referral: Option<&str>,
        utm: Option<&Utm>,
        meta: &RequestMeta,
    ) -> ServiceResult<OauthStart> {
        if !self.services.settings().client_access.site_login {
            return Err(ServiceError::rejected(
                "auth.site_off",
                "Вход на сайт выключен: пользуйтесь ботом",
            ));
        }
        self.begin(
            provider,
            Purpose::Login,
            None,
            referral.and_then(referral_code),
            site_param(utm),
            meta,
        )
        .await
    }

    pub async fn start_link(
        &self,
        principal: &ClientPrincipal,
        provider: Provider,
        meta: &RequestMeta,
    ) -> ServiceResult<OauthStart> {
        let session = principal.session_id.ok_or(ServiceError::Forbidden)?;
        let identities = {
            let mut conn = self.services.store().conn().await?;
            conn.clients().identities(principal.client_id).await?
        };
        if unconfirmed_only(&identities) {
            return Err(ServiceError::rejected(
                EMAIL_UNVERIFIED,
                unconfirmed_text(provider),
            ));
        }
        let linked = identities.iter().any(|i| i.kind == provider.identity());
        if linked {
            return Err(ServiceError::rejected(
                "oauth.already_linked",
                format!(
                    "Вход через {} уже привязан. Чтобы сменить аккаунт, сначала отвяжите прежний.",
                    provider.title()
                ),
            ));
        }
        self.begin(
            provider,
            Purpose::Link,
            Some((principal.client_id, session)),
            None,
            None,
            meta,
        )
        .await
    }

    pub async fn callback(
        &self,
        provider: Provider,
        params: &CallbackParams,
        browser: Option<&str>,
    ) -> ServiceResult<CallbackOutcome> {
        let fallback = CallbackOutcome {
            purpose: Purpose::Login,
            provider,
        };
        let Some(state) = params
            .state
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 128)
        else {
            return Ok(fallback);
        };
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: Option<RequestRow> = sqlx::query_as(&format!(
            "SELECT {REQUEST_COLUMNS} FROM oauth_requests WHERE state_hash = $1 AND provider = $2 FOR UPDATE"
        ))
        .bind(token_hash(state))
        .bind(provider.as_str())
        .fetch_optional(tx.conn())
        .await?;
        let Some(row) = row else {
            return Ok(fallback);
        };
        let outcome = CallbackOutcome {
            purpose: row.purpose(),
            provider,
        };
        let Some(browser) = browser
            .map(str::trim)
            .filter(|b| same(&token_hash(b), &row.browser_hash))
        else {
            tracing::info!(
                request = row.id,
                "ответ провайдера входа пришёл в другом браузере"
            );
            return Ok(outcome);
        };
        if row.status != "pending" {
            return Ok(outcome);
        }
        let result = if row.expires_at <= now {
            Err(Refusal::new(
                "oauth.expired",
                "Время на вход вышло. Нажмите кнопку ещё раз.",
            ))
        } else if let Some(error) = params.error.as_deref() {
            tracing::info!(request = row.id, error, "провайдер входа вернул отказ");
            Err(match error {
                "access_denied" | "user_cancelled_authorize" => Refusal::new(
                    "oauth.canceled",
                    format!("Вход через {} отменён.", provider.title()),
                ),
                _ => protocol_refusal(provider, &ProtocolError::Refused(error.to_string())),
            })
        } else if let Some(code) = params.code.as_deref().filter(|c| !c.trim().is_empty()) {
            self.complete(
                &mut tx,
                &row,
                provider,
                code.trim(),
                browser,
                params.user.as_deref(),
                now,
            )
            .await?
        } else {
            Err(Refusal::new(
                "oauth.bad_reply",
                format!(
                    "{} вернул неполный ответ. Начните вход заново.",
                    provider.title()
                ),
            ))
        };
        match &result {
            Ok(done) => {
                sqlx::query(
                    "UPDATE oauth_requests SET status = 'done', result_client_id = $2, created_client = $3, finished_at = $4 WHERE id = $1",
                )
                .bind(row.id)
                .bind(done.client_id.0)
                .bind(done.created)
                .bind(now)
                .execute(tx.conn())
                .await?;
            }
            Err(refusal) => {
                sqlx::query(
                    "UPDATE oauth_requests SET status = 'failed', error_code = $2, error_text = $3, finished_at = $4 WHERE id = $1",
                )
                .bind(row.id)
                .bind(refusal.code)
                .bind(&refusal.message)
                .bind(now)
                .execute(tx.conn())
                .await?;
            }
        }
        tx.commit().await?;
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    async fn complete(
        &self,
        tx: &mut Tx,
        row: &RequestRow,
        provider: Provider,
        code: &str,
        browser: &str,
        user: Option<&str>,
        now: DateTime<Utc>,
    ) -> ServiceResult<Result<Done, Refusal>> {
        let Some((settings, base, _)) = self.configured() else {
            return Ok(Err(Refusal::new(
                "oauth.not_configured",
                format!("Вход через {} сейчас недоступен", provider.title()),
            )));
        };
        let (credentials, audience) = match provider {
            Provider::Google => match settings.google.as_ref() {
                Some(google) => (Credentials::Google(google), google.client_id.as_str()),
                None => {
                    return Ok(Err(Refusal::new(
                        "oauth.not_configured",
                        "Вход через Google сейчас недоступен",
                    )));
                }
            },
            Provider::Apple => match settings.apple.as_ref() {
                Some(apple) => (Credentials::Apple(apple), apple.services_id.as_str()),
                None => {
                    return Ok(Err(Refusal::new(
                        "oauth.not_configured",
                        "Вход через Apple сейчас недоступен",
                    )));
                }
            },
        };
        let endpoints = self.services.oauth_endpoints();
        let endpoints = endpoints.of(provider);
        let redirect_uri = Self::redirect_uri(&base, provider);
        let claims = match protocol::exchange_code(
            endpoints,
            credentials,
            code,
            &redirect_uri,
            browser,
            now,
        )
        .await
        {
            Ok(token) => {
                protocol::verify_id_token(provider, endpoints, &token, audience, browser).await
            }
            Err(err) => Err(err),
        };
        let claims = match claims {
            Ok(claims) => claims,
            Err(err) => {
                tracing::warn!(request = row.id, provider = provider.as_str(), error = %err, "вход через провайдера не прошёл");
                return Ok(Err(protocol_refusal(provider, &err)));
            }
        };
        let name = match provider {
            Provider::Apple => protocol::apple_name(user),
            Provider::Google => AppleName {
                first_name: claims.first_name.clone(),
                last_name: claims.last_name.clone(),
            },
        };
        match row.purpose() {
            Purpose::Login => {
                self.login_with(
                    tx,
                    &claims,
                    &name,
                    row.referral.as_deref(),
                    row.utm.as_deref(),
                )
                .await
            }
            Purpose::Link => self.link_with(tx, row, &claims, &name, now).await,
        }
    }

    async fn login_with(
        &self,
        tx: &mut Tx,
        claims: &IdClaims,
        name: &AppleName,
        referral: Option<&str>,
        utm: Option<&str>,
    ) -> ServiceResult<Result<Done, Refusal>> {
        let provider = claims.provider;
        let kind = provider.identity();
        lock_subject(tx, provider, &claims.sub).await?;
        if let Some(client) = tx.clients().find_by_identity(kind, &claims.sub).await? {
            if client.is_blocked() {
                return Ok(Err(blocked()));
            }
            let previous = tx
                .clients()
                .identities(client.id)
                .await?
                .into_iter()
                .find(|i| i.kind == kind && i.value == claims.sub)
                .map(|i| i.meta);
            tx.clients()
                .update_identity_meta(
                    kind,
                    &claims.sub,
                    &identity_meta(claims, name, previous.as_ref()),
                )
                .await?;
            tx.audit()
                .record(&Actor::Client(client.id).audit(
                    "client.oauth_login",
                    "client",
                    Some(client.id.0),
                    serde_json::json!({ "provider": provider }),
                ))
                .await?;
            return Ok(Ok(Done {
                client_id: client.id,
                created: false,
            }));
        }
        let email = claims
            .verified_email()
            .and_then(|e| normalize_identity(IdentityKind::Email, e).ok());
        let identity = NewIdentity {
            kind,
            value: claims.sub.clone(),
            verified: true,
            meta: identity_meta(claims, name, None),
        };
        if let Some(email) = &email
            && let Some(owner) = tx
                .clients()
                .find_by_identity(IdentityKind::Email, email)
                .await?
        {
            let owner = tx.clients().lock(owner.id).await?;
            if owner.is_blocked() {
                return Ok(Err(blocked()));
            }
            let identities = tx.clients().identities(owner.id).await?;
            if identities.iter().any(|i| i.kind == kind) {
                return Ok(Err(Refusal::new(
                    "oauth.other_account",
                    format!(
                        "К аккаунту с почтой {email} уже привязан другой аккаунт {}. Войдите им или прежним способом.",
                        provider.title()
                    ),
                )));
            }
            let confirmed_here = identities
                .iter()
                .any(|i| i.kind == IdentityKind::Email && i.value == *email && i.verified);
            if !(claims.authoritative() && confirmed_here) {
                return Ok(Err(Refusal::new(
                    "oauth.email_taken",
                    format!(
                        "Почта {email} уже есть в другом аккаунте. Войдите в него прежним способом и привяжите {} в разделе «Безопасность».",
                        provider.title()
                    ),
                )));
            }
            tx.clients().add_identity(owner.id, &identity).await?;
            notify_linked(tx, owner.id, claims).await?;
            tx.audit()
                .record(&Actor::Client(owner.id).audit(
                    "client.oauth_linked",
                    "client",
                    Some(owner.id.0),
                    serde_json::json!({ "provider": provider, "via": "email" }),
                ))
                .await?;
            return Ok(Ok(Done {
                client_id: owner.id,
                created: false,
            }));
        }
        let client = tx
            .clients()
            .create(&NewClient {
                locale: self
                    .services
                    .settings()
                    .languages
                    .pick(sn_domain::lang::hint()),
                registration_source: RegistrationSource::Web,
                note: None,
            })
            .await?;
        tx.clients().add_identity(client.id, &identity).await?;
        if let Some(email) = &email {
            tx.clients()
                .add_identity_if_free(
                    client.id,
                    &NewIdentity::new(IdentityKind::Email, email, claims.authoritative())?,
                )
                .await?;
        }
        tx.events()
            .push(&DomainEvent::ClientRegistered {
                client_id: client.id,
                source: RegistrationSource::Web,
                start_param: with_referral(utm, referral),
            })
            .await?;
        tx.audit()
            .record(&Actor::Client(client.id).audit(
                "client.oauth_registered",
                "client",
                Some(client.id.0),
                serde_json::json!({ "provider": provider }),
            ))
            .await?;
        Ok(Ok(Done {
            client_id: client.id,
            created: true,
        }))
    }

    async fn link_with(
        &self,
        tx: &mut Tx,
        row: &RequestRow,
        claims: &IdClaims,
        name: &AppleName,
        now: DateTime<Utc>,
    ) -> ServiceResult<Result<Done, Refusal>> {
        let provider = claims.provider;
        let kind = provider.identity();
        let (Some(client_id), Some(session_id)) = (row.client_id.map(ClientId), row.session_id)
        else {
            return Ok(Err(Refusal::new(
                "oauth.session_gone",
                "Сессия кабинета закончилась. Войдите заново и повторите привязку.",
            )));
        };
        let alive: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM client_sessions WHERE id = $1 AND client_id = $2 \
             AND channel = 'cabinet' AND revoked_at IS NULL AND expires_at > $3)",
        )
        .bind(session_id)
        .bind(client_id.0)
        .bind(now)
        .fetch_one(tx.conn())
        .await?;
        if !alive {
            return Ok(Err(Refusal::new(
                "oauth.session_gone",
                "Сессия кабинета закончилась. Войдите заново и повторите привязку.",
            )));
        }
        lock_subject(tx, provider, &claims.sub).await?;
        let client = tx.clients().lock(client_id).await?;
        if client.is_blocked() {
            return Ok(Err(blocked()));
        }
        if unconfirmed_only(&tx.clients().identities(client_id).await?) {
            return Ok(Err(Refusal::new(
                EMAIL_UNVERIFIED,
                unconfirmed_text(provider),
            )));
        }
        if let Some(owner) = tx.clients().find_by_identity(kind, &claims.sub).await? {
            if owner.id == client_id {
                return Ok(Ok(Done {
                    client_id,
                    created: false,
                }));
            }
            return Ok(Err(Refusal::new(
                "oauth.taken",
                format!(
                    "Этот аккаунт {} уже используется для входа в другой аккаунт. Сначала отвяжите его там.",
                    provider.title()
                ),
            )));
        }
        if tx
            .clients()
            .identities(client_id)
            .await?
            .iter()
            .any(|i| i.kind == kind)
        {
            return Ok(Err(Refusal::new(
                "oauth.already_linked",
                format!(
                    "К аккаунту уже привязан другой аккаунт {}. Сначала отвяжите его.",
                    provider.title()
                ),
            )));
        }
        tx.clients()
            .add_identity(
                client_id,
                &NewIdentity {
                    kind,
                    value: claims.sub.clone(),
                    verified: true,
                    meta: identity_meta(claims, name, None),
                },
            )
            .await?;
        notify_linked(tx, client_id, claims).await?;
        tx.audit()
            .record(&Actor::Client(client_id).audit(
                "client.oauth_linked",
                "client",
                Some(client_id.0),
                serde_json::json!({ "provider": provider, "via": "security" }),
            ))
            .await?;
        Ok(Ok(Done {
            client_id,
            created: false,
        }))
    }

    pub async fn finish(
        &self,
        browser: Option<&str>,
        meta: &RequestMeta,
    ) -> ServiceResult<OauthFinish> {
        let Some(browser) = browser.map(str::trim).filter(|b| !b.is_empty()) else {
            return Err(no_request());
        };
        self.services
            .limits()
            .hit_ip(rules::OAUTH_FINISH_IP, meta.ip.as_deref())
            .await?;
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: Option<RequestRow> = sqlx::query_as(&format!(
            "SELECT {REQUEST_COLUMNS} FROM oauth_requests WHERE browser_hash = $1 ORDER BY id DESC LIMIT 1 FOR UPDATE"
        ))
        .bind(token_hash(browser))
        .fetch_optional(tx.conn())
        .await?;
        let Some(row) = row else {
            return Err(no_request());
        };
        let provider = row.provider().ok_or_else(no_request)?;
        let late = row
            .finished_at
            .is_some_and(|at| at + Duration::seconds(FINISH_SECONDS) <= now);
        match row.status.as_str() {
            "pending" | "consumed" => return Err(no_request()),
            _ if late => {
                sqlx::query("UPDATE oauth_requests SET status = 'consumed' WHERE id = $1")
                    .bind(row.id)
                    .execute(tx.conn())
                    .await?;
                tx.commit().await?;
                return Err(ServiceError::rejected(
                    "oauth.expired",
                    "Время на вход вышло. Нажмите кнопку ещё раз.",
                ));
            }
            _ => {}
        }
        sqlx::query("UPDATE oauth_requests SET status = 'consumed' WHERE id = $1")
            .bind(row.id)
            .execute(tx.conn())
            .await?;
        if row.status == "failed" {
            tx.commit().await?;
            return Err(ServiceError::rejected(
                stored_code(row.error_code.as_deref()),
                row.error_text
                    .unwrap_or_else(|| "Вход не завершён. Нажмите кнопку входа ещё раз.".into()),
            ));
        }
        let client_id = ClientId(row.result_client_id.ok_or_else(no_request)?);
        let result = match row.purpose() {
            Purpose::Link => OauthFinish::Linked { provider },
            Purpose::Login => {
                let client = tx.clients().get(client_id).await?;
                if client.is_blocked() {
                    tx.commit().await?;
                    let refusal = blocked();
                    return Err(ServiceError::rejected(refusal.code, refusal.message));
                }
                if !self.services.settings().client_access.site_login {
                    tx.commit().await?;
                    return Err(ServiceError::rejected(
                        "auth.site_off",
                        "Вход на сайт выключен: пользуйтесь ботом",
                    ));
                }
                match cabinet_session(&mut tx, client_id, now, meta, row.created_client).await? {
                    CabinetLogin::Session(login) => OauthFinish::Confirmed { login },
                    CabinetLogin::TotpRequired(totp) => OauthFinish::TotpRequired {
                        challenge: totp.challenge,
                        expires_at: totp.expires_at,
                    },
                }
            }
        };
        tx.commit().await?;
        Ok(result)
    }

    fn describe(&self, identities: &[Identity], password: bool) -> LoginMethods {
        let available = self.availability();
        let mail = self.services.mail();
        let email_login = password || mail.code_login() || mail.available();
        let usable = |identity: &Identity| match identity.kind {
            IdentityKind::Telegram => true,
            IdentityKind::Email => email_login,
            IdentityKind::Google => available.google,
            IdentityKind::Apple => available.apple,
        };
        let methods = identities
            .iter()
            .map(|identity| LoginMethod {
                kind: identity.kind,
                account: account_title(identity),
                linked_at: identity.created_at,
                removable: Provider::of(identity.kind).is_some()
                    && identities
                        .iter()
                        .any(|other| other.kind != identity.kind && usable(other)),
            })
            .collect();
        let can_link = Provider::ALL
            .into_iter()
            .filter(|p| available.has(*p) && !identities.iter().any(|i| i.kind == p.identity()))
            .collect();
        LoginMethods { methods, can_link }
    }

    pub async fn logins(&self, client_id: ClientId) -> ServiceResult<LoginMethods> {
        let mut conn = self.services.store().conn().await?;
        let identities = conn.clients().identities(client_id).await?;
        let password = has_password(&mut conn, client_id).await?;
        Ok(self.describe(&identities, password))
    }

    pub async fn unlink(
        &self,
        principal: &ClientPrincipal,
        provider: Provider,
    ) -> ServiceResult<LoginMethods> {
        let client_id = principal.client_id;
        let mut tx = self.services.store().begin().await?;
        tx.clients().lock(client_id).await?;
        let identities = tx.clients().identities(client_id).await?;
        let password = has_password(&mut tx, client_id).await?;
        let view = self.describe(&identities, password);
        let Some(method) = view.methods.iter().find(|m| m.kind == provider.identity()) else {
            return Err(ServiceError::rejected(
                "oauth.not_linked",
                format!("Вход через {} не привязан", provider.title()),
            ));
        };
        if !method.removable {
            return Err(ServiceError::rejected(
                "oauth.last_method",
                "Это единственный способ входа. Сначала привяжите другой: Telegram, почту или второй аккаунт.",
            ));
        }
        sqlx::query("DELETE FROM client_identities WHERE client_id = $1 AND kind = $2")
            .bind(client_id.0)
            .bind(provider.identity().as_str())
            .execute(tx.conn())
            .await?;
        tx.audit()
            .record(&Actor::Client(client_id).audit(
                "client.oauth_unlinked",
                "client",
                Some(client_id.0),
                serde_json::json!({ "provider": provider }),
            ))
            .await?;
        tx.commit().await?;
        self.logins(client_id).await
    }
}
