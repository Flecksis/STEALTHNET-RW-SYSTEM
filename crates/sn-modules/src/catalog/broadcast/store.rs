use super::{Audience, ButtonSpec, Cleaned, Status};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::ids::{AdminId, ClientId, TariffId};
use sn_domain::image::ImageInfo;
use sn_domain::paging::{Page, PageRequest};
use sn_store::{Db, StoreError, StoreResult};
use std::collections::HashMap;

const PREVIEW_CHARS: usize = 120;
const PROBLEMS: i64 = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageMeta {
    pub id: i64,
    pub content_type: String,
    pub width: i32,
    pub height: i32,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ImageData {
    pub id: i64,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub file_bot: Option<i64>,
    pub file_id: Option<String>,
}

impl ImageData {
    pub fn file_for(&self, bot: Option<i64>) -> Option<&str> {
        match (bot, self.file_bot) {
            (Some(bot), Some(file_bot)) if bot == file_bot => self.file_id.as_deref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub total: i64,
    pub sent: i64,
    pub failed: i64,
    pub blocked: i64,
    pub skipped: i64,
    pub pending: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub error: String,
    pub count: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Reach {
    pub recipients: i64,
    pub stopped: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Broadcast {
    pub id: i64,
    pub status: Status,
    pub text: String,
    pub image: Option<ImageMeta>,
    pub buttons: Vec<ButtonSpec>,
    pub audience: Audience,
    pub audience_label: String,
    pub send_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub notice: Option<String>,
    pub progress: Progress,
    pub problems: Vec<Problem>,
    pub queued_behind: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListItem {
    pub id: i64,
    pub status: Status,
    pub preview: String,
    pub has_image: bool,
    pub buttons: usize,
    pub audience: Audience,
    pub audience_label: String,
    pub send_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub notice: Option<String>,
    pub progress: Progress,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    text: String,
    image_id: Option<i64>,
    buttons: serde_json::Value,
    audience: serde_json::Value,
    status: String,
    send_at: Option<DateTime<Utc>>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    notice: Option<String>,
    tariff_name: Option<String>,
}

const COLUMNS: &str = "b.id, b.text, b.image_id, b.buttons, b.audience, b.status, b.send_at, b.started_at, \
                       b.finished_at, b.created_at, b.updated_at, b.notice, \
                       (SELECT t.name FROM tariffs t WHERE t.id = (b.audience ->> 'tariff_id')::bigint) AS tariff_name";

struct Parsed {
    status: Status,
    buttons: Vec<ButtonSpec>,
    audience: Audience,
    audience_label: String,
}

fn parse(row: &Row) -> StoreResult<Parsed> {
    let status = Status::parse(&row.status)
        .ok_or_else(|| StoreError::Decode(format!("состояние рассылки: {}", row.status)))?;
    let buttons: Vec<ButtonSpec> = serde_json::from_value(row.buttons.clone())
        .map_err(|e| StoreError::Decode(format!("кнопки рассылки: {e}")))?;
    let audience: Audience = serde_json::from_value(row.audience.clone())
        .map_err(|e| StoreError::Decode(format!("аудитория рассылки: {e}")))?;
    Ok(Parsed {
        status,
        audience_label: audience.describe(row.tariff_name.as_deref()),
        buttons,
        audience,
    })
}

fn json<T: Serialize>(value: &T) -> StoreResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| StoreError::Invalid(e.to_string()))
}

pub fn preview(html: &str) -> String {
    let mut out = String::with_capacity(html.len().min(PREVIEW_CHARS * 4));
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&");
    let line = decoded
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= PREVIEW_CHARS {
        line
    } else {
        let mut cut: String = line.chars().take(PREVIEW_CHARS - 1).collect();
        cut.push('…');
        cut
    }
}

pub async fn insert(
    db: &mut impl Db,
    content: &Cleaned,
    admin: Option<AdminId>,
) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO broadcasts (text, image_id, buttons, audience, created_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(&content.text)
    .bind(content.image_id)
    .bind(json(&content.buttons)?)
    .bind(json(&content.audience)?)
    .bind(admin.map(|a| a.0))
    .fetch_one(db.conn())
    .await?)
}

pub async fn update(db: &mut impl Db, id: i64, content: &Cleaned) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET text = $2, image_id = $3, buttons = $4, audience = $5 WHERE id = $1",
    )
    .bind(id)
    .bind(&content.text)
    .bind(content.image_id)
    .bind(json(&content.buttons)?)
    .bind(json(&content.audience)?)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn delete(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query("DELETE FROM broadcasts WHERE id = $1")
        .bind(id)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn lock(db: &mut impl Db, id: i64) -> StoreResult<Status> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM broadcasts WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(db.conn())
            .await?;
    let status = status.ok_or(StoreError::NotFound("рассылка"))?;
    Status::parse(&status)
        .ok_or_else(|| StoreError::Decode(format!("состояние рассылки: {status}")))
}

async fn progress_of(db: &mut impl Db, ids: &[i64]) -> StoreResult<HashMap<i64, Progress>> {
    let rows: Vec<(i64, i64, i64, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT broadcast_id, count(*), \
         count(*) FILTER (WHERE state = 'sent'), count(*) FILTER (WHERE state = 'failed'), \
         count(*) FILTER (WHERE state = 'blocked'), count(*) FILTER (WHERE state = 'skipped'), \
         count(*) FILTER (WHERE state IN ('pending', 'sending')) \
         FROM broadcast_recipients WHERE broadcast_id = ANY($1) GROUP BY broadcast_id",
    )
    .bind(ids)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, total, sent, failed, blocked, skipped, pending)| {
            (
                id,
                Progress {
                    total,
                    sent,
                    failed,
                    blocked,
                    skipped,
                    pending,
                },
            )
        })
        .collect())
}

