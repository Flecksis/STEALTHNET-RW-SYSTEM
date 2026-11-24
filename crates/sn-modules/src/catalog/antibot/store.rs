use super::signals::Signal;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::ids::{AdminId, ClientId};
use sn_domain::paging::{Page, PageRequest};
use sn_store::{ClientPerson, Db, StoreResult};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mark {
    pub client_id: ClientId,
    pub reasons: Vec<Signal>,
    pub details: serde_json::Value,
    pub marked_at: DateTime<Utc>,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub cleared_at: Option<DateTime<Utc>>,
    pub cleared_by: Option<AdminId>,
}

#[derive(sqlx::FromRow)]
struct MarkRow {
    client_id: i64,
    reasons: serde_json::Value,
    details: serde_json::Value,
    marked_at: DateTime<Utc>,
    confirmed_at: Option<DateTime<Utc>>,
    cleared_at: Option<DateTime<Utc>>,
    cleared_by: Option<i64>,
}

fn signals_of(value: &serde_json::Value) -> Vec<Signal> {
    serde_json::from_value::<Vec<Signal>>(value.clone()).unwrap_or_default()
}

impl From<MarkRow> for Mark {
    fn from(row: MarkRow) -> Self {
        Mark {
            client_id: ClientId(row.client_id),
            reasons: signals_of(&row.reasons),
            details: row.details,
            marked_at: row.marked_at,
            confirmed_at: row.confirmed_at,
            cleared_at: row.cleared_at,
            cleared_by: row.cleared_by.map(AdminId),
        }
    }
}

const MARK_COLUMNS: &str =
    "client_id, reasons, details, marked_at, confirmed_at, cleared_at, cleared_by";

pub async fn mark_of(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<Mark>> {
    let row: Option<MarkRow> = sqlx::query_as(&format!(
        "SELECT {MARK_COLUMNS} FROM antibot_marks WHERE client_id = $1"
    ))
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?;
    Ok(row.map(Mark::from))
}

pub async fn marks_among(db: &mut impl Db, ids: &[i64]) -> StoreResult<HashMap<ClientId, Mark>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<MarkRow> = sqlx::query_as(&format!(
        "SELECT {MARK_COLUMNS} FROM antibot_marks WHERE client_id = ANY($1)"
    ))
    .bind(ids)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (ClientId(row.client_id), Mark::from(row)))
        .collect())
}

pub async fn open_marks_among(
    db: &mut impl Db,
    ids: &[i64],
) -> StoreResult<HashMap<ClientId, Vec<Signal>>> {
    Ok(marks_among(db, ids)
        .await?
        .into_iter()
        .filter(|(_, m)| m.cleared_at.is_none())
        .map(|(id, m)| (id, m.reasons))
        .collect())
}

pub async fn save_mark(
    db: &mut impl Db,
    client_id: ClientId,
    reasons: &[Signal],
    details: &serde_json::Value,
) -> StoreResult<bool> {
    let created: Option<i64> = sqlx::query_scalar(
        "INSERT INTO antibot_marks (client_id, reasons, details) VALUES ($1, $2, $3) \
         ON CONFLICT (client_id) DO NOTHING RETURNING client_id",
    )
    .bind(client_id.0)
    .bind(serde_json::json!(reasons))
    .bind(details)
    .fetch_optional(db.conn())
    .await?;
    if created.is_some() {
        return Ok(true);
    }
    sqlx::query(
        "UPDATE antibot_marks SET reasons = $2, details = details || $3 \
         WHERE client_id = $1 AND cleared_at IS NULL AND reasons IS DISTINCT FROM $2",
    )
    .bind(client_id.0)
    .bind(serde_json::json!(reasons))
    .bind(details)
    .execute(db.conn())
    .await?;
    Ok(false)
}

