use crate::error::StoreResult;
use chrono::{DateTime, Utc};
use sn_domain::ids::AdminId;
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentPage {
    pub id: String,
    pub body_ru: String,
    pub body_en: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentPageInfo {
    pub id: String,
    pub english: bool,
    pub updated_at: DateTime<Utc>,
}

pub struct DocumentPagesRepo<'c> {
    conn: &'c mut PgConnection,
}

type PageRow = (String, String, Option<String>, DateTime<Utc>);

impl<'c> DocumentPagesRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        DocumentPagesRepo { conn }
    }

    pub async fn get(&mut self, id: &str) -> StoreResult<Option<DocumentPage>> {
        let row: Option<PageRow> = sqlx::query_as(
            "SELECT id, body_ru, body_en, updated_at FROM document_pages WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(row.map(|(id, body_ru, body_en, updated_at)| DocumentPage {
            id,
            body_ru,
            body_en,
            updated_at,
        }))
    }

    pub async fn infos(&mut self) -> StoreResult<Vec<DocumentPageInfo>> {
        let rows: Vec<(String, bool, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, body_en IS NOT NULL, updated_at FROM document_pages ORDER BY id",
        )
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, english, updated_at)| DocumentPageInfo {
                id,
                english,
                updated_at,
            })
            .collect())
    }

    pub async fn put(
        &mut self,
        id: &str,
        body_ru: &str,
        body_en: Option<&str>,
        by: Option<AdminId>,
    ) -> StoreResult<DocumentPage> {
        let (id, body_ru, body_en, updated_at): PageRow = sqlx::query_as(
            "INSERT INTO document_pages (id, body_ru, body_en, updated_by, updated_at) \
             VALUES ($1, $2, $3, $4, now()) \
             ON CONFLICT (id) DO UPDATE SET body_ru = EXCLUDED.body_ru, body_en = EXCLUDED.body_en, \
             updated_by = CASE WHEN document_pages.body_ru IS DISTINCT FROM EXCLUDED.body_ru \
             OR document_pages.body_en IS DISTINCT FROM EXCLUDED.body_en \
             THEN EXCLUDED.updated_by ELSE document_pages.updated_by END, \
             updated_at = CASE WHEN document_pages.body_ru IS DISTINCT FROM EXCLUDED.body_ru \
             OR document_pages.body_en IS DISTINCT FROM EXCLUDED.body_en \
             THEN now() ELSE document_pages.updated_at END \
             RETURNING id, body_ru, body_en, updated_at",
        )
        .bind(id)
        .bind(body_ru)
        .bind(body_en)
        .bind(by.map(|a| a.0))
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(DocumentPage {
            id,
            body_ru,
            body_en,
            updated_at,
        })
    }

    pub async fn delete(&mut self, id: &str) -> StoreResult<bool> {
        let done = sqlx::query("DELETE FROM document_pages WHERE id = $1")
            .bind(id)
            .execute(&mut *self.conn)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn keep_only(&mut self, ids: &[String]) -> StoreResult<u64> {
        let done = sqlx::query("DELETE FROM document_pages WHERE NOT (id = ANY($1))")
            .bind(ids)
            .execute(&mut *self.conn)
            .await?;
        Ok(done.rows_affected())
    }
}
