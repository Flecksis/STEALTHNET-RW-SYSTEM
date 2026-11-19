use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::access::{AdminRole, Permission};
use sn_domain::ids::AdminId;
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyRecord {
    pub id: i64,
    pub name: String,
    pub token_prefix: String,
    pub permissions: Vec<Permission>,
    pub created_by: AdminId,
    pub created_by_email: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewApiKey {
    pub name: String,
    pub token_prefix: String,
    pub token_hash: Vec<u8>,
    pub permissions: Vec<Permission>,
    pub created_by: AdminId,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyOwner {
    pub key_id: i64,
    pub permissions: Vec<Permission>,
    pub admin_id: AdminId,
    pub role: AdminRole,
    pub admin_permissions: Vec<Permission>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    id: i64,
    name: String,
    token_prefix: String,
    permissions: Vec<String>,
    created_by: i64,
    created_by_email: String,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct OwnerRow {
    key_id: i64,
    key_permissions: Vec<String>,
    admin_id: i64,
    role: String,
    admin_permissions: Vec<String>,
    last_used_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "k.id, k.name, k.token_prefix, k.permissions, k.created_by, a.email AS created_by_email, \
                       k.created_at, k.expires_at, k.last_used_at, k.revoked_at";

fn permissions(raw: &[String]) -> StoreResult<Vec<Permission>> {
    Ok(raw
        .iter()
        .map(|p| Permission::parse(p))
        .collect::<Result<Vec<_>, _>>()?)
}

impl TryFrom<ApiKeyRow> for ApiKeyRecord {
    type Error = StoreError;

    fn try_from(row: ApiKeyRow) -> Result<Self, Self::Error> {
        Ok(ApiKeyRecord {
            permissions: permissions(&row.permissions)?,
            id: row.id,
            name: row.name,
            token_prefix: row.token_prefix,
            created_by: AdminId(row.created_by),
            created_by_email: row.created_by_email,
            created_at: row.created_at,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
            revoked_at: row.revoked_at,
        })
    }
}

pub struct ApiKeysRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> ApiKeysRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        ApiKeysRepo { conn }
    }

    pub async fn create(&mut self, key: &NewApiKey) -> StoreResult<ApiKeyRecord> {
        let permissions: Vec<&str> = key.permissions.iter().map(|p| p.as_str()).collect();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO api_keys (name, token_prefix, token_hash, permissions, created_by, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(&key.name)
        .bind(&key.token_prefix)
        .bind(&key.token_hash)
        .bind(&permissions)
        .bind(key.created_by.0)
        .bind(key.expires_at)
        .fetch_one(&mut *self.conn)
        .await?;
        self.get(id).await
    }

    pub async fn get(&mut self, id: i64) -> StoreResult<ApiKeyRecord> {
        let row: Option<ApiKeyRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM api_keys k JOIN admins a ON a.id = k.created_by WHERE k.id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.ok_or(StoreError::NotFound("ключ API"))?.try_into()
    }

    pub async fn list(&mut self) -> StoreResult<Vec<ApiKeyRecord>> {
        let rows: Vec<ApiKeyRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM api_keys k JOIN admins a ON a.id = k.created_by \
             ORDER BY k.revoked_at IS NOT NULL, k.id DESC"
        ))
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn owner_by_token(
        &mut self,
        token_hash: &[u8],
        now: DateTime<Utc>,
    ) -> StoreResult<Option<ApiKeyOwner>> {
        let row: Option<OwnerRow> = sqlx::query_as(
            "SELECT k.id AS key_id, k.permissions AS key_permissions, a.id AS admin_id, a.role, \
             a.permissions AS admin_permissions, k.last_used_at \
             FROM api_keys k JOIN admins a ON a.id = k.created_by \
             WHERE k.token_hash = $1 AND k.revoked_at IS NULL \
             AND (k.expires_at IS NULL OR k.expires_at > $2) AND a.disabled_at IS NULL \
             AND NOT EXISTS (SELECT 1 FROM ext_modules m WHERE m.api_key_id = k.id AND NOT m.enabled)",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&mut *self.conn)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(ApiKeyOwner {
            key_id: row.key_id,
            permissions: permissions(&row.key_permissions)?,
            admin_id: AdminId(row.admin_id),
            role: AdminRole::parse(&row.role)?,
            admin_permissions: permissions(&row.admin_permissions)?,
            last_used_at: row.last_used_at,
        }))
    }

    pub async fn touch(&mut self, id: i64) -> StoreResult<()> {
        sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn set_permissions(
        &mut self,
        id: i64,
        permissions: &[Permission],
    ) -> StoreResult<bool> {
        let permissions: Vec<&str> = permissions.iter().map(|p| p.as_str()).collect();
        let done = sqlx::query(
            "UPDATE api_keys SET permissions = $2 WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .bind(&permissions)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn revoke_personal(&mut self, admin: AdminId) -> StoreResult<u64> {
        let done = sqlx::query(
            "UPDATE api_keys k SET revoked_at = now() WHERE k.created_by = $1 AND k.revoked_at IS NULL \
             AND NOT EXISTS (SELECT 1 FROM ext_modules m WHERE m.api_key_id = k.id)",
        )
        .bind(admin.0)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected())
    }

    pub async fn revoke(&mut self, id: i64) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE api_keys SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }
}
