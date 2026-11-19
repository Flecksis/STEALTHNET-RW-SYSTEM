use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId};
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    pub token_hash: Vec<u8>,
    pub expires_at: DateTime<Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminLoginMethod {
    Password,
    Totp,
}

impl AdminLoginMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            AdminLoginMethod::Password => "password",
            AdminLoginMethod::Totp => "totp",
        }
    }

    pub fn parse(raw: &str) -> Option<AdminLoginMethod> {
        match raw {
            "password" => Some(AdminLoginMethod::Password),
            "totp" => Some(AdminLoginMethod::Totp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionChannel {
    Cabinet,
    Miniapp,
}

impl SessionChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            SessionChannel::Cabinet => "cabinet",
            SessionChannel::Miniapp => "miniapp",
        }
    }

    fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "cabinet" => Ok(SessionChannel::Cabinet),
            "miniapp" => Ok(SessionChannel::Miniapp),
            other => Err(StoreError::Decode(format!("канал сессии: {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScope {
    Full,
    SubscriptionView,
    PasswordChange,
}

impl SessionScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            SessionScope::Full => "full",
            SessionScope::SubscriptionView => "subscription_view",
            SessionScope::PasswordChange => "password_change",
        }
    }

    fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "full" => Ok(SessionScope::Full),
            "subscription_view" => Ok(SessionScope::SubscriptionView),
            "password_change" => Ok(SessionScope::PasswordChange),
            other => Err(StoreError::Decode(format!("права сессии: {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminSessionRecord {
    pub id: i64,
    pub admin_id: AdminId,
    pub expires_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSessionRecord {
    pub id: i64,
    pub client_id: ClientId,
    pub channel: SessionChannel,
    pub scope: SessionScope,
    pub expires_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ClientSessionRow {
    id: i64,
    client_id: i64,
    channel: String,
    scope: String,
    expires_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

pub struct SessionsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> SessionsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        SessionsRepo { conn }
    }

    pub async fn create_admin(
        &mut self,
        admin_id: AdminId,
        session: &NewSession,
        method: Option<AdminLoginMethod>,
    ) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO admin_sessions (admin_id, token_hash, expires_at, ip, user_agent, method) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(admin_id.0)
        .bind(&session.token_hash)
        .bind(session.expires_at)
        .bind(&session.ip)
        .bind(&session.user_agent)
        .bind(method.map(AdminLoginMethod::as_str))
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn latest_admin_method(
        &mut self,
        admin_id: AdminId,
    ) -> StoreResult<Option<AdminLoginMethod>> {
        let method: Option<Option<String>> = sqlx::query_scalar(
            "SELECT method FROM admin_sessions WHERE admin_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(admin_id.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(method
            .flatten()
            .as_deref()
            .and_then(AdminLoginMethod::parse))
    }

    pub async fn admin_by_token(
        &mut self,
        token_hash: &[u8],
    ) -> StoreResult<Option<AdminSessionRecord>> {
        let row: Option<(i64, i64, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT s.id, s.admin_id, s.expires_at, s.last_seen_at FROM admin_sessions s \
             JOIN admins a ON a.id = s.admin_id \
             WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > now() AND a.disabled_at IS NULL",
        )
        .bind(token_hash)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(row.map(
            |(id, admin_id, expires_at, last_seen_at)| AdminSessionRecord {
                id,
                admin_id: AdminId(admin_id),
                expires_at,
                last_seen_at,
            },
        ))
    }

    pub async fn touch_admin(&mut self, session_id: i64) -> StoreResult<()> {
        sqlx::query("UPDATE admin_sessions SET last_seen_at = now() WHERE id = $1")
            .bind(session_id)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn revoke_admin(&mut self, token_hash: &[u8]) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE admin_sessions SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(token_hash)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn revoke_admin_others(&mut self, admin_id: AdminId, keep: i64) -> StoreResult<u64> {
        let done = sqlx::query(
            "UPDATE admin_sessions SET revoked_at = now() WHERE admin_id = $1 AND revoked_at IS NULL AND id <> $2",
        )
        .bind(admin_id.0)
        .bind(keep)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected())
    }

    pub async fn revoke_all_admin(&mut self, admin_id: AdminId) -> StoreResult<u64> {
        let done = sqlx::query(
            "UPDATE admin_sessions SET revoked_at = now() WHERE admin_id = $1 AND revoked_at IS NULL",
        )
        .bind(admin_id.0)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected())
    }

    pub async fn create_client(
        &mut self,
        client_id: ClientId,
        channel: SessionChannel,
        scope: SessionScope,
        session: &NewSession,
    ) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO client_sessions (client_id, token_hash, channel, scope, expires_at, ip, user_agent) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
        )
        .bind(client_id.0)
        .bind(&session.token_hash)
        .bind(channel.as_str())
        .bind(scope.as_str())
        .bind(session.expires_at)
        .bind(&session.ip)
        .bind(&session.user_agent)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn client_by_token(
        &mut self,
        token_hash: &[u8],
    ) -> StoreResult<Option<ClientSessionRecord>> {
        let row: Option<ClientSessionRow> = sqlx::query_as(
            "SELECT s.id, s.client_id, s.channel, s.scope, s.expires_at, s.last_seen_at FROM client_sessions s \
             JOIN clients c ON c.id = s.client_id \
             WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > now() AND c.deleted_at IS NULL",
        )
        .bind(token_hash)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(|row| {
            Ok(ClientSessionRecord {
                id: row.id,
                client_id: ClientId(row.client_id),
                channel: SessionChannel::parse(&row.channel)?,
                scope: SessionScope::parse(&row.scope)?,
                expires_at: row.expires_at,
                last_seen_at: row.last_seen_at,
            })
        })
        .transpose()
    }

    pub async fn revoke_client(&mut self, token_hash: &[u8]) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE client_sessions SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(token_hash)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn revoke_client_channel(
        &mut self,
        client_id: ClientId,
        channel: SessionChannel,
    ) -> StoreResult<u64> {
        let done = sqlx::query(
            "UPDATE client_sessions SET revoked_at = now() WHERE client_id = $1 AND channel = $2 AND revoked_at IS NULL",
        )
        .bind(client_id.0)
        .bind(channel.as_str())
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected())
    }

    pub async fn revoke_client_others(
        &mut self,
        client_id: ClientId,
        keep: i64,
    ) -> StoreResult<u64> {
        let done = sqlx::query(
            "UPDATE client_sessions SET revoked_at = now() WHERE client_id = $1 AND revoked_at IS NULL AND id <> $2",
        )
        .bind(client_id.0)
        .bind(keep)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected())
    }

    pub async fn revoke_all_client(&mut self, client_id: ClientId) -> StoreResult<u64> {
        let done = sqlx::query(
            "UPDATE client_sessions SET revoked_at = now() WHERE client_id = $1 AND revoked_at IS NULL",
        )
        .bind(client_id.0)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected())
    }
}