pub async fn clear(
    db: &mut impl Db,
    client_id: ClientId,
    admin: Option<AdminId>,
) -> StoreResult<bool> {
    let changed = sqlx::query(
        "UPDATE antibot_marks SET cleared_at = now(), cleared_by = $2 \
         WHERE client_id = $1 AND cleared_at IS NULL",
    )
    .bind(client_id.0)
    .bind(admin.map(|a| a.0))
    .execute(db.conn())
    .await?
    .rows_affected();
    Ok(changed > 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Marked,
    Asked,
    Confirmed,
    TrialDenied,
    PromoDenied,
    NoBonus,
    Cleared,
    RouletteAsked,
    RouletteDenied,
    RouletteNoted,
    ContestAsked,
    ContestDenied,
    ContestNoted,
    ContestExcluded,
    PromoLinkAsked,
    PromoLinkDenied,
}

impl Action {
    pub const ALL: [Action; 16] = [
        Action::Marked,
        Action::Asked,
        Action::Confirmed,
        Action::TrialDenied,
        Action::PromoDenied,
        Action::NoBonus,
        Action::Cleared,
        Action::RouletteAsked,
        Action::RouletteDenied,
        Action::RouletteNoted,
        Action::ContestAsked,
        Action::ContestDenied,
        Action::ContestNoted,
        Action::ContestExcluded,
        Action::PromoLinkAsked,
        Action::PromoLinkDenied,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Action::Marked => "marked",
            Action::Asked => "asked",
            Action::Confirmed => "confirmed",
            Action::TrialDenied => "trial_denied",
            Action::PromoDenied => "promo_denied",
            Action::NoBonus => "no_bonus",
            Action::Cleared => "cleared",
            Action::RouletteAsked => "roulette_asked",
            Action::RouletteDenied => "roulette_denied",
            Action::RouletteNoted => "roulette_noted",
            Action::ContestAsked => "contest_asked",
            Action::ContestDenied => "contest_denied",
            Action::ContestNoted => "contest_noted",
            Action::ContestExcluded => "contest_excluded",
            Action::PromoLinkAsked => "promo_link_asked",
            Action::PromoLinkDenied => "promo_link_denied",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == value)
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            Action::Marked => "Отмечен как подозрительный",
            Action::Asked => "Попросили подтвердить, что не робот",
            Action::Confirmed => "Прошёл проверку «Я не робот»",
            Action::TrialDenied => "Пробный период не выдан",
            Action::PromoDenied => "Бесплатные дни не выданы",
            Action::NoBonus => "Бонус пригласившему не начислен",
            Action::Cleared => "Отметку сняли",
            Action::RouletteAsked => "Попросили подтвердить перед вращением рулетки",
            Action::RouletteDenied => "Бесплатное вращение рулетки не выдано",
            Action::RouletteNoted => "Бесплатное вращение у отмеченного клиента",
            Action::ContestAsked => "Попросили подтвердить перед участием в конкурсе",
            Action::ContestDenied => "Участие в конкурсе не принято",
            Action::ContestNoted => "Отмеченный клиент участвует в конкурсе",
            Action::ContestExcluded => "Не прошли проверку на жеребьёвке конкурса",
            Action::PromoLinkAsked => "Попросили подтвердить перед промо-ссылкой",
            Action::PromoLinkDenied => "Подписка по промо-ссылке не выдана",
        }
    }
}

pub struct NewEvent<'a> {
    pub client_id: ClientId,
    pub action: Action,
    pub reasons: &'a [Signal],
    pub details: serde_json::Value,
    pub dedupe: Option<String>,
    pub admin: Option<AdminId>,
}