pub async fn image_meta(db: &mut impl Db, id: i64) -> StoreResult<Option<ImageMeta>> {
    let row: Option<(i64, String, i32, i32, i64)> = sqlx::query_as(
        "SELECT id, content_type, width, height, octet_length(bytes)::bigint FROM broadcast_images WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    Ok(
        row.map(|(id, content_type, width, height, size)| ImageMeta {
            id,
            content_type,
            width,
            height,
            size,
        }),
    )
}

pub async fn view(db: &mut impl Db, id: i64) -> StoreResult<Broadcast> {
    let row: Row = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM broadcasts b WHERE b.id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("рассылка"))?;
    let parsed = parse(&row)?;
    let image = match row.image_id {
        Some(image) => image_meta(db, image).await?,
        None => None,
    };
    let progress = progress_of(db, &[id])
        .await?
        .remove(&id)
        .unwrap_or_default();
    let problems: Vec<(String, i64)> = sqlx::query_as(
        "SELECT error, count(*) FROM broadcast_recipients \
         WHERE broadcast_id = $1 AND state = 'failed' AND error IS NOT NULL \
         GROUP BY error ORDER BY count(*) DESC, error LIMIT $2",
    )
    .bind(id)
    .bind(PROBLEMS)
    .fetch_all(db.conn())
    .await?;
    let queued_behind = if parsed.status == Status::Sending {
        sqlx::query_scalar(
            "SELECT b.id FROM broadcasts b WHERE b.status = 'sending' AND (b.started_at, b.id) < ($1, $2) \
             ORDER BY b.started_at, b.id LIMIT 1",
        )
        .bind(row.started_at)
        .bind(id)
        .fetch_optional(db.conn())
        .await?
    } else {
        None
    };
    Ok(Broadcast {
        id: row.id,
        status: parsed.status,
        text: row.text,
        image,
        buttons: parsed.buttons,
        audience: parsed.audience,
        audience_label: parsed.audience_label,
        send_at: row.send_at,
        started_at: row.started_at,
        finished_at: row.finished_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        notice: row.notice,
        progress,
        problems: problems
            .into_iter()
            .map(|(error, count)| Problem { error, count })
            .collect(),
        queued_behind,
    })
}

pub async fn list(db: &mut impl Db, page: PageRequest) -> StoreResult<Page<ListItem>> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM broadcasts b WHERE ($1::bigint IS NULL OR b.id < $1) ORDER BY b.id DESC LIMIT $2"
    ))
    .bind(page.after)
    .bind(page.fetch_limit())
    .fetch_all(db.conn())
    .await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut progress = progress_of(db, &ids).await?;
    let items = rows
        .into_iter()
        .map(|row| {
            let parsed = parse(&row)?;
            Ok(ListItem {
                id: row.id,
                status: parsed.status,
                preview: preview(&row.text),
                has_image: row.image_id.is_some(),
                buttons: parsed.buttons.len(),
                audience: parsed.audience,
                audience_label: parsed.audience_label,
                send_at: row.send_at,
                started_at: row.started_at,
                finished_at: row.finished_at,
                created_at: row.created_at,
                notice: row.notice,
                progress: progress.remove(&row.id).unwrap_or_default(),
            })
        })
        .collect::<StoreResult<Vec<ListItem>>>()?;
    Ok(Page::from_overfetch(items, page, |item| item.id))
}

