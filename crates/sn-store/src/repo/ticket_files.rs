use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId, TicketFileId, TicketId};
use sn_domain::support::{FileKind, TicketFile};
use sqlx::PgConnection;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOwner {
    Client(ClientId),
    Admin(AdminId),
}

impl FileOwner {
    const fn author(self) -> &'static str {
        match self {
            FileOwner::Client(_) => "client",
            FileOwner::Admin(_) => "support",
        }
    }

    const fn client(self) -> Option<i64> {
        match self {
            FileOwner::Client(id) => Some(id.0),
            FileOwner::Admin(_) => None,
        }
    }

    const fn admin(self) -> Option<i64> {
        match self {
            FileOwner::Admin(id) => Some(id.0),
            FileOwner::Client(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTicketFile<'a> {
    pub owner: FileOwner,
    pub name: &'a str,
    pub mime: &'a str,
    pub bytes: &'a [u8],
    pub width: Option<i32>,
    pub height: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub file: TicketFile,
    pub ticket_id: Option<TicketId>,
    pub ticket_client: Option<ClientId>,
    pub owner: Option<FileOwner>,
    pub bytes: Option<Vec<u8>>,
}

const META: &str =
    "f.id, f.name, f.mime, f.size, f.width, f.height, f.purged_at IS NOT NULL AS removed";

#[derive(sqlx::FromRow)]
struct MetaRow {
    id: i64,
    name: String,
    mime: String,
    size: i32,
    width: Option<i32>,
    height: Option<i32>,
    removed: bool,
}

impl From<MetaRow> for TicketFile {
    fn from(row: MetaRow) -> Self {
        TicketFile {
            id: TicketFileId(row.id),
            kind: if row.mime.starts_with("image/") {
                FileKind::Image
            } else {
                FileKind::File
            },
            name: row.name,
            mime: row.mime,
            size: i64::from(row.size),
            width: row.width,
            height: row.height,
            removed: row.removed,
        }
    }
}

#[derive(sqlx::FromRow)]
struct MessageMetaRow {
    message_id: i64,
    #[sqlx(flatten)]
    meta: MetaRow,
}

#[derive(sqlx::FromRow)]
struct StoredRow {
    #[sqlx(flatten)]
    meta: MetaRow,
    ticket_id: Option<i64>,
    ticket_client: Option<i64>,
    author: String,
    client_id: Option<i64>,
    admin_id: Option<i64>,
    data: Option<Vec<u8>>,
}

impl From<StoredRow> for StoredFile {
    fn from(row: StoredRow) -> Self {
        let owner = match (row.author.as_str(), row.client_id, row.admin_id) {
            ("client", Some(id), _) => Some(FileOwner::Client(ClientId(id))),
            ("support", _, Some(id)) => Some(FileOwner::Admin(AdminId(id))),
            _ => None,
        };
        StoredFile {
            file: row.meta.into(),
            ticket_id: row.ticket_id.map(TicketId),
            ticket_client: row.ticket_client.map(ClientId),
            owner,
            bytes: row.data,
        }
    }
}

const OWNER: &str =
    "f.author = $2 AND f.client_id IS NOT DISTINCT FROM $3 AND f.admin_id IS NOT DISTINCT FROM $4";

pub(crate) async fn attach(
    conn: &mut PgConnection,
    ticket_id: TicketId,
    message_id: i64,
    files: &[TicketFileId],
) -> StoreResult<()> {
    if files.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = files.iter().map(|f| f.0).collect();
    let attached = sqlx::query(
        "UPDATE ticket_files SET ticket_id = $1, message_id = $2 \
         WHERE id = ANY($3) AND message_id IS NULL AND data IS NOT NULL",
    )
    .bind(ticket_id.0)
    .bind(message_id)
    .bind(&ids)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if attached != ids.len() as u64 {
        return Err(StoreError::NotFound("файл"));
    }
    Ok(())
}

pub(crate) async fn for_ticket(
    conn: &mut PgConnection,
    ticket_id: TicketId,
) -> StoreResult<HashMap<i64, Vec<TicketFile>>> {
    let rows: Vec<MessageMetaRow> = sqlx::query_as(&format!(
        "SELECT f.message_id, {META} FROM ticket_files f \
         WHERE f.ticket_id = $1 AND f.message_id IS NOT NULL ORDER BY f.id"
    ))
    .bind(ticket_id.0)
    .fetch_all(&mut *conn)
    .await?;
    let mut grouped: HashMap<i64, Vec<TicketFile>> = HashMap::new();
    for row in rows {
        grouped
            .entry(row.message_id)
            .or_default()
            .push(row.meta.into());
    }
    Ok(grouped)
}

pub struct TicketFilesRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> TicketFilesRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        TicketFilesRepo { conn }
    }

    pub async fn insert(&mut self, file: &NewTicketFile<'_>) -> StoreResult<TicketFile> {
        let size = i32::try_from(file.bytes.len())
            .map_err(|_| StoreError::Invalid("файл слишком большой".into()))?;
        let row: MetaRow = sqlx::query_as(&format!(
            "INSERT INTO ticket_files AS f (author, client_id, admin_id, name, mime, size, width, height, data) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {META}"
        ))
        .bind(file.owner.author())
        .bind(file.owner.client())
        .bind(file.owner.admin())
        .bind(file.name)
        .bind(file.mime)
        .bind(size)
        .bind(file.width)
        .bind(file.height)
        .bind(file.bytes)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(row.into())
    }

    pub async fn uploaded_today(&mut self, client_id: ClientId) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM ticket_files WHERE client_id = $1 AND author = 'client' \
             AND created_at >= now() - interval '1 day'",
        )
        .bind(client_id.0)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn pending(
        &mut self,
        owner: FileOwner,
        files: &[TicketFileId],
    ) -> StoreResult<Vec<TicketFile>> {
        let ids: Vec<i64> = files.iter().map(|f| f.0).collect();
        let rows: Vec<MetaRow> = sqlx::query_as(&format!(
            "SELECT {META} FROM ticket_files f WHERE f.id = ANY($1) AND {OWNER} \
             AND f.message_id IS NULL AND f.data IS NOT NULL ORDER BY f.id FOR UPDATE"
        ))
        .bind(&ids)
        .bind(owner.author())
        .bind(owner.client())
        .bind(owner.admin())
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn attach(
        &mut self,
        ticket_id: TicketId,
        message_id: i64,
        files: &[TicketFileId],
    ) -> StoreResult<()> {
        attach(&mut *self.conn, ticket_id, message_id, files).await
    }

    pub async fn for_ticket(
        &mut self,
        ticket_id: TicketId,
    ) -> StoreResult<HashMap<i64, Vec<TicketFile>>> {
        for_ticket(&mut *self.conn, ticket_id).await
    }

    pub async fn for_message(&mut self, message_id: i64) -> StoreResult<Vec<TicketFile>> {
        let rows: Vec<MetaRow> = sqlx::query_as(&format!(
            "SELECT {META} FROM ticket_files f WHERE f.message_id = $1 ORDER BY f.id"
        ))
        .bind(message_id)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn stored(
        &mut self,
        condition: &str,
        bind: StoredKey<'_>,
    ) -> StoreResult<Option<StoredFile>> {
        let sql = format!(
            "SELECT {META}, f.ticket_id, t.client_id AS ticket_client, f.author, f.client_id, f.admin_id, f.data \
             FROM ticket_files f LEFT JOIN tickets t ON t.id = f.ticket_id WHERE {condition}"
        );
        let query = sqlx::query_as::<_, StoredRow>(&sql);
        let query = match bind {
            StoredKey::Id(id) => query.bind(id.0),
            StoredKey::Link(token) => query.bind(token),
        };
        Ok(query.fetch_optional(&mut *self.conn).await?.map(Into::into))
    }

    pub async fn content(&mut self, id: TicketFileId) -> StoreResult<Option<StoredFile>> {
        self.stored("f.id = $1", StoredKey::Id(id)).await
    }

    pub async fn by_link(&mut self, token: &str) -> StoreResult<Option<StoredFile>> {
        self.stored(
            "f.link_token = $1 AND f.link_until > now() AND f.message_id IS NOT NULL",
            StoredKey::Link(token),
        )
        .await
    }

    pub async fn issue_link(
        &mut self,
        id: TicketFileId,
        token: &str,
        minutes: i64,
    ) -> StoreResult<DateTime<Utc>> {
        Ok(sqlx::query_scalar(
            "UPDATE ticket_files SET link_token = $2, link_until = now() + make_interval(mins => $3::int) \
             WHERE id = $1 RETURNING link_until",
        )
        .bind(id.0)
        .bind(token)
        .bind(i32::try_from(minutes).unwrap_or(60))
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn discard(&mut self, id: TicketFileId, owner: FileOwner) -> StoreResult<bool> {
        Ok(sqlx::query(&format!(
            "DELETE FROM ticket_files f WHERE f.id = $1 AND {OWNER} AND f.message_id IS NULL"
        ))
        .bind(id.0)
        .bind(owner.author())
        .bind(owner.client())
        .bind(owner.admin())
        .execute(&mut *self.conn)
        .await?
        .rows_affected()
            > 0)
    }

    pub async fn sent_to_telegram(&mut self, id: TicketFileId) -> StoreResult<bool> {
        let at: Option<Option<DateTime<Utc>>> =
            sqlx::query_scalar("SELECT telegram_sent_at FROM ticket_files WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?;
        Ok(at.flatten().is_some())
    }

    pub async fn mark_sent_to_telegram(&mut self, id: TicketFileId) -> StoreResult<()> {
        sqlx::query("UPDATE ticket_files SET telegram_sent_at = now() WHERE id = $1")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn purge_pending(&mut self, hours: i64) -> StoreResult<u64> {
        Ok(sqlx::query(
            "DELETE FROM ticket_files WHERE message_id IS NULL \
             AND created_at < now() - make_interval(hours => $1::int)",
        )
        .bind(i32::try_from(hours).unwrap_or(i32::MAX))
        .execute(&mut *self.conn)
        .await?
        .rows_affected())
    }

    pub async fn purge_closed(&mut self, days: i64) -> StoreResult<u64> {
        Ok(sqlx::query(
            "UPDATE ticket_files f SET data = NULL, purged_at = now(), link_token = NULL, link_until = NULL \
             FROM tickets t WHERE t.id = f.ticket_id AND f.data IS NOT NULL AND t.status = 'closed' \
             AND t.closed_at < now() - make_interval(days => $1::int)",
        )
        .bind(i32::try_from(days).unwrap_or(i32::MAX))
        .execute(&mut *self.conn)
        .await?
        .rows_affected())
    }
}

enum StoredKey<'a> {
    Id(TicketFileId),
    Link(&'a str),
}
