use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{ClientId, SubscriptionId, TicketId};
use sn_domain::jobs::JobPayload;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::support::{
    MAX_OPEN_TICKETS, MessageAuthor, ReplyTarget, Ticket, TicketChannel, TicketFilter,
    TicketMessage, TicketStatus,
};
use sn_store::{Db, NewReply, NewTicket};

pub const MAX_MESSAGE_CHARS: usize = 4_000;
pub const MAX_SUBJECT_CHARS: usize = 200;
pub const REPLY_TEMPLATE: &str = "support_reply";
pub const CLIENT_TICKETS_LIMIT: i64 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTicket {
    pub subscription_id: Option<SubscriptionId>,
    pub channel: TicketChannel,
    pub subject: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyRequest {
    pub body: String,
    #[serde(default)]
    pub close: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TicketThread {
    pub ticket: Ticket,
    pub messages: Vec<TicketMessage>,
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

fn variant<T: serde::de::DeserializeOwned>(value: &str) -> ServiceResult<T> {
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .map_err(|e| ServiceError::Internal(format!("значение «{value}» не разобрано: {e}")))
}

impl TryFrom<TicketRow> for Ticket {
    type Error = ServiceError;

    fn try_from(row: TicketRow) -> Result<Self, Self::Error> {
        Ok(Ticket {
            id: TicketId(row.id),
            client_id: ClientId(row.client_id),
            status: variant::<TicketStatus>(&row.status)?,
            channel: TicketChannel::parse(&row.channel)?,
            subject: row.subject,
            last_author: variant::<MessageAuthor>(&row.last_author)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
            closed_at: row.closed_at,
        })
    }
}

fn message_body(body: &str) -> ServiceResult<String> {
    let body = body.trim();
    if body.is_empty() || body.chars().count() > MAX_MESSAGE_CHARS {
        return Err(ServiceError::rejected(
            "support.body",
            format!("сообщение пустое или длиннее {MAX_MESSAGE_CHARS} символов"),
        ));
    }
    Ok(body.to_string())
}

pub struct SupportOps<'a> {
    services: &'a Services,
}

impl<'a> SupportOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        SupportOps { services }
    }

    pub async fn open(&self, client_id: ClientId, request: &OpenTicket) -> ServiceResult<TicketId> {
        let body = message_body(&request.body)?;
        let subject = request
            .subject
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(MAX_SUBJECT_CHARS).collect::<String>());
        let mut tx = self.services.store().begin().await?;
        if let Some(subscription_id) = request.subscription_id
            && tx.subscriptions().get(subscription_id).await?.client_id != client_id
        {
            return Err(ServiceError::NotFound("подписка"));
        }
        if tx.tickets().count_open(client_id).await? >= MAX_OPEN_TICKETS as i64 {
            return Err(ServiceError::rejected(
                "support.too_many",
                "открыто слишком много обращений, дождитесь ответа",
            ));
        }
        let id = tx
            .tickets()
            .open(&NewTicket {
                client_id,
                subscription_id: request.subscription_id,
                channel: request.channel,
                subject,
                body,
            })
            .await?;
        tx.events()
            .push(&DomainEvent::TicketOpened {
                ticket_id: id,
                client_id,
            })
            .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn list(
        &self,
        filter: TicketFilter,
        page: PageRequest,
    ) -> ServiceResult<Page<Ticket>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.tickets().list(filter, page).await?)
    }

    pub async fn list_view(
        &self,
        filter: TicketFilter,
        page: PageRequest,
    ) -> ServiceResult<(Page<sn_store::TicketListItem>, sn_store::TicketCounts)> {
        let mut conn = self.services.store().conn().await?;
        let items = conn.tickets().list_view(filter, page).await?;
        let counts = conn.tickets().counts().await?;
        Ok((items, counts))
    }

    pub async fn for_client(&self, client_id: ClientId) -> ServiceResult<Vec<Ticket>> {
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<TicketRow> = sqlx::query_as(
            "SELECT id, client_id, status, channel, subject, last_author, created_at, updated_at, closed_at \
             FROM tickets WHERE client_id = $1 ORDER BY status = 'closed', updated_at DESC, id DESC LIMIT $2",
        )
        .bind(client_id.0)
        .bind(CLIENT_TICKETS_LIMIT)
        .fetch_all(conn.conn())
        .await?;
        rows.into_iter().map(Ticket::try_from).collect()
    }

    pub async fn waiting_count(&self) -> ServiceResult<i64> {
        let mut conn = self.services.store().conn().await?;
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM tickets WHERE status = 'open' AND last_author = 'client'",
        )
        .fetch_one(conn.conn())
        .await?)
    }

    async fn ticket_for(
        &self,
        db: &mut impl Db,
        id: TicketId,
        actor: Actor,
    ) -> ServiceResult<Ticket> {
        let ticket = db.tickets().get(id).await?;
        if let Actor::Client(client_id) = actor
            && ticket.client_id != client_id
        {
            return Err(ServiceError::NotFound("обращение"));
        }
        Ok(ticket)
    }

    pub async fn thread(&self, id: TicketId, actor: Actor) -> ServiceResult<TicketThread> {
        let mut conn = self.services.store().conn().await?;
        let ticket = self.ticket_for(&mut conn, id, actor).await?;
        let messages = conn.tickets().messages(id).await?;
        Ok(TicketThread { ticket, messages })
    }

    pub async fn reply(
        &self,
        id: TicketId,
        request: &ReplyRequest,
        actor: Actor,
    ) -> ServiceResult<()> {
        let body = message_body(&request.body)?;
        let mut tx = self.services.store().begin().await?;
        let ticket = self.ticket_for(&mut tx, id, actor).await?;
        let (author, admin_id, close) = match actor {
            Actor::Client(_) => {
                if ticket.status == TicketStatus::Closed {
                    return Err(ServiceError::rejected(
                        "support.closed",
                        "обращение закрыто, создайте новое",
                    ));
                }
                (MessageAuthor::Client, None, false)
            }
            Actor::Admin(admin) => (MessageAuthor::Support, Some(admin), request.close),
            Actor::System => (MessageAuthor::System, None, request.close),
        };
        tx.tickets()
            .reply(&NewReply {
                ticket_id: id,
                author,
                admin_id,
                body: body.clone(),
                attachments: Vec::new(),
                close,
            })
            .await?;
        if author != MessageAuthor::Client {
            for target in ticket.channel.reply_targets() {
                match target {
                    ReplyTarget::Telegram => {
                        tx.jobs()
                            .enqueue(
                                &JobPayload::Notify {
                                    client_id: ticket.client_id,
                                    template: REPLY_TEMPLATE.to_string(),
                                    params: serde_json::json!({
                                        "ticket_id": id,
                                        "body": body,
                                        "closed": close,
                                    }),
                                },
                                None,
                            )
                            .await?;
                    }
                    ReplyTarget::Cabinet => {}
                    ReplyTarget::Email => {
                        if !self
                            .services
                            .modules()
                            .is_enabled(sn_domain::module::ModuleId::Mail)
                        {
                            tracing::debug!(ticket = %id, "модуль «Почта» выключен, ответ виден только в кабинете");
                        }
                    }
                }
            }
            tx.events()
                .push(&DomainEvent::TicketAnswered {
                    ticket_id: id,
                    client_id: ticket.client_id,
                })
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn close(&self, id: TicketId, actor: Actor) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        if let Actor::Client(client_id) = actor {
            let owner: Option<i64> =
                sqlx::query_scalar("SELECT client_id FROM tickets WHERE id = $1")
                    .bind(id.0)
                    .fetch_optional(conn.conn())
                    .await?;
            if owner != Some(client_id.0) {
                return Err(ServiceError::NotFound("обращение"));
            }
        }
        conn.tickets().close(id).await?;
        Ok(())
    }
}