fn status_of(alias: &str) -> String {
    format!(
        "COALESCE({alias}.status, CASE WHEN {alias}.expire_at IS NULL THEN NULL \
         WHEN {alias}.expire_at <= $1 THEN 'expired' ELSE 'active' END)"
    )
}

fn condition(audience: Audience) -> String {
    let s = status_of("s");
    match audience {
        Audience::All => "TRUE".into(),
        Audience::Active => format!("{s} = 'active' AND s.kind <> 'trial'"),
        Audience::Expiring { .. } => {
            format!("{s} = 'active' AND s.expire_at > $1 AND s.expire_at <= $2")
        }
        Audience::Expired => format!(
            "({s} = 'expired' OR (s.id IS NULL AND EXISTS (SELECT 1 FROM subscriptions h WHERE h.client_id = c.id)))"
        ),
        Audience::NeverConnected => format!("{s} = 'active' AND s.first_connected_at IS NULL"),
        Audience::Trial => format!("{s} = 'active' AND s.kind = 'trial'"),
        Audience::NoSubscription => {
            "NOT EXISTS (SELECT 1 FROM subscriptions h WHERE h.client_id = c.id)".into()
        }
        Audience::Tariff { .. } => format!(
            "EXISTS (SELECT 1 FROM subscriptions h WHERE h.client_id = c.id AND h.ended_at IS NULL \
             AND h.remna_missing_at IS NULL AND {} = 'active' AND h.tariff_id = $3)",
            status_of("h")
        ),
    }
}

fn audience_from(audience: Audience) -> String {
    let x = status_of("x");
    format!(
        "FROM clients c \
         JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = c.id AND i.kind = 'telegram' \
             AND i.value ~ '^[0-9]{{1,15}}$' ORDER BY i.id LIMIT 1) tg ON true \
         LEFT JOIN LATERAL (SELECT x.* FROM subscriptions x WHERE x.client_id = c.id AND x.ended_at IS NULL \
             AND x.remna_missing_at IS NULL ORDER BY CASE {x} WHEN 'active' THEN 0 WHEN 'limited' THEN 1 \
             WHEN 'disabled' THEN 2 WHEN 'expired' THEN 3 ELSE 4 END, x.expire_at DESC NULLS LAST, x.id DESC LIMIT 1) s ON true \
         WHERE c.deleted_at IS NULL AND c.status = 'active' AND {}",
        condition(audience)
    )
}

fn bound(audience: Audience, now: DateTime<Utc>) -> DateTime<Utc> {
    match audience {
        Audience::Expiring { days } => now + Duration::days(i64::from(days)),
        _ => now,
    }
}

pub async fn reach(db: &mut impl Db, audience: Audience, now: DateTime<Utc>) -> StoreResult<Reach> {
    let (recipients, stopped): (i64, i64) = sqlx::query_as(&format!(
        "SELECT count(*) FILTER (WHERE NOT c.bot_unreachable), count(*) FILTER (WHERE c.bot_unreachable) {}",
        audience_from(audience)
    ))
    .bind(now)
    .bind(bound(audience, now))
    .bind(audience.tariff().map_or(0, |t| t.0))
    .fetch_one(db.conn())
    .await?;
    Ok(Reach {
        recipients,
        stopped,
    })
}

pub async fn launch(
    db: &mut impl Db,
    id: i64,
    audience: Audience,
    now: DateTime<Utc>,
    admin: Option<AdminId>,
) -> StoreResult<i64> {
    let inserted = sqlx::query(&format!(
        "INSERT INTO broadcast_recipients (broadcast_id, client_id, chat_id) \
         SELECT $4, c.id, tg.value::bigint {} AND NOT c.bot_unreachable ON CONFLICT DO NOTHING",
        audience_from(audience)
    ))
    .bind(now)
    .bind(bound(audience, now))
    .bind(audience.tariff().map_or(0, |t| t.0))
    .bind(id)
    .execute(db.conn())
    .await?
    .rows_affected();
    let total = i64::try_from(inserted).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE broadcasts SET status = 'sending', started_at = $2, total = $3, notice = NULL, \
         started_by = COALESCE($4, started_by) WHERE id = $1",
    )
    .bind(id)
    .bind(now)
    .bind(i32::try_from(total).unwrap_or(i32::MAX))
    .bind(admin.map(|a| a.0))
    .execute(db.conn())
    .await?;
    Ok(total)
}

