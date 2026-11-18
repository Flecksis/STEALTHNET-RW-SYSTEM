use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::ids::{ClientId, TicketId};
use sn_domain::support::{HandoffReason, MessageAuthor, TicketChannel};
use sn_store::{Db, NewReply, StoreError, StoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    pub ticket_id: TicketId,
    pub client_id: ClientId,
    pub channel: TicketChannel,
    pub last_message_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spent {
    pub model: &'static str,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

impl Spent {
    pub const NONE: Spent = Spent {
        model: "",
        prompt_tokens: 0,
        completion_tokens: 0,
    };
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub answers: i64,
    pub handoffs: i64,
    pub tokens: i64,
    pub clients: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LastError {
    pub at: DateTime<Utc>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    Answered,
    Stale,
}

fn store_err(err: sqlx::Error) -> StoreError {
    StoreError::from(err)
}

fn model(spent: &Spent) -> Option<&'static str> {
    (!spent.model.is_empty()).then_some(spent.model)
}

pub async fn claim(db: &mut impl Db, lease_seconds: i64) -> StoreResult<Option<Claimed>> {
    let row: Option<(i64, i64, String, i64)> = sqlx::query_as(
        "UPDATE tickets SET ai_claimed_until = now() + make_interval(secs => $1) \
         WHERE id = (SELECT id FROM tickets \
             WHERE status = 'open' AND ai_state = 'active' AND last_author = 'client' \
               AND (ai_claimed_until IS NULL OR ai_claimed_until < now()) \
             ORDER BY last_message_id NULLS FIRST, id LIMIT 1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, client_id, channel, coalesce(last_message_id, 0)",
    )
    .bind(lease_seconds as f64)
    .fetch_optional(db.conn())
    .await
    .map_err(store_err)?;
    row.map(|(id, client, channel, last)| {
        Ok(Claimed {
            ticket_id: TicketId(id),
            client_id: ClientId(client),
            channel: TicketChannel::parse(&channel)
                .map_err(|e| StoreError::Decode(e.to_string()))?,
            last_message_id: last,
        })
    })
    .transpose()
}

pub async fn release(db: &mut impl Db, ticket: TicketId) -> StoreResult<()> {
    sqlx::query("UPDATE tickets SET ai_claimed_until = NULL WHERE id = $1")
        .bind(ticket.0)
        .execute(db.conn())
        .await
        .map_err(store_err)?;
    Ok(())
}

struct Entry<'a> {
    ticket: TicketId,
    client: ClientId,
    message: Option<i64>,
    outcome: &'static str,
    reason: Option<HandoffReason>,
    error: Option<&'a str>,
}

async fn log(db: &mut impl Db, entry: &Entry<'_>, spent: &Spent) -> StoreResult<()> {
    sqlx::query(
        "INSERT INTO ai_support_log (ticket_id, client_id, message_id, outcome, reason, model, \
         prompt_tokens, completion_tokens, error) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(entry.ticket.0)
    .bind(entry.client.0)
    .bind(entry.message)
    .bind(entry.outcome)
    .bind(entry.reason.map(HandoffReason::as_str))
    .bind(model(spent))
    .bind(i32::try_from(spent.prompt_tokens).unwrap_or(i32::MAX))
    .bind(i32::try_from(spent.completion_tokens).unwrap_or(i32::MAX))
    .bind(entry.error.map(|e| e.chars().take(500).collect::<String>()))
    .execute(db.conn())
    .await
    .map_err(store_err)?;
    Ok(())
}

pub async fn hand_off(
    db: &mut impl Db,
    ticket: TicketId,
    reason: HandoffReason,
    spent: &Spent,
    error: Option<&str>,
) -> StoreResult<bool> {
    let client: Option<i64> = sqlx::query_scalar(
        "UPDATE tickets SET ai_state = 'handed_off', ai_reason = $2, ai_handoff_at = now(), \
         ai_claimed_until = NULL, \
         waiting_message_id = CASE WHEN $3 AND last_author <> 'client' THEN last_message_id \
             ELSE waiting_message_id END, \
         last_author = CASE WHEN $3 THEN 'client' ELSE last_author END \
         WHERE id = $1 AND ai_state = 'active' RETURNING client_id",
    )
    .bind(ticket.0)
    .bind(reason.as_str())
    .bind(reason.alerts())
    .fetch_optional(db.conn())
    .await
    .map_err(store_err)?;
    let Some(client) = client else {
        return Ok(false);
    };
    if reason.alerts() {
        log(
            db,
            &Entry {
                ticket,
                client: ClientId(client),
                message: None,
                outcome: "handoff",
                reason: Some(reason),
                error,
            },
            spent,
        )
        .await?;
    }
    Ok(true)
}

pub async fn release_all(db: &mut impl Db) -> StoreResult<u64> {
    Ok(sqlx::query(
        "UPDATE tickets SET ai_state = 'handed_off', ai_reason = 'disabled', ai_handoff_at = now(), \
         ai_claimed_until = NULL WHERE ai_state = 'active'",
    )
    .execute(db.conn())
    .await
    .map_err(store_err)?
    .rows_affected())
}

pub async fn answer(
    db: &mut impl Db,
    claimed: &Claimed,
    body: &str,
    spent: &Spent,
) -> StoreResult<Saved> {
    let current: Option<(String, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT status, ai_state, last_message_id FROM tickets WHERE id = $1 FOR UPDATE",
    )
    .bind(claimed.ticket_id.0)
    .fetch_optional(db.conn())
    .await
    .map_err(store_err)?;
    let fresh = current.as_ref().is_some_and(|(status, state, last)| {
        status == "open"
            && state.as_deref() == Some("active")
            && last.unwrap_or(0) == claimed.last_message_id
    });
    if !fresh {
        log(
            db,
            &Entry {
                ticket: claimed.ticket_id,
                client: claimed.client_id,
                message: None,
                outcome: "stale",
                reason: None,
                error: None,
            },
            spent,
        )
        .await?;
        release(db, claimed.ticket_id).await?;
        return Ok(Saved::Stale);
    }
    db.tickets()
        .reply(&NewReply {
            ticket_id: claimed.ticket_id,
            author: MessageAuthor::Ai,
            admin_id: None,
            body: body.to_string(),
            attachments: Vec::new(),
            close: false,
        })
        .await?;
    let message: Option<i64> =
        sqlx::query_scalar("SELECT last_message_id FROM tickets WHERE id = $1")
            .bind(claimed.ticket_id.0)
            .fetch_one(db.conn())
            .await
            .map_err(store_err)?;
    log(
        db,
        &Entry {
            ticket: claimed.ticket_id,
            client: claimed.client_id,
            message,
            outcome: "answered",
            reason: None,
            error: None,
        },
        spent,
    )
    .await?;
    release(db, claimed.ticket_id).await?;
    Ok(Saved::Answered)
}

pub async fn client_answers(
    db: &mut impl Db,
    client: ClientId,
    since: DateTime<Utc>,
) -> StoreResult<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM ai_support_log WHERE client_id = $1 AND outcome = 'answered' AND created_at >= $2",
    )
    .bind(client.0)
    .bind(since)
    .fetch_one(db.conn())
    .await
    .map_err(store_err)
}

