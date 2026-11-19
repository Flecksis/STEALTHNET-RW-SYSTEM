use crate::error::StoreResult;
use sn_domain::ids::ClientId;
use sqlx::PgConnection;
use std::time::Duration;

pub const PHOTO_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPhoto {
    pub content: Vec<u8>,
    pub content_type: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoTarget {
    pub client_id: ClientId,
    pub telegram_id: i64,
    pub known: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewPhoto<'a> {
    pub version: &'a str,
    pub content: &'a [u8],
    pub content_type: &'a str,
}

pub struct ClientPhotosRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> ClientPhotosRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        ClientPhotosRepo { conn }
    }

    pub async fn get(&mut self, client_id: ClientId) -> StoreResult<Option<ClientPhoto>> {
        let row: Option<(Vec<u8>, String, String)> = sqlx::query_as(
            "SELECT p.content, p.content_type, p.file_unique_id FROM client_photos p \
             JOIN clients c ON c.id = p.client_id AND c.deleted_at IS NULL \
             WHERE p.client_id = $1 AND p.content IS NOT NULL",
        )
        .bind(client_id.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(row.map(|(content, content_type, version)| ClientPhoto {
            content,
            content_type,
            version,
        }))
    }

    pub async fn versions(&mut self, ids: &[ClientId]) -> StoreResult<Vec<(ClientId, String)>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT client_id, file_unique_id FROM client_photos \
             WHERE client_id = ANY($1) AND content IS NOT NULL",
        )
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, version)| (ClientId(id), version))
            .collect())
    }

    pub async fn request_missing(&mut self, ids: &[ClientId]) -> StoreResult<u64> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let done = sqlx::query(
            "INSERT INTO client_photos (client_id, due_at) \
             SELECT DISTINCT i.client_id, now() FROM client_identities i \
             JOIN clients c ON c.id = i.client_id AND c.deleted_at IS NULL \
             WHERE i.kind = 'telegram' AND i.client_id = ANY($1) \
             ON CONFLICT (client_id) DO NOTHING",
        )
        .bind(&ids)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected())
    }

    pub async fn request_if_stale(
        &mut self,
        client_id: ClientId,
        older_than: Duration,
    ) -> StoreResult<bool> {
        let settled: Option<bool> = sqlx::query_scalar(
            "SELECT due_at IS NOT NULL \
                    OR (checked_at IS NOT NULL AND checked_at >= now() - make_interval(secs => $2)) \
             FROM client_photos WHERE client_id = $1",
        )
        .bind(client_id.0)
        .bind(older_than.as_secs_f64())
        .fetch_optional(&mut *self.conn)
        .await?;
        if settled == Some(true) {
            return Ok(false);
        }
        let done = sqlx::query(
            "INSERT INTO client_photos (client_id, due_at) VALUES ($1, now()) \
             ON CONFLICT (client_id) DO UPDATE SET due_at = now() \
             WHERE client_photos.due_at IS NULL \
               AND (client_photos.checked_at IS NULL \
                    OR client_photos.checked_at < now() - make_interval(secs => $2))",
        )
        .bind(client_id.0)
        .bind(older_than.as_secs_f64())
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn due(&mut self, limit: i64) -> StoreResult<Vec<PhotoTarget>> {
        let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
            "SELECT p.client_id, i.value, p.file_unique_id FROM client_photos p \
             JOIN clients c ON c.id = p.client_id AND c.deleted_at IS NULL \
             JOIN client_identities i ON i.client_id = p.client_id AND i.kind = 'telegram' \
             WHERE p.due_at <= now() ORDER BY p.due_at, p.client_id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(client_id, value, known)| {
                value
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .map(|telegram_id| PhotoTarget {
                        client_id: ClientId(client_id),
                        telegram_id,
                        known,
                    })
            })
            .collect())
    }

    pub async fn save(
        &mut self,
        client_id: ClientId,
        photo: Option<NewPhoto<'_>>,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE client_photos SET file_unique_id = $2, content = $3, content_type = $4, \
             due_at = NULL, checked_at = now(), updated_at = now() WHERE client_id = $1",
        )
        .bind(client_id.0)
        .bind(photo.map(|p| p.version))
        .bind(photo.map(|p| p.content))
        .bind(photo.map(|p| p.content_type))
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn keep(&mut self, client_id: ClientId) -> StoreResult<()> {
        sqlx::query(
            "UPDATE client_photos SET due_at = NULL, checked_at = now() WHERE client_id = $1",
        )
        .bind(client_id.0)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn postpone(&mut self, client_id: ClientId, delay: Duration) -> StoreResult<()> {
        sqlx::query(
            "UPDATE client_photos SET due_at = now() + make_interval(secs => $2) WHERE client_id = $1",
        )
        .bind(client_id.0)
        .bind(delay.as_secs_f64())
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }
}
