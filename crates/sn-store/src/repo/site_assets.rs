use crate::error::StoreResult;
use sn_domain::ids::AdminId;
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteAsset {
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

pub struct SiteAssetsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> SiteAssetsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        SiteAssetsRepo { conn }
    }

    pub async fn get(&mut self, name: &str) -> StoreResult<Option<SiteAsset>> {
        let row: Option<(String, Vec<u8>, String)> =
            sqlx::query_as("SELECT content_type, bytes, sha256 FROM site_assets WHERE name = $1")
                .bind(name)
                .fetch_optional(&mut *self.conn)
                .await?;
        Ok(row.map(|(content_type, bytes, sha256)| SiteAsset {
            content_type,
            bytes,
            sha256,
        }))
    }

    pub async fn put(
        &mut self,
        name: &str,
        asset: &SiteAsset,
        by: Option<AdminId>,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO site_assets (name, content_type, bytes, sha256, updated_by, updated_at) \
             VALUES ($1, $2, $3, $4, $5, now()) \
             ON CONFLICT (name) DO UPDATE SET content_type = EXCLUDED.content_type, \
             bytes = EXCLUDED.bytes, sha256 = EXCLUDED.sha256, \
             updated_by = EXCLUDED.updated_by, updated_at = now()",
        )
        .bind(name)
        .bind(&asset.content_type)
        .bind(&asset.bytes)
        .bind(&asset.sha256)
        .bind(by.map(|a| a.0))
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn delete(&mut self, name: &str) -> StoreResult<bool> {
        let done = sqlx::query("DELETE FROM site_assets WHERE name = $1")
            .bind(name)
            .execute(&mut *self.conn)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}
