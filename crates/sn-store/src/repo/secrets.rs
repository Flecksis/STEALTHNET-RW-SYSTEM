use crate::crypto::SealedSecret;
use crate::error::StoreResult;
use sn_domain::ids::AdminId;
use sqlx::PgConnection;

pub struct SecretsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> SecretsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        SecretsRepo { conn }
    }

    pub async fn get(&mut self, key: &str) -> StoreResult<Option<SealedSecret>> {
        let row: Option<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT nonce, ciphertext FROM secrets WHERE key = $1")
                .bind(key)
                .fetch_optional(&mut *self.conn)
                .await?;
        Ok(row.map(|(nonce, ciphertext)| SealedSecret { nonce, ciphertext }))
    }

    pub async fn set(
        &mut self,
        key: &str,
        sealed: &SealedSecret,
        by: Option<AdminId>,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO secrets (key, nonce, ciphertext, updated_by, updated_at) VALUES ($1, $2, $3, $4, now()) \
             ON CONFLICT (key) DO UPDATE SET nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext, \
             updated_by = EXCLUDED.updated_by, updated_at = now()",
        )
        .bind(key)
        .bind(&sealed.nonce)
        .bind(&sealed.ciphertext)
        .bind(by.map(|a| a.0))
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn delete(&mut self, key: &str) -> StoreResult<bool> {
        let done = sqlx::query("DELETE FROM secrets WHERE key = $1")
            .bind(key)
            .execute(&mut *self.conn)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn keys(&mut self, prefix: &str) -> StoreResult<Vec<String>> {
        Ok(
            sqlx::query_scalar("SELECT key FROM secrets WHERE starts_with(key, $1) ORDER BY key")
                .bind(prefix)
                .fetch_all(&mut *self.conn)
                .await?,
        )
    }
}
