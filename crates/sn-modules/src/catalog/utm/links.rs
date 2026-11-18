use crate::catalog::analytics::query::{Filters, bound_after, local_day, midnight};
use crate::catalog::analytics::sources::{SourceStat, stats};
use crate::catalog::analytics::{self, offset};
use crate::catalog::installation_currency;
use crate::catalog::start_param::{MAX_LEN, Utm};
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::settings::keys;
use sn_store::{Db, StoreResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct UtmLink {
    pub id: i64,
    pub title: String,
    pub source: String,
    pub medium: Option<String>,
    pub campaign: Option<String>,
    pub content: Option<String>,
    pub term: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl UtmLink {
    pub fn utm(&self) -> Utm {
        Utm {
            source: Some(self.source.clone()),
            medium: self.medium.clone(),
            campaign: self.campaign.clone(),
            content: self.content.clone(),
            term: self.term.clone(),
        }
    }

    fn matches(&self, s: &SourceStat) -> bool {
        s.source_type == "utm"
            && s.source_key == self.source
            && s.medium == self.medium.clone().unwrap_or_default()
            && s.campaign == self.campaign.clone().unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkRow {
    pub link: UtmLink,
    pub param: String,
    pub stats: SourceStat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinksPage {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub bot: Option<String>,
    pub site: Option<String>,
    pub preparing: bool,
    pub trials: bool,
    pub links: Vec<LinkRow>,
    pub stray: Vec<SourceStat>,
}

#[derive(Debug, Default, Deserialize)]
pub struct LinksQuery {
    #[serde(default)]
    pub from: Option<NaiveDate>,
    #[serde(default)]
    pub to: Option<NaiveDate>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct LinkInput {
    pub title: String,
    pub source: String,
    #[serde(default)]
    pub medium: Option<String>,
    #[serde(default)]
    pub campaign: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub term: Option<String>,
}

fn bad(code: &'static str, text: &str) -> WebError {
    WebError::rejected(code, text.to_string())
}

pub fn check(input: &LinkInput) -> Result<(String, Utm), WebError> {
    let title = input.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 100 {
        return Err(bad("utm.title", "Название — от 1 до 100 знаков"));
    }
    let field = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let parts = [
        (
            "источник",
            Some(input.source.trim().to_string()).filter(|v| !v.is_empty()),
        ),
        ("канал", field(&input.medium)),
        ("кампания", field(&input.campaign)),
        ("объявление", field(&input.content)),
        ("ключевое слово", field(&input.term)),
    ];
    if parts[0].1.is_none() {
        return Err(bad("utm.source", "Укажите источник"));
    }
    for (name, value) in &parts {
        if let Some(v) = value
            && !crate::catalog::start_param::tag_value(v)
        {
            return Err(bad(
                "utm.value",
                &format!(
                    "В поле «{name}» можно латиницу, цифры, «-» и «_», без двух «_» подряд, до 64 знаков"
                ),
            ));
        }
    }
    let utm = Utm {
        source: parts[0].1.clone(),
        medium: parts[1].1.clone(),
        campaign: parts[2].1.clone(),
        content: parts[3].1.clone(),
        term: parts[4].1.clone(),
    };
    if utm.to_param().len() > MAX_LEN {
        return Err(bad(
            "utm.length",
            "Ссылка для Telegram получится длиннее 64 знаков: сократите кампанию или уберите объявление",
        ));
    }
    Ok((title, utm))
}

pub async fn list(db: &mut impl Db) -> StoreResult<Vec<UtmLink>> {
    Ok(sqlx::query_as(
        "SELECT id, title, source, medium, campaign, content, term, created_at FROM utm_links \
         WHERE archived_at IS NULL ORDER BY id DESC",
    )
    .fetch_all(db.conn())
    .await?)
}

pub async fn by_tags(db: &mut impl Db, utm: &Utm) -> StoreResult<Option<UtmLink>> {
    Ok(sqlx::query_as(
        "SELECT id, title, source, medium, campaign, content, term, created_at FROM utm_links \
         WHERE archived_at IS NULL AND source = $1 AND COALESCE(medium, '') = $2 AND COALESCE(campaign, '') = $3 \
         ORDER BY (COALESCE(content, '') = $4 AND COALESCE(term, '') = $5) DESC, id DESC LIMIT 1",
    )
    .bind(utm.source.clone().unwrap_or_default())
    .bind(utm.medium.clone().unwrap_or_default())
    .bind(utm.campaign.clone().unwrap_or_default())
    .bind(utm.content.clone().unwrap_or_default())
    .bind(utm.term.clone().unwrap_or_default())
    .fetch_optional(db.conn())
    .await?)
}

async fn setting_text(db: &mut impl Db, key: &str) -> StoreResult<Option<String>> {
    Ok(db
        .settings()
        .get(key)
        .await?
        .and_then(|v| v.as_str().map(str::to_string))
        .map(|v| {
            v.trim()
                .trim_start_matches('@')
                .trim_end_matches('/')
                .to_string()
        })
        .filter(|v| !v.is_empty()))
}

pub async fn page(
    ctx: &ModuleContext,
    q: &LinksQuery,
    now: DateTime<Utc>,
) -> Result<LinksPage, WebError> {
    let (currency, offset, links, bot, site) = {
        let mut conn = ctx.store.conn().await?;
        let currency = installation_currency(&mut conn).await?;
        let offset = offset(&mut conn).await?;
        let links = list(&mut conn).await?;
        let bot = setting_text(&mut conn, keys::BOT_USERNAME).await?;
        let site = setting_text(&mut conn, keys::PUBLIC_URL).await?;
        (currency, offset, links, bot, site)
    };
    let today = local_day(now, offset);
    let to = q.to.unwrap_or(today).min(today);
    let from = q
        .from
        .unwrap_or_else(|| to.with_day(1).unwrap_or(to))
        .min(to);
    let (ready, trial_on) = {
        let mut conn = ctx.store.conn().await?;
        (
            analytics::ready(&mut conn, currency).await?,
            analytics::sources::trial_on(&mut conn).await?,
        )
    };
    let mut rows: Vec<LinkRow> = links
        .into_iter()
        .map(|link| LinkRow {
            param: link.utm().to_param(),
            stats: SourceStat {
                source_type: "utm".into(),
                source_key: link.source.clone(),
                medium: link.medium.clone().unwrap_or_default(),
                campaign: link.campaign.clone().unwrap_or_default(),
                ..SourceStat::default()
            },
            link,
        })
        .collect();
    if !ready {
        analytics::build_in_background(ctx.store.clone(), currency);
        return Ok(LinksPage {
            from,
            to,
            bot,
            site,
            preparing: true,
            trials: trial_on,
            links: rows,
            stray: Vec::new(),
        });
    }
    analytics::spans::refresh_if_stale(&ctx.store, currency).await?;
    let found = stats(
        &ctx.store,
        midnight(from, offset),
        bound_after(to, offset),
        now,
        &Filters::default(),
    )
    .await?;
    let mut stray = Vec::new();
    for s in found.into_iter().filter(|s| s.source_type == "utm") {
        match rows.iter_mut().find(|r| r.link.matches(&s)) {
            Some(r) => {
                let mut merged = s.clone();
                merged.clicks += r.stats.clicks;
                r.stats = merged;
            }
            None => stray.push(s),
        }
    }
    let trials =
        trial_on || rows.iter().any(|r| r.stats.trials > 0) || stray.iter().any(|s| s.trials > 0);
    Ok(LinksPage {
        from,
        to,
        bot,
        site,
        preparing: false,
        trials,
        links: rows,
        stray,
    })
}

pub async fn links_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(q): Query<LinksQuery>,
) -> Result<Json<LinksPage>, WebError> {
    admin.require(Permission::Marketing)?;
    let now = {
        let mut conn = ctx.store.conn().await?;
        crate::catalog::db_now(&mut conn).await?
    };
    Ok(Json(page(&ctx, &q, now).await?))
}

pub async fn create_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(input): Json<LinkInput>,
) -> Result<Json<UtmLink>, WebError> {
    admin.require(Permission::Marketing)?;
    let (title, utm) = check(&input)?;
    let mut conn = ctx.store.conn().await?;
    if by_tags(&mut conn, &utm)
        .await?
        .is_some_and(|l| l.content == utm.content && l.term == utm.term)
    {
        return Err(bad("utm.exists", "Ссылка с такими метками уже есть"));
    }
    let link: UtmLink = sqlx::query_as(
        "INSERT INTO utm_links (title, source, medium, campaign, content, term, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id, title, source, medium, campaign, content, term, created_at",
    )
    .bind(&title)
    .bind(&utm.source)
    .bind(&utm.medium)
    .bind(&utm.campaign)
    .bind(&utm.content)
    .bind(&utm.term)
    .bind(admin.0.admin_id.0)
    .fetch_one(conn.conn())
    .await
    .map_err(sn_store::StoreError::from)?;
    Ok(Json(link))
}

pub async fn update_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
    Json(input): Json<LinkInput>,
) -> Result<Json<UtmLink>, WebError> {
    admin.require(Permission::Marketing)?;
    let (title, utm) = check(&input)?;
    let mut conn = ctx.store.conn().await?;
    if let Some(other) = by_tags(&mut conn, &utm).await?
        && other.id != id
        && other.content == utm.content
        && other.term == utm.term
    {
        return Err(bad("utm.exists", "Ссылка с такими метками уже есть"));
    }
    let link: Option<UtmLink> = sqlx::query_as(
        "UPDATE utm_links SET title = $2, source = $3, medium = $4, campaign = $5, content = $6, term = $7 \
         WHERE id = $1 AND archived_at IS NULL \
         RETURNING id, title, source, medium, campaign, content, term, created_at",
    )
    .bind(id)
    .bind(&title)
    .bind(&utm.source)
    .bind(&utm.medium)
    .bind(&utm.campaign)
    .bind(&utm.content)
    .bind(&utm.term)
    .fetch_optional(conn.conn())
    .await
    .map_err(sn_store::StoreError::from)?;
    link.map(Json).ok_or_else(|| {
        WebError::new(
            axum::http::StatusCode::NOT_FOUND,
            "utm.not_found",
            "Ссылки нет",
        )
    })
}

pub async fn archive_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::Marketing)?;
    let mut conn = ctx.store.conn().await?;
    let done = sqlx::query(
        "UPDATE utm_links SET archived_at = now() WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(id)
    .execute(conn.conn())
    .await
    .map_err(sn_store::StoreError::from)?;
    Ok(Json(
        serde_json::json!({ "archived": done.rows_affected() > 0 }),
    ))
}