pub async fn finish_empty(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET status = 'done', finished_at = now(), \
         notice = 'Получателей не нашлось: в аудитории никого с Telegram, кто не остановил бота' WHERE id = $1",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn schedule(
    db: &mut impl Db,
    id: i64,
    at: DateTime<Utc>,
    admin: Option<AdminId>,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET status = 'scheduled', send_at = $2, started_by = $3, notice = NULL WHERE id = $1",
    )
    .bind(id)
    .bind(at)
    .bind(admin.map(|a| a.0))
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn unschedule(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET status = 'draft', send_at = NULL, started_by = NULL, notice = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn stop(db: &mut impl Db, id: i64, notice: Option<&str>) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcast_recipients SET state = 'skipped', done_at = now() \
         WHERE broadcast_id = $1 AND state = 'pending'",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    sqlx::query(
        "UPDATE broadcasts SET status = 'canceled', finished_at = now(), notice = $2 WHERE id = $1",
    )
    .bind(id)
    .bind(notice)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn image_exists(db: &mut impl Db, id: i64) -> StoreResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM broadcast_images WHERE id = $1)")
            .bind(id)
            .fetch_one(db.conn())
            .await?,
    )
}

pub async fn insert_image(
    db: &mut impl Db,
    bytes: &[u8],
    info: ImageInfo,
    admin: Option<AdminId>,
) -> StoreResult<ImageMeta> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO broadcast_images (content_type, bytes, width, height, created_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(info.kind.content_type())
    .bind(bytes)
    .bind(i32::try_from(info.width).unwrap_or(i32::MAX))
    .bind(i32::try_from(info.height).unwrap_or(i32::MAX))
    .bind(admin.map(|a| a.0))
    .fetch_one(db.conn())
    .await?;
    image_meta(db, id)
        .await?
        .ok_or(StoreError::NotFound("картинка рассылки"))
}

pub async fn image(db: &mut impl Db, id: i64) -> StoreResult<ImageData> {
    sqlx::query_as::<_, ImageData>(
        "SELECT id, content_type, bytes, file_bot, file_id FROM broadcast_images WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("картинка рассылки"))
}

pub async fn remember_file(db: &mut impl Db, id: i64, bot: i64, file_id: &str) -> StoreResult<()> {
    sqlx::query("UPDATE broadcast_images SET file_bot = $2, file_id = $3 WHERE id = $1")
        .bind(id)
        .bind(bot)
        .bind(file_id)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn forget_file(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query("UPDATE broadcast_images SET file_bot = NULL, file_id = NULL WHERE id = $1")
        .bind(id)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn prune_images(db: &mut impl Db, older_than_hours: i64) -> StoreResult<u64> {
    Ok(sqlx::query(
        "DELETE FROM broadcast_images i WHERE i.created_at < now() - make_interval(hours => $1::int) \
         AND NOT EXISTS (SELECT 1 FROM broadcasts b WHERE b.image_id = i.id)",
    )
    .bind(i32::try_from(older_than_hours).unwrap_or(i32::MAX))
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn chat_of(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<i64>> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM clients WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(client_id.0)
    .fetch_one(db.conn())
    .await?;
    if !exists {
        return Err(StoreError::NotFound("клиент"));
    }
    let value: Option<String> = sqlx::query_scalar(
        "SELECT value FROM client_identities WHERE client_id = $1 AND kind = 'telegram' ORDER BY id LIMIT 1",
    )
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?;
    Ok(value.and_then(|v| v.parse::<i64>().ok()).filter(|v| *v > 0))
}

pub async fn tariffs(db: &mut impl Db) -> StoreResult<Vec<super::TariffOption>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT t.id, t.name FROM tariffs t JOIN tariff_categories g ON g.id = t.category_id \
         WHERE t.deleted_at IS NULL ORDER BY g.sort, g.id, t.sort, t.id",
    )
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| super::TariffOption {
            id: TariffId(id),
            name,
        })
        .collect())
}

pub async fn tariff_alive(db: &mut impl Db, id: TariffId) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tariffs WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(id.0)
    .fetch_one(db.conn())
    .await?)
}

pub async fn enabled_modules(db: &mut impl Db) -> StoreResult<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT module FROM module_states WHERE enabled")
            .fetch_all(db.conn())
            .await?,
    )
}

pub struct Lease<'a> {
    pub key: &'a str,
    pub owner: &'a str,
    pub seconds: f64,
}

