use crate::error::StoreResult;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sn_domain::ids::AdminId;
use sqlx::PgConnection;

pub struct SettingsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> SettingsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        SettingsRepo { conn }
    }

    pub async fn get(&mut self, key: &str) -> StoreResult<Option<Value>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM settings WHERE key = $1")
                .bind(key)
                .fetch_optional(&mut *self.conn)
                .await?,
        )
    }

    pub async fn get_as<T: DeserializeOwned>(&mut self, key: &str) -> StoreResult<Option<T>> {
        match self.get(key).await? {
            Some(value) => Ok(Some(serde_json::from_value(value)?)),
            None => Ok(None),
        }
    }

    pub async fn set(&mut self, key: &str, value: &Value, by: Option<AdminId>) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_by, updated_at) VALUES ($1, $2, $3, now()) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_by = EXCLUDED.updated_by, updated_at = now()",
        )
        .bind(key)
        .bind(value)
        .bind(by.map(|a| a.0))
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn set_as<T: Serialize>(
        &mut self,
        key: &str,
        value: &T,
        by: Option<AdminId>,
    ) -> StoreResult<()> {
        let value = serde_json::to_value(value)?;
        self.set(key, &value, by).await
    }

    pub async fn delete(&mut self, key: &str) -> StoreResult<bool> {
        let done = sqlx::query("DELETE FROM settings WHERE key = $1")
            .bind(key)
            .execute(&mut *self.conn)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn with_prefix(&mut self, prefix: &str) -> StoreResult<Vec<(String, Value)>> {
        Ok(sqlx::query_as(
            "SELECT key, value FROM settings WHERE starts_with(key, $1) ORDER BY key",
        )
        .bind(prefix)
        .fetch_all(&mut *self.conn)
        .await?)
    }
}
