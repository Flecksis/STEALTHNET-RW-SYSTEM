pub mod report;
pub mod spans;

#[cfg(test)]
mod tests;

use crate::catalog::installation_currency;
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{Module, ScheduledTask};
use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, FixedOffset, Offset, Utc};
use report::{MAX_MONTHS, Month, Params, Report, Source};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::module::ModuleId;
use sn_domain::settings::keys;
use sn_store::{Db, StoreResult};
use std::time::Duration;

const REFRESH_TASK: &str = "refresh";
const REFRESH_EVERY_SECS: u64 = 600;
const DEFAULT_MONTHS: i32 = 12;
const REBUILD_HOURS: i64 = 24;

pub async fn offset(db: &mut impl Db) -> StoreResult<FixedOffset> {
    let minutes = db
        .settings()
        .get(keys::REMNA_UTC_OFFSET)
        .await?
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .clamp(-1439, 1439);
    let seconds = i32::try_from(minutes * 60).unwrap_or(0);
    Ok(FixedOffset::east_opt(seconds).unwrap_or(Utc.fix()))
}

#[derive(Debug, Default, Deserialize)]
pub struct ReportQuery {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub months: Option<i32>,
    #[serde(default)]
    pub all: Option<bool>,
    #[serde(default)]
    pub source: Option<String>,
}

pub fn period(
    query: &ReportQuery,
    current: Month,
    first: Option<Month>,
) -> Result<(Month, Month), WebError> {
    let bad = |text: &str| WebError::rejected("analytics.period", text.to_string());
    let to = match query.to.as_deref() {
        Some(text) => Month::parse(text).ok_or_else(|| bad("Месяц указан неверно"))?,
        None => current,
    };
    if current.until(to) > 0 {
        return Err(bad("Период не может заканчиваться в будущем"));
    }
    let from = if query.all == Some(true) {
        first.filter(|f| f.until(to) >= 0).unwrap_or(to)
    } else if let Some(text) = query.from.as_deref() {
        Month::parse(text).ok_or_else(|| bad("Месяц указан неверно"))?
    } else {
        let months = query.months.unwrap_or(DEFAULT_MONTHS);
        if !(1..=MAX_MONTHS).contains(&months) {
            return Err(bad("Число месяцев — от 1 до 120"));
        }
        to.shift(1 - months)
    };
    if from.until(to) < 0 {
        return Err(bad("Начало периода позже конца"));
    }
    let from = if from.until(to) >= MAX_MONTHS {
        to.shift(1 - MAX_MONTHS)
    } else {
        from
    };
    Ok((from, to))
}

pub async fn report_for(
    ctx: &ModuleContext,
    query: &ReportQuery,
    now: DateTime<Utc>,
) -> Result<Report, WebError> {
    let currency = {
        let mut conn = ctx.store.conn().await?;
        installation_currency(&mut conn).await?
    };
    spans::refresh(&ctx.store, currency, None).await?;
    let (offset, first) = {
        let mut conn = ctx.store.conn().await?;
        let offset = offset(&mut conn).await?;
        let first = if query.all == Some(true) {
            report::first_month(&mut conn, offset).await?
        } else {
            None
        };
        (offset, first)
    };
    let current = Month::of(now, offset);
    let (from, to) = period(query, current, first)?;
    let titles = ctx.core.payment_titles();
    let params = Params {
        currency,
        offset,
        from,
        to,
        now,
        source: Source::from_query(query.source.as_deref()),
    };
    Ok(report::build(&ctx.store, &params, &titles).await?)
}

async fn report_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ReportQuery>,
) -> Result<Json<Report>, WebError> {
    admin.require(Permission::Money)?;
    let now = {
        let mut conn = ctx.store.conn().await?;
        crate::catalog::db_now(&mut conn).await?
    };
    Ok(Json(report_for(&ctx, &query, now).await?))
}

pub struct AnalyticsModule;

#[async_trait]
impl Module for AnalyticsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Analytics
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(
            Router::new()
                .route("/report", get(report_route))
                .with_state(ctx),
        )
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: REFRESH_TASK,
            every: Duration::from_secs(REFRESH_EVERY_SECS),
        }]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        match task {
            REFRESH_TASK => {
                let currency = {
                    let mut conn = ctx.store.conn().await?;
                    installation_currency(&mut conn).await?
                };
                let done = spans::refresh(
                    &ctx.store,
                    currency,
                    Some(chrono::Duration::hours(REBUILD_HOURS)),
                )
                .await?;
                if done.full {
                    tracing::info!(
                        clients = done.clients,
                        "аналитика: периоды подписок посчитаны заново"
                    );
                }
                Ok(())
            }
            other => Err(ModuleError::Invalid(format!("у модуля нет задачи {other}"))),
        }
    }
}