pub async fn take_lease(db: &mut impl Db, lease: &Lease<'_>) -> StoreResult<bool> {
    let taken: Option<bool> = sqlx::query_scalar(
        "INSERT INTO bot_state (key, value, updated_at) \
         VALUES ($1, jsonb_build_object('owner', $2::text, 'until', now() + make_interval(secs => $3)), now()) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now() \
         WHERE bot_state.value ->> 'owner' = $2 OR (bot_state.value ->> 'until')::timestamptz < now() \
         RETURNING true",
    )
    .bind(lease.key)
    .bind(lease.owner)
    .bind(lease.seconds)
    .fetch_optional(db.conn())
    .await?;
    Ok(taken.unwrap_or(false))
}

pub async fn drop_lease(db: &mut impl Db, key: &str, owner: &str) -> StoreResult<()> {
    sqlx::query("DELETE FROM bot_state WHERE key = $1 AND value ->> 'owner' = $2")
        .bind(key)
        .bind(owner)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn abandon_flights(db: &mut impl Db, stale: Option<(i64, i64)>) -> StoreResult<u64> {
    let (broadcast, seconds) = stale.map_or((None, 0), |(id, seconds)| (Some(id), seconds));
    Ok(sqlx::query(
        "UPDATE broadcast_recipients SET state = 'failed', done_at = now(), \
         error = 'Отправка прервана перезапуском: дошло ли сообщение, неизвестно, повторно не отправляли' \
         WHERE state = 'sending' AND ($1::bigint IS NULL \
             OR (broadcast_id = $1 AND claimed_at < now() - make_interval(secs => $2::double precision)))",
    )
    .bind(broadcast)
    .bind(seconds)
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn due(db: &mut impl Db, limit: i64) -> StoreResult<Vec<(i64, Audience)>> {
    let rows: Vec<(i64, serde_json::Value)> = sqlx::query_as(
        "SELECT id, audience FROM broadcasts WHERE status = 'scheduled' AND send_at <= now() \
         ORDER BY send_at, id LIMIT $1 FOR UPDATE SKIP LOCKED",
    )
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    rows.into_iter()
        .map(|(id, audience)| {
            serde_json::from_value(audience)
                .map(|audience| (id, audience))
                .map_err(|e| StoreError::Decode(format!("аудитория рассылки: {e}")))
        })
        .collect()
}

pub async fn next_sending(db: &mut impl Db) -> StoreResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM broadcasts WHERE status = 'sending' ORDER BY started_at, id LIMIT 1",
    )
    .fetch_optional(db.conn())
    .await?)
}

pub struct Message {
    pub text: String,
    pub buttons: Vec<ButtonSpec>,
    pub image: Option<ImageData>,
}

