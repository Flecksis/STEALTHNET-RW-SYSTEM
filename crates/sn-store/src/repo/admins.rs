use crate::crypto::SealedSecret;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::access::{AdminRole, Permission};
use sn_domain::ids::AdminId;
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRecord {
    pub id: AdminId,
    pub email: String,
    pub password_hash: String,
    pub role: AdminRole,
    pub permissions: Vec<Permission>,
    pub totp: Option<SealedSecret>,
    pub totp_last_step: Option<i64>,
    pub telegram_id: Option<i64>,
    pub must_change_password: bool,
    pub disabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdmin {
    pub email: String,
    pub password_hash: String,
    pub role: AdminRole,
    pub must_change_password: bool,
}

#[derive(sqlx::FromRow)]
struct AdminRow {
    id: i64,
    email: String,
    password_hash: String,
    role: String,
    permissions: Vec<String>,
    totp_secret: Option<Vec<u8>>,
    totp_nonce: Option<Vec<u8>>,
    totp_last_step: Option<i64>,
    telegram_id: Option<i64>,
    must_change_password: bool,
    disabled_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, email, password_hash, role, permissions, totp_secret, totp_nonce, totp_last_step, \
                       telegram_id, must_change_password, disabled_at, created_at";

impl TryFrom<AdminRow> for AdminRecord {
    type Error = StoreError;

    fn try_from(row: AdminRow) -> Result<Self, Self::Error> {
        let permissions = row
            .permissions
            .iter()
            .map(|p| Permission::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        let totp = match (row.totp_nonce, row.totp_secret) {
            (Some(nonce), Some(ciphertext)) => Some(SealedSecret { nonce, ciphertext }),
            _ => None,
        };
        Ok(AdminRecord {
            id: AdminId(row.id),
            email: row.email,
            password_hash: row.password_hash,
            role: AdminRole::parse(&row.role)?,
            permissions,
            totp,
            totp_last_step: row.totp_last_step,
            telegram_id: row.telegram_id,
            must_change_password: row.must_change_password,
            disabled_at: row.disabled_at,
            created_at: row.created_at,
        })
    }
}

pub struct AdminsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> AdminsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        AdminsRepo { conn }
    }

    pub async fn create(&mut self, admin: &NewAdmin) -> StoreResult<AdminRecord> {
        let row: AdminRow = sqlx::query_as(&format!(
            "INSERT INTO admins (email, password_hash, role, must_change_password) VALUES ($1, $2, $3, $4) RETURNING {COLUMNS}"
        ))
        .bind(admin.email.trim())
        .bind(&admin.password_hash)
        .bind(admin.role.as_str())
        .bind(admin.must_change_password)
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    pub async fn get(&mut self, id: AdminId) -> StoreResult<AdminRecord> {
        let row: AdminRow = sqlx::query_as(&format!("SELECT {COLUMNS} FROM admins WHERE id = $1"))
            .bind(id.0)
            .fetch_optional(&mut *self.conn)
            .await?
            .ok_or(StoreError::NotFound("администратор"))?;
        row.try_into()
    }

    pub async fn by_email(&mut self, email: &str) -> StoreResult<Option<AdminRecord>> {
        let row: Option<AdminRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM admins WHERE lower(email) = lower($1)"
        ))
        .bind(email.trim())
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn list(&mut self) -> StoreResult<Vec<AdminRecord>> {
        let rows: Vec<AdminRow> =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM admins ORDER BY id"))
                .fetch_all(&mut *self.conn)
                .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn count_active(&mut self) -> StoreResult<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM admins WHERE disabled_at IS NULL")
                .fetch_one(&mut *self.conn)
                .await?,
        )
    }

    pub async fn set_password(
        &mut self,
        id: AdminId,
        password_hash: &str,
        must_change: bool,
    ) -> StoreResult<()> {
        let done = sqlx::query(
            "UPDATE admins SET password_hash = $2, must_change_password = $3, disabled_at = NULL WHERE id = $1",
        )
        .bind(id.0)
        .bind(password_hash)
        .bind(must_change)
        .execute(&mut *self.conn)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound("администратор"));
        }
        Ok(())
    }

    pub async fn set_role(
        &mut self,
        id: AdminId,
        role: AdminRole,
        permissions: &[Permission],
    ) -> StoreResult<()> {
        let permissions: Vec<&str> = permissions.iter().map(|p| p.as_str()).collect();
        sqlx::query("UPDATE admins SET role = $2, permissions = $3 WHERE id = $1")
            .bind(id.0)
            .bind(role.as_str())
            .bind(&permissions)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn set_totp(&mut self, id: AdminId, totp: Option<&SealedSecret>) -> StoreResult<()> {
        sqlx::query(
            "UPDATE admins SET totp_nonce = $2, totp_secret = $3, totp_last_step = NULL WHERE id = $1",
        )
        .bind(id.0)
        .bind(totp.map(|t| t.nonce.clone()))
        .bind(totp.map(|t| t.ciphertext.clone()))
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn use_totp_step(&mut self, id: AdminId, step: i64) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE admins SET totp_last_step = $2 WHERE id = $1 AND (totp_last_step IS NULL OR totp_last_step < $2)",
        )
        .bind(id.0)
        .bind(step)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn disable(&mut self, id: AdminId) -> StoreResult<()> {
        sqlx::query("UPDATE admins SET disabled_at = now() WHERE id = $1 AND disabled_at IS NULL")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }
}