pub async fn usage(db: &mut impl Db, since: DateTime<Utc>) -> StoreResult<Usage> {
    let (answers, handoffs, tokens, clients): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE outcome = 'answered'), \
                count(*) FILTER (WHERE outcome = 'handoff'), \
                coalesce(sum(prompt_tokens::bigint + completion_tokens::bigint), 0)::bigint, \
                count(DISTINCT client_id) FILTER (WHERE outcome = 'answered') \
         FROM ai_support_log WHERE created_at >= $1",
    )
    .bind(since)
    .fetch_one(db.conn())
    .await
    .map_err(store_err)?;
    Ok(Usage {
        answers,
        handoffs,
        tokens,
        clients,
    })
}

pub async fn last_error(db: &mut impl Db) -> StoreResult<Option<LastError>> {
    let row: Option<(DateTime<Utc>, String)> = sqlx::query_as(
        "SELECT created_at, error FROM ai_support_log WHERE error IS NOT NULL ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(db.conn())
    .await
    .map_err(store_err)?;
    Ok(row.map(|(at, text)| LastError { at, text }))
}

pub async fn bot_username(db: &mut impl Db) -> StoreResult<Option<String>> {
    Ok(db
        .settings()
        .get(sn_domain::settings::keys::BOT_USERNAME)
        .await?
        .and_then(|v| v.as_str().map(str::to_string))
        .map(|name| name.trim().trim_start_matches('@').to_string())
        .filter(|name| !name.is_empty()))
}