pub async fn event(db: &mut impl Db, event: &NewEvent<'_>) -> StoreResult<bool> {
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO antibot_events (client_id, action, reasons, details, dedupe, admin_id) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(event.client_id.0)
    .bind(event.action.as_str())
    .bind(serde_json::json!(event.reasons))
    .bind(&event.details)
    .bind(event.dedupe.as_deref())
    .bind(event.admin.map(|a| a.0))
    .fetch_optional(db.conn())
    .await?;
    Ok(id.is_some())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalFilter {
    All,
    Signup,
    Trial,
    Promo,
    Referral,
    Roulette,
    Contest,
    Cleared,
}

impl JournalFilter {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.map(str::trim).unwrap_or("all") {
            "" | "all" => Some(JournalFilter::All),
            "signup" => Some(JournalFilter::Signup),
            "trial" => Some(JournalFilter::Trial),
            "promo" => Some(JournalFilter::Promo),
            "referral" => Some(JournalFilter::Referral),
            "roulette" => Some(JournalFilter::Roulette),
            "contest" => Some(JournalFilter::Contest),
            "cleared" => Some(JournalFilter::Cleared),
            _ => None,
        }
    }

    fn actions(self) -> Vec<&'static str> {
        let list: &[Action] = match self {
            JournalFilter::All => &Action::ALL,
            JournalFilter::Signup => &[Action::Marked],
            JournalFilter::Trial => &[Action::Asked, Action::TrialDenied],
            JournalFilter::Promo => &[
                Action::PromoDenied,
                Action::PromoLinkAsked,
                Action::PromoLinkDenied,
            ],
            JournalFilter::Referral => &[Action::NoBonus],
            JournalFilter::Roulette => &[
                Action::RouletteAsked,
                Action::RouletteDenied,
                Action::RouletteNoted,
            ],
            JournalFilter::Contest => &[
                Action::ContestAsked,
                Action::ContestDenied,
                Action::ContestNoted,
                Action::ContestExcluded,
            ],
            JournalFilter::Cleared => &[Action::Cleared],
        };
        list.iter().map(|a| a.as_str()).collect()
    }

    fn confirmed_for(self) -> &'static [&'static str] {
        match self {
            JournalFilter::Trial => &["trial"],
            JournalFilter::Promo => &["promo_link"],
            JournalFilter::Roulette => &["roulette"],
            JournalFilter::Contest => &["contest"],
            JournalFilter::All
            | JournalFilter::Signup
            | JournalFilter::Referral
            | JournalFilter::Cleared => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reason {
    pub code: Signal,
    pub title: &'static str,
}

pub fn reasons(signals: &[Signal]) -> Vec<Reason> {
    signals
        .iter()
        .map(|s| Reason {
            code: *s,
            title: s.title_ru(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalItem {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub client_id: ClientId,
    pub client: Option<ClientPerson>,
    pub action: Action,
    pub title: &'static str,
    pub reasons: Vec<Reason>,
    pub details: serde_json::Value,
    pub admin: Option<String>,
}

#[derive(sqlx::FromRow)]
struct EventRow {
    id: i64,
    client_id: i64,
    action: String,
    reasons: serde_json::Value,
    details: serde_json::Value,
    created_at: DateTime<Utc>,
    admin_email: Option<String>,
}

pub async fn journal(
    db: &mut impl Db,
    filter: JournalFilter,
    client: Option<ClientId>,
    page: PageRequest,
) -> StoreResult<Page<JournalItem>> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT e.id, e.client_id, e.action, e.reasons, e.details, e.created_at, a.email AS admin_email \
         FROM antibot_events e LEFT JOIN admins a ON a.id = e.admin_id \
         WHERE (e.action = ANY($1) \
                OR (e.action = 'confirmed' AND COALESCE(e.details->>'target', 'trial') = ANY($5))) \
           AND ($2::bigint IS NULL OR e.client_id = $2) \
           AND ($3::bigint IS NULL OR e.id < $3) \
         ORDER BY e.id DESC LIMIT $4",
    )
    .bind(filter.actions())
    .bind(client.map(|c| c.0))
    .bind(page.after)
    .bind(page.fetch_limit())
    .bind(filter.confirmed_for())
    .fetch_all(db.conn())
    .await?;
    let items: Vec<JournalItem> = rows
        .into_iter()
        .filter_map(|row| {
            let action = Action::parse(&row.action)?;
            Some(JournalItem {
                id: row.id,
                at: row.created_at,
                client_id: ClientId(row.client_id),
                client: None,
                action,
                title: action.title_ru(),
                reasons: reasons(&signals_of(&row.reasons)),
                details: row.details,
                admin: row.admin_email,
            })
        })
        .collect();
    let mut page = Page::from_overfetch(items, page, |i| i.id);
    let ids: Vec<ClientId> = page.items.iter().map(|i| i.client_id).collect();
    let people = db.clients().people(&ids).await?;
    for item in &mut page.items {
        item.client = people.get(&item.client_id).cloned();
    }
    Ok(page)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarkItem {
    pub client_id: ClientId,
    pub client: Option<ClientPerson>,
    pub reasons: Vec<Reason>,
    pub marked_at: DateTime<Utc>,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub trial_denied: bool,
}

pub async fn open_marks(db: &mut impl Db, page: PageRequest) -> StoreResult<Page<MarkItem>> {
    let rows: Vec<(MarkRow, bool)> = sqlx::query_as::<_, (i64, serde_json::Value, serde_json::Value, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<i64>, bool)>(
        "SELECT m.client_id, m.reasons, m.details, m.marked_at, m.confirmed_at, m.cleared_at, m.cleared_by, \
                EXISTS (SELECT 1 FROM antibot_events e WHERE e.client_id = m.client_id AND e.action = 'trial_denied') \
         FROM antibot_marks m WHERE m.cleared_at IS NULL AND ($1::bigint IS NULL OR m.client_id < $1) \
         ORDER BY m.client_id DESC LIMIT $2",
    )
    .bind(page.after)
    .bind(page.fetch_limit())
    .fetch_all(db.conn())
    .await?
    .into_iter()
    .map(|(client_id, reasons, details, marked_at, confirmed_at, cleared_at, cleared_by, denied)| {
        (
            MarkRow {
                client_id,
                reasons,
                details,
                marked_at,
                confirmed_at,
                cleared_at,
                cleared_by,
            },
            denied,
        )
    })
    .collect();
    let items: Vec<MarkItem> = rows
        .into_iter()
        .map(|(row, denied)| MarkItem {
            client_id: ClientId(row.client_id),
            client: None,
            reasons: reasons(&signals_of(&row.reasons)),
            marked_at: row.marked_at,
            confirmed_at: row.confirmed_at,
            trial_denied: denied,
        })
        .collect();
    let mut page = Page::from_overfetch(items, page, |i| i.client_id.0);
    let ids: Vec<ClientId> = page.items.iter().map(|i| i.client_id).collect();
    let people = db.clients().people(&ids).await?;
    for item in &mut page.items {
        item.client = people.get(&item.client_id).cloned();
    }
    Ok(page)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub marked_open: i64,
    pub marked_week: i64,
    pub asked_week: i64,
    pub confirmed_week: i64,
    pub denied_week: i64,
    pub no_bonus_week: i64,
}

pub async fn summary(db: &mut impl Db) -> StoreResult<Summary> {
    let (marked_open, marked_week, asked_week, confirmed_week, denied_week, no_bonus_week): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM antibot_marks WHERE cleared_at IS NULL), \
                count(*) FILTER (WHERE action = 'marked'), \
                count(*) FILTER (WHERE action IN ('asked', 'roulette_asked', 'contest_asked', \
                                                  'promo_link_asked')), \
                count(*) FILTER (WHERE action = 'confirmed'), \
                count(*) FILTER (WHERE action IN ('trial_denied', 'promo_denied', 'roulette_denied', \
                                                  'contest_denied', 'contest_excluded', \
                                                  'promo_link_denied')), \
                count(*) FILTER (WHERE action = 'no_bonus') \
         FROM antibot_events WHERE created_at >= now() - interval '7 days'",
    )
    .fetch_one(db.conn())
    .await?;
    Ok(Summary {
        marked_open,
        marked_week,
        asked_week,
        confirmed_week,
        denied_week,
        no_bonus_week,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub created_at: DateTime<Utc>,
    pub web: bool,
}

pub async fn registration(
    db: &mut impl Db,
    client_id: ClientId,
) -> StoreResult<Option<Registration>> {
    let row: Option<(DateTime<Utc>, String)> = sqlx::query_as(
        "SELECT created_at, registration_source FROM clients WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?;
    Ok(row.map(|(created_at, source)| Registration {
        created_at,
        web: source == "web",
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramFacts {
    pub id: i64,
    pub username: Option<String>,
    pub premium: bool,
    pub linked_at: DateTime<Utc>,
}

pub async fn telegram(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<TelegramFacts>> {
    let row: Option<(String, serde_json::Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT value, meta, created_at FROM client_identities \
         WHERE client_id = $1 AND kind = 'telegram' ORDER BY id LIMIT 1",
    )
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?;
    Ok(row.and_then(|(value, meta, linked_at)| {
        let id = value.trim().parse::<i64>().ok()?;
        let username = meta
            .get("username")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        let premium = meta
            .get("is_premium")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Some(TelegramFacts {
            id,
            username,
            premium,
            linked_at,
        })
    }))
}

pub async fn telegram_history(
    db: &mut impl Db,
    before: DateTime<Utc>,
) -> StoreResult<(i64, Option<i64>)> {
    Ok(sqlx::query_as(
        "SELECT count(*), max(value::bigint) FROM client_identities \
         WHERE kind = 'telegram' AND created_at <= $1 AND value ~ '^[0-9]{1,18}$'",
    )
    .bind(before)
    .fetch_one(db.conn())
    .await?)
}

pub async fn telegram_neighbors(
    db: &mut impl Db,
    client_id: ClientId,
    telegram_id: i64,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    spread: i64,
) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(DISTINCT client_id) FROM client_identities \
         WHERE kind = 'telegram' AND client_id <> $1 AND created_at BETWEEN $2 AND $3 \
           AND value ~ '^[0-9]{1,18}$' AND abs(value::bigint - $4) <= $5",
    )
    .bind(client_id.0)
    .bind(from)
    .bind(to)
    .bind(telegram_id)
    .bind(spread)
    .fetch_one(db.conn())
    .await?)
}

pub async fn email(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT value FROM client_identities WHERE client_id = $1 AND kind = 'email' ORDER BY id LIMIT 1",
    )
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn email_twin(
    db: &mut impl Db,
    client_id: ClientId,
    email: &str,
) -> StoreResult<Option<ClientId>> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT client_id FROM client_identities \
         WHERE kind = 'email' AND client_id <> $1 AND antibot_email_key(value) = antibot_email_key($2) \
         ORDER BY client_id LIMIT 1",
    )
    .bind(client_id.0)
    .bind(email)
    .fetch_optional(db.conn())
    .await?;
    Ok(found.map(ClientId))
}

pub async fn first_ip(
    db: &mut impl Db,
    client_id: ClientId,
    until: DateTime<Utc>,
) -> StoreResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT ip FROM client_sessions WHERE client_id = $1 AND created_at <= $2 AND ip IS NOT NULL \
         ORDER BY created_at, id LIMIT 1",
    )
    .bind(client_id.0)
    .bind(until)
    .fetch_optional(db.conn())
    .await?
    .flatten())
}

pub async fn web_signups(
    db: &mut impl Db,
    client_id: ClientId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    grace: Duration,
) -> StoreResult<Vec<String>> {
    let rows: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT (SELECT s.ip FROM client_sessions s WHERE s.client_id = c.id \
                 AND s.created_at <= c.created_at + $4 AND s.ip IS NOT NULL ORDER BY s.created_at, s.id LIMIT 1) \
         FROM clients c WHERE c.registration_source = 'web' AND c.id <> $1 \
           AND c.created_at >= $2 AND c.created_at < $3 AND c.deleted_at IS NULL \
         ORDER BY c.id LIMIT 2000",
    )
    .bind(client_id.0)
    .bind(from)
    .bind(to)
    .bind(grace)
    .fetch_all(db.conn())
    .await?;
    Ok(rows.into_iter().flatten().collect())
}

pub async fn session_ips(
    db: &mut impl Db,
    client_id: ClientId,
    since: DateTime<Utc>,
) -> StoreResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT ip FROM client_sessions WHERE client_id = $1 AND ip IS NOT NULL \
           AND (created_at >= $2 OR last_seen_at >= $2) LIMIT 200",
    )
    .bind(client_id.0)
    .bind(since)
    .fetch_all(db.conn())
    .await?)
}

