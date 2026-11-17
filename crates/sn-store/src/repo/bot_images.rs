use crate::error::StoreResult;
use sn_domain::ids::AdminId;
use sqlx::PgConnection;

pub const BOT_IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotImage {
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

pub struct BotImagesRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> BotImagesRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        BotImagesRepo { conn }
    }

    pub async fn get(&mut self, slot: &str) -> StoreResult<Option<BotImage>> {
        let row: Option<(String, Vec<u8>, String)> =
            sqlx::query_as("SELECT content_type, bytes, sha256 FROM bot_images WHERE slot = $1")
                .bind(slot)
                .fetch_optional(&mut *self.conn)
                .await?;
        Ok(row.map(|(content_type, bytes, sha256)| BotImage {
            content_type,
            bytes,
            sha256,
        }))
    }

    pub async fn versions(&mut self) -> StoreResult<Vec<(String, String)>> {
        Ok(
            sqlx::query_as("SELECT slot, sha256 FROM bot_images ORDER BY slot")
                .fetch_all(&mut *self.conn)
                .await?,
        )
    }

    pub async fn put(
        &mut self,
        slot: &str,
        image: &BotImage,
        by: Option<AdminId>,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO bot_images (slot, content_type, bytes, sha256, updated_by, updated_at) \
             VALUES ($1, $2, $3, $4, $5, now()) \
             ON CONFLICT (slot) DO UPDATE SET content_type = EXCLUDED.content_type, \
             bytes = EXCLUDED.bytes, sha256 = EXCLUDED.sha256, \
             updated_by = EXCLUDED.updated_by, updated_at = now()",
        )
        .bind(slot)
        .bind(&image.content_type)
        .bind(&image.bytes)
        .bind(&image.sha256)
        .bind(by.map(|a| a.0))
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn delete(&mut self, slot: &str) -> StoreResult<bool> {
        let done = sqlx::query("DELETE FROM bot_images WHERE slot = $1")
            .bind(slot)
            .execute(&mut *self.conn)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}
