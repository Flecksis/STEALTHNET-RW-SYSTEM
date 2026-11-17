use super::clients::display_name;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId, SubscriptionId, TicketId};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::support::{
    MessageAuthor, Ticket, TicketChannel, TicketFilter, TicketMessage, TicketStatus,
};
use sqlx::PgConnection;
use sqlx::types::Json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTicket {
    pub client_id: ClientId,
    pub subscription_id: Option<SubscriptionId>,
    pub channel: TicketChannel,
    pub subject: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReply {
    pub ticket_id: TicketId,
    pub author: MessageAuthor,
    pub admin_id: Option<AdminId>,
    pub body: String,
    pub attachments: Vec<String>,
    pub close: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TicketListItem {
    pub ticket: Ticket,
    pub client_name: Option<String>,
    pub telegram_username: Option<String>,
    pub email: Option<String>,
    pub waiting_since: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct TicketCounts {
    pub waiting: i64,
    pub answered: i64,
    pub closed: i64,
}

impl TicketCounts {
    pub const fn get(&self, filter: TicketFilter) -> i64 {
        match filter {
            TicketFilter::Waiting => self.waiting,
            TicketFilter::Answered => self.answered,
            TicketFilter::Closed => self.closed,
        }
    }
}

const fn author_str(author: MessageAuthor) -> &'static str {
    match author {
        MessageAuthor::Client => "client",
        MessageAuthor::Support => "support",
        MessageAuthor::Ai => "ai",
        MessageAuthor::System => "system",
    }
}

fn parse_author(value: &str) -> StoreResult<MessageAuthor> {
    match value {
        "client" => Ok(MessageAuthor::Client),
        "support" => Ok(MessageAuthor::Support),
        "ai" => Ok(MessageAuthor::Ai),
        "system" => Ok(MessageAuthor::System),
        other => Err(StoreError::Decode(format!("автор сообщения: {other}"))),
    }
}

fn parse_status(value: &str) -> StoreResult<TicketStatus> {
    match value {
        "open" => Ok(TicketStatus::Open),
        "closed" => Ok(TicketStatus::Closed),
        other => Err(StoreError::Decode(format!("статус обращения: {other}"))),
    }
}

#[derive(sqlx::FromRow)]
struct TicketRow {
    id: i64,
    client_id: i64,
    status: String,
    channel: String,
    subject: Option<String>,
    last_author: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    closed_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "t.id, t.client_id, t.status, t.channel, t.subject, t.last_author, t.created_at, \
                       t.updated_at, t.closed_at";

const PREVIEW_CHARS: i32 = 200;

const fn filter_order(
    filter: TicketFilter,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match filter {
        TicketFilter::Waiting => (
            "t.status = 'open' AND t.last_author = 'client'",
            "COALESCE(t.waiting_message_id, t.last_message_id, 0)",
            "ASC",
            ">",
        ),
        TicketFilter::Answered => (
            "t.status = 'open' AND t.last_author <> 'client'",
            "COALESCE(t.last_message_id, 0)",
            "DESC",
            "<",
        ),
        TicketFilter::Closed => (
            "t.status = 'closed'",
            "COALESCE(t.last_message_id, 0)",
            "DESC",
            "<",
        ),
    }
}

impl TryFrom<TicketRow> for Ticket {
    type Error = StoreError;

    fn try_from(row: TicketRow) -> Result<Self, Self::Error> {
        Ok(Ticket {
            id: TicketId(row.id),
            client_id: ClientId(row.client_id),
            status: parse_status(&row.status)?,
            channel: TicketChannel::parse(&row.channel)?,
            subject: row.subject,
            last_author: parse_author(&row.last_author)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
            closed_at: row.closed_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct KeyedTicketRow {
    #[sqlx(flatten)]
    ticket: TicketRow,
    sort_key: i64,
}

#[derive(sqlx::FromRow)]
struct ViewRow {
    #[sqlx(flatten)]
    ticket: TicketRow,
    sort_key: i64,
    first_name: Option<String>,
    last_name: Option<String>,
    telegram_username: Option<String>,
    email: Option<String>,
    waiting_since: Option<DateTime<Utc>>,
    last_message_at: Option<DateTime<Utc>>,
    preview: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    ticket_id: i64,
    author: String,
    admin_id: Option<i64>,
    body: String,
    attachments: Json<Vec<String>>,
    created_at: DateTime<Utc>,
}

impl TryFrom<MessageRow> for TicketMessage {
    type Error = StoreError;

    fn try_from(row: MessageRow) -> Result<Self, Self::Error> {
        Ok(TicketMessage {
            ticket_id: TicketId(row.ticket_id),
            author: parse_author(&row.author)?,
            admin_id: row.admin_id.map(AdminId),
            body: row.body,
            attachments: row.attachments.0,
            created_at: row.created_at,
        })
    }
}

fn keyed_page(rows: Vec<KeyedTicketRow>, page: PageRequest) -> StoreResult<Page<Ticket>> {
    let keyed = rows
        .into_iter()
        .map(|row| Ok((row.sort_key, Ticket::try_from(row.ticket)?)))
        .collect::<StoreResult<Vec<(i64, Ticket)>>>()?;
    let page = Page::from_overfetch(keyed, page, |(key, _)| *key);
    Ok(Page {
        items: page.items.into_iter().map(|(_, ticket)| ticket).collect(),
        next: page.next,
    })
}

pub struct TicketsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> TicketsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        TicketsRepo { conn }
    }

    pub async fn open(&mut self, ticket: &NewTicket) -> StoreResult<TicketId> {
        let mut tx = sqlx::Connection::begin(&mut *self.conn).await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO tickets (client_id, subscription_id, channel, subject, last_author) \
             VALUES ($1, $2, $3, $4, 'client') RETURNING id",
        )
        .bind(ticket.client_id.0)
        .bind(ticket.subscription_id.map(|s| s.0))
        .bind(ticket.channel.as_str())
        .bind(&ticket.subject)
        .fetch_one(&mut *tx)
        .await?;
        let message: i64 = sqlx::query_scalar(
            "INSERT INTO ticket_messages (ticket_id, author, body) VALUES ($1, 'client', $2) RETURNING id",
        )
        .bind(id)
        .bind(&ticket.body)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE tickets SET last_message_id = $2, waiting_message_id = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(message)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(TicketId(id))
    }

    pub async fn count_open(&mut self, client_id: ClientId) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM tickets WHERE client_id = $1 AND status = 'open'",
        )
        .bind(client_id.0)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn close(&mut self, id: TicketId) -> StoreResult<()> {
        sqlx::query("UPDATE tickets SET status = 'closed', closed_at = now() WHERE id = $1 AND status = 'open'")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn reopen(&mut self, id: TicketId) -> StoreResult<()> {
        sqlx::query(
            "UPDATE tickets SET status = 'open', closed_at = NULL WHERE id = $1 AND status = 'closed'",
        )
        .bind(id.0)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn get(&mut self, id: TicketId) -> StoreResult<Ticket> {
        let row: TicketRow =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM tickets t WHERE t.id = $1"))
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?
                .ok_or(StoreError::NotFound("обращение"))?;
        row.try_into()
    }

    pub async fn list(
        &mut self,
        filter: TicketFilter,
        page: PageRequest,
    ) -> StoreResult<Page<Ticket>> {
        let (condition, key, direction, compare) = filter_order(filter);
        let rows: Vec<KeyedTicketRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, {key} AS sort_key FROM tickets t \
             WHERE {condition} AND ($1::bigint IS NULL OR {key} {compare} $1) \
             ORDER BY {key} {direction}, t.id {direction} LIMIT $2"
        ))
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        keyed_page(rows, page)
    }

    pub async fn list_view(
        &mut self,
        filter: TicketFilter,
        page: PageRequest,
    ) -> StoreResult<Page<TicketListItem>> {
        let (condition, key, direction, compare) = filter_order(filter);
        let rows: Vec<ViewRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, {key} AS sort_key, tg.meta ->> 'first_name' AS first_name, \
             tg.meta ->> 'last_name' AS last_name, tg.meta ->> 'username' AS telegram_username, \
             em.value AS email, \
             CASE WHEN t.status = 'open' AND t.last_author = 'client' THEN wm.created_at END AS waiting_since, \
             lm.created_at AS last_message_at, left(lm.body, {PREVIEW_CHARS}) AS preview \
             FROM tickets t \
             LEFT JOIN ticket_messages wm ON wm.id = t.waiting_message_id \
             LEFT JOIN ticket_messages lm ON lm.id = t.last_message_id \
             LEFT JOIN LATERAL (SELECT i.meta FROM client_identities i WHERE i.client_id = t.client_id \
                 AND i.kind = 'telegram' ORDER BY i.id LIMIT 1) tg ON true \
             LEFT JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = t.client_id \
                 AND i.kind = 'email' ORDER BY i.id LIMIT 1) em ON true \
             WHERE {condition} AND ($1::bigint IS NULL OR {key} {compare} $1) \
             ORDER BY {key} {direction}, t.id {direction} LIMIT $2"
        ))
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        let keyed = rows
            .into_iter()
            .map(|row| {
                Ok((
                    row.sort_key,
                    TicketListItem {
                        ticket: Ticket::try_from(row.ticket)?,
                        client_name: display_name(
                            row.first_name.as_deref(),
                            row.last_name.as_deref(),
                        ),
                        telegram_username: row.telegram_username,
                        email: row.email,
                        waiting_since: row.waiting_since,
                        last_message_at: row.last_message_at,
                        preview: row.preview,
                    },
                ))
            })
            .collect::<StoreResult<Vec<(i64, TicketListItem)>>>()?;
        let page = Page::from_overfetch(keyed, page, |(key, _)| *key);
        Ok(Page {
            items: page.items.into_iter().map(|(_, item)| item).collect(),
            next: page.next,
        })
    }

    pub async fn for_client(
        &mut self,
        client_id: ClientId,
        page: PageRequest,
    ) -> StoreResult<Page<Ticket>> {
        let rows: Vec<KeyedTicketRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, t.id AS sort_key FROM tickets t \
             WHERE t.client_id = $1 AND ($2::bigint IS NULL OR t.id < $2) ORDER BY t.id DESC LIMIT $3"
        ))
        .bind(client_id.0)
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        keyed_page(rows, page)
    }

    pub async fn counts(&mut self) -> StoreResult<TicketCounts> {
        let (waiting, answered, closed): (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE status = 'open' AND last_author = 'client'), \
             count(*) FILTER (WHERE status = 'open' AND last_author <> 'client'), \
             count(*) FILTER (WHERE status = 'closed') FROM tickets",
        )
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(TicketCounts {
            waiting,
            answered,
            closed,
        })
    }

    pub async fn messages(&mut self, id: TicketId) -> StoreResult<Vec<TicketMessage>> {
        let rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT ticket_id, author, admin_id, body, attachments, created_at FROM ticket_messages \
             WHERE ticket_id = $1 ORDER BY id",
        )
        .bind(id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        if rows.is_empty() {
            self.get(id).await?;
        }
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn reply(&mut self, reply: &NewReply) -> StoreResult<()> {
        let mut tx = sqlx::Connection::begin(&mut *self.conn).await?;
        let found: Option<i64> =
            sqlx::query_scalar("SELECT id FROM tickets WHERE id = $1 FOR UPDATE")
                .bind(reply.ticket_id.0)
                .fetch_optional(&mut *tx)
                .await?;
        if found.is_none() {
            return Err(StoreError::NotFound("обращение"));
        }
        let message: i64 = sqlx::query_scalar(
            "INSERT INTO ticket_messages (ticket_id, author, admin_id, body, attachments) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(reply.ticket_id.0)
        .bind(author_str(reply.author))
        .bind(reply.admin_id.map(|a| a.0))
        .bind(&reply.body)
        .bind(Json(&reply.attachments))
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE tickets SET last_author = $2, last_message_id = $3, \
             waiting_message_id = CASE WHEN $2 <> 'client' THEN NULL \
                 WHEN status = 'open' AND last_author = 'client' THEN COALESCE(waiting_message_id, $3) \
                 ELSE $3 END, \
             status = CASE WHEN $4 THEN 'closed' WHEN $2 = 'client' THEN 'open' ELSE status END, \
             closed_at = CASE WHEN $4 THEN COALESCE(closed_at, now()) WHEN $2 = 'client' THEN NULL ELSE closed_at END \
             WHERE id = $1",
        )
        .bind(reply.ticket_id.0)
        .bind(author_str(reply.author))
        .bind(message)
        .bind(reply.close)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