pub async fn referrer(
    db: &mut impl Db,
    client_id: ClientId,
) -> StoreResult<Option<(ClientId, DateTime<Utc>)>> {
    let row: Option<(i64, DateTime<Utc>)> =
        sqlx::query_as("SELECT referrer_id, created_at FROM referral_links WHERE client_id = $1")
            .bind(client_id.0)
            .fetch_optional(db.conn())
            .await?;
    Ok(row.map(|(id, at)| (ClientId(id), at)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitee {
    pub client_id: ClientId,
    pub telegram_id: Option<i64>,
    pub marked: bool,
}

pub async fn invitees_around(
    db: &mut impl Db,
    referrer: ClientId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> StoreResult<Vec<Invitee>> {
    let rows: Vec<(i64, Option<String>, bool)> = sqlx::query_as(
        "SELECT l.client_id, \
                (SELECT i.value FROM client_identities i WHERE i.client_id = l.client_id AND i.kind = 'telegram' \
                  ORDER BY i.id LIMIT 1), \
                EXISTS (SELECT 1 FROM antibot_marks m WHERE m.client_id = l.client_id AND m.cleared_at IS NULL \
                  AND m.reasons @> '[\"young_account\"]'::jsonb) \
         FROM referral_links l WHERE l.referrer_id = $1 AND l.created_at BETWEEN $2 AND $3 \
         ORDER BY l.client_id LIMIT 500",
    )
    .bind(referrer.0)
    .bind(from)
    .bind(to)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, telegram, marked)| Invitee {
            client_id: ClientId(id),
            telegram_id: telegram.and_then(|t| t.trim().parse().ok()),
            marked,
        })
        .collect())
}

pub async fn module_state(db: &mut impl Db) -> StoreResult<Option<(bool, serde_json::Value)>> {
    Ok(
        sqlx::query_as("SELECT enabled, config FROM module_states WHERE module = 'antibot'")
            .fetch_optional(db.conn())
            .await?,
    )
}
