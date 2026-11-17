use crate::DomainError;
use crate::ids::{AdminId, ClientId, TicketId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MAX_OPEN_TICKETS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketChannel {
    Bot,
    Cabinet,
    Miniapp,
    Unknown,
}

impl TicketChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            TicketChannel::Bot => "bot",
            TicketChannel::Cabinet => "cabinet",
            TicketChannel::Miniapp => "miniapp",
            TicketChannel::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "bot" => Ok(TicketChannel::Bot),
            "cabinet" => Ok(TicketChannel::Cabinet),
            "miniapp" => Ok(TicketChannel::Miniapp),
            "unknown" => Ok(TicketChannel::Unknown),
            other => Err(DomainError::unknown("ticket_channel", other)),
        }
    }

    pub const fn reply_targets(self) -> &'static [ReplyTarget] {
        match self {
            TicketChannel::Bot => &[ReplyTarget::Telegram],
            TicketChannel::Cabinet | TicketChannel::Miniapp => {
                &[ReplyTarget::Cabinet, ReplyTarget::Email]
            }
            TicketChannel::Unknown => &[ReplyTarget::Cabinet, ReplyTarget::Telegram],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyTarget {
    Telegram,
    Cabinet,
    Email,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageAuthor {
    Client,
    Support,
    Ai,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketFilter {
    Waiting,
    Answered,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub id: TicketId,
    pub client_id: ClientId,
    pub status: TicketStatus,
    pub channel: TicketChannel,
    pub subject: Option<String>,
    pub last_author: MessageAuthor,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Ticket {
    pub const fn filter(&self) -> TicketFilter {
        match (self.status, self.last_author) {
            (TicketStatus::Closed, _) => TicketFilter::Closed,
            (TicketStatus::Open, MessageAuthor::Client) => TicketFilter::Waiting,
            (TicketStatus::Open, _) => TicketFilter::Answered,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketMessage {
    pub ticket_id: TicketId,
    pub author: MessageAuthor,
    pub admin_id: Option<AdminId>,
    pub body: String,
    pub attachments: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn фильтры_поддержки() {
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap();
        let mut ticket = Ticket {
            id: TicketId(1),
            client_id: ClientId(1),
            status: TicketStatus::Open,
            channel: TicketChannel::Bot,
            subject: None,
            last_author: MessageAuthor::Client,
            created_at: at,
            updated_at: at,
            closed_at: None,
        };
        assert_eq!(ticket.filter(), TicketFilter::Waiting);
        ticket.last_author = MessageAuthor::Support;
        assert_eq!(ticket.filter(), TicketFilter::Answered);
        ticket.status = TicketStatus::Closed;
        assert_eq!(ticket.filter(), TicketFilter::Closed);
        assert_eq!(TicketChannel::Bot.reply_targets(), [ReplyTarget::Telegram]);
    }
}