pub async fn message(db: &mut impl Db, id: i64) -> StoreResult<Message> {
    let row: Option<(String, serde_json::Value, Option<i64>)> =
        sqlx::query_as("SELECT text, buttons, image_id FROM broadcasts WHERE id = $1")
            .bind(id)
            .fetch_optional(db.conn())
            .await?;
    let (text, buttons, image_id) = row.ok_or(StoreError::NotFound("рассылка"))?;
    let buttons = serde_json::from_value(buttons)
        .map_err(|e| StoreError::Decode(format!("кнопки рассылки: {e}")))?;
    let image = match image_id {
        Some(image_id) => Some(image(db, image_id).await?),
        None => None,
    };
    Ok(Message {
        text,
        buttons,
        image,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    pub client_id: ClientId,
    pub chat_id: i64,
    pub attempts: i32,
}

pub async fn claim(db: &mut impl Db, id: i64, limit: i64) -> StoreResult<Vec<Claimed>> {
    let rows: Vec<(i64, i64, i32)> = sqlx::query_as(
        "WITH pick AS MATERIALIZED ( \
             SELECT q.client_id FROM broadcast_recipients q \
             WHERE q.broadcast_id = $1 AND q.state = 'pending' AND (q.not_before IS NULL OR q.not_before <= now()) \
             ORDER BY q.client_id LIMIT $2 FOR UPDATE SKIP LOCKED) \
         UPDATE broadcast_recipients r SET state = 'sending', attempts = r.attempts + 1, claimed_at = now() \
         FROM pick WHERE r.broadcast_id = $1 AND r.client_id = pick.client_id \
           AND EXISTS (SELECT 1 FROM broadcasts b WHERE b.id = $1 AND b.status = 'sending') \
         RETURNING r.client_id, r.chat_id, r.attempts",
    )
    .bind(id)
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(client_id, chat_id, attempts)| Claimed {
            client_id: ClientId(client_id),
            chat_id,
            attempts,
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome<'a> {
    Sent,
    Blocked(&'a str),
    Retry {
        after_ms: i64,
        error: Option<&'a str>,
        counted: bool,
    },
    Failed(&'a str),
}

pub async fn record(
    db: &mut impl Db,
    id: i64,
    client_id: ClientId,
    outcome: &Outcome<'_>,
) -> StoreResult<bool> {
    let done = match outcome {
        Outcome::Sent => {
            sqlx::query(
                "UPDATE broadcast_recipients SET state = 'sent', done_at = now(), error = NULL \
                 WHERE broadcast_id = $1 AND client_id = $2 AND state = 'sending'",
            )
            .bind(id)
            .bind(client_id.0)
            .execute(db.conn())
            .await?
        }
        Outcome::Blocked(error) => {
            sqlx::query("UPDATE clients SET bot_unreachable = true WHERE id = $1")
                .bind(client_id.0)
                .execute(db.conn())
                .await?;
            sqlx::query(
                "UPDATE broadcast_recipients SET state = 'blocked', done_at = now(), error = $3 \
                 WHERE broadcast_id = $1 AND client_id = $2 AND state = 'sending'",
            )
            .bind(id)
            .bind(client_id.0)
            .bind(error)
            .execute(db.conn())
            .await?
        }
        Outcome::Retry {
            after_ms,
            error,
            counted,
        } => {
            sqlx::query(
                "UPDATE broadcast_recipients SET \
                 state = CASE WHEN (SELECT b.status FROM broadcasts b WHERE b.id = $1) = 'sending' \
                     THEN 'pending' ELSE 'skipped' END, \
                 not_before = now() + make_interval(secs => $3::double precision / 1000), \
                 error = COALESCE($4, error), \
                 attempts = CASE WHEN $5 THEN attempts ELSE greatest(attempts - 1, 0) END, \
                 done_at = CASE WHEN (SELECT b.status FROM broadcasts b WHERE b.id = $1) = 'sending' \
                     THEN NULL ELSE now() END \
                 WHERE broadcast_id = $1 AND client_id = $2 AND state = 'sending'",
            )
            .bind(id)
            .bind(client_id.0)
            .bind(*after_ms)
            .bind(*error)
            .bind(*counted)
            .execute(db.conn())
            .await?
        }
        Outcome::Failed(error) => {
            sqlx::query(
                "UPDATE broadcast_recipients SET state = 'failed', done_at = now(), error = $3 \
                 WHERE broadcast_id = $1 AND client_id = $2 AND state = 'sending'",
            )
            .bind(id)
            .bind(client_id.0)
            .bind(error)
            .execute(db.conn())
            .await?
        }
    };
    Ok(done.rows_affected() > 0)
}

pub async fn complete(db: &mut impl Db, id: i64) -> StoreResult<bool> {
    let done = sqlx::query(
        "UPDATE broadcasts SET status = 'done', finished_at = now(), notice = NULL \
         WHERE id = $1 AND status = 'sending' AND NOT EXISTS ( \
             SELECT 1 FROM broadcast_recipients r WHERE r.broadcast_id = $1 AND r.state IN ('pending', 'sending'))",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn status(db: &mut impl Db, id: i64) -> StoreResult<Option<Status>> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM broadcasts WHERE id = $1")
        .bind(id)
        .fetch_optional(db.conn())
        .await?;
    Ok(status.as_deref().and_then(Status::parse))
}

pub async fn next_retry_ms(db: &mut impl Db, id: i64) -> StoreResult<Option<i64>> {
    let wait: Option<f64> = sqlx::query_scalar(
        "SELECT (extract(epoch FROM (min(not_before) - now())) * 1000)::double precision FROM broadcast_recipients \
         WHERE broadcast_id = $1 AND state = 'pending'",
    )
    .bind(id)
    .fetch_one(db.conn())
    .await?;
    Ok(wait.map(|ms| ms.max(0.0).round() as i64))
}

pub async fn set_notice(db: &mut impl Db, id: i64, notice: Option<&str>) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET notice = $2 WHERE id = $1 AND status = 'sending' AND notice IS DISTINCT FROM $2",
    )
    .bind(id)
    .bind(notice)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn sent_count(db: &mut impl Db, id: i64) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM broadcast_recipients WHERE broadcast_id = $1 AND state = 'sent'",
    )
    .bind(id)
    .fetch_one(db.conn())
    .await?)
}
