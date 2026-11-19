use crate::telegram::{TelegramApi, TelegramError, Update, WebhookInfo, parse_update};
use crate::{BotConfig, BotError, stars};
use chrono::{DateTime, Utc};
use sn_domain::settings::FallbackReason;
use sn_services::bot_webhook::{
    self, ALLOWED_UPDATES, BROKEN_ACTION, MAX_CONNECTIONS, RESTORED_ACTION, clip_error, host_of,
};
use sn_services::{ServiceError, Services};
use sn_store::StoreError;
use sqlx::{Connection, PgConnection};
use std::time::Duration;

const LOCK_KEY: i32 = 0x5342_4F54;
pub const BATCH: i64 = 100;

pub struct Lease {
    conn: Option<PgConnection>,
}

impl Lease {
    pub async fn acquire(services: &Services) -> Result<Lease, BotError> {
        let mut conn = services
            .store()
            .pool()
            .acquire()
            .await
            .map_err(StoreError::from)?
            .detach();
        let locked: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_lock($1, hashtext(current_schema()))")
                .bind(LOCK_KEY)
                .fetch_one(&mut conn)
                .await
                .map_err(StoreError::from)?;
        if !locked {
            let _ = conn.close().await;
            return Err(BotError::Duplicate);
        }
        Ok(Lease { conn: Some(conn) })
    }

    pub async fn alive(&mut self) -> Result<(), BotError> {
        match self.conn.as_mut() {
            Some(conn) => {
                sqlx::query("SELECT 1")
                    .execute(conn)
                    .await
                    .map_err(StoreError::from)?;
                Ok(())
            }
            None => Err(BotError::Duplicate),
        }
    }

    pub async fn release(mut self) {
        if let Some(mut conn) = self.conn.take() {
            let _ = sqlx::query("SELECT pg_advisory_unlock($1, hashtext(current_schema()))")
                .bind(LOCK_KEY)
                .execute(&mut conn)
                .await;
            let _ = conn.close().await;
        }
    }
}

pub enum Setup {
    Ready(String),
    Rejected(String),
}

fn up_to_date(hook: &WebhookInfo, url: &str) -> bool {
    hook.url == url
        && hook.max_connections == Some(MAX_CONNECTIONS)
        && hook.allowed_updates.as_ref().is_some_and(|list| {
            let mut have: Vec<&str> = list.iter().map(String::as_str).collect();
            let mut want: Vec<&str> = ALLOWED_UPDATES.to_vec();
            have.sort_unstable();
            want.sort_unstable();
            have == want
        })
}

pub async fn register(
    services: &Services,
    api: &TelegramApi,
    bot: i64,
    hook: Option<&WebhookInfo>,
) -> Result<Setup, BotError> {
    let ops = services.bot_webhook();
    let mut snapshot = ops.snapshot().await?.unwrap_or_default();
    if services.bot_webhook_keys().is_none() || snapshot.bot_id.is_some_and(|id| id != bot) {
        ops.rotate_keys().await?;
    }
    let Some((url, keys)) = ops.target() else {
        return Err(BotError::Service(ServiceError::Internal(
            "адрес вебхука не собран: нет адреса панели с https или ключа".into(),
        )));
    };
    let fresh = hook.is_some_and(|h| up_to_date(h, &url));
    if !fresh {
        match api
            .set_webhook(&url, &keys.header_token(), MAX_CONNECTIONS)
            .await
        {
            Ok(_) => {
                tracing::info!(host = ?host_of(&url), "вебхук бота выставлен");
            }
            Err(TelegramError::Api {
                code: 400,
                description,
                ..
            }) => return Ok(Setup::Rejected(description)),
            Err(err) => return Err(err.into()),
        }
        snapshot.registered_at = Some(services.now());
        snapshot.broken_since = None;
        snapshot.reported = false;
        snapshot.last_error = None;
        snapshot.last_error_at = None;
        snapshot.pending = None;
    }
    snapshot.bot_id = Some(bot);
    snapshot.host = host_of(&url);
    snapshot.max_connections = Some(MAX_CONNECTIONS);
    ops.save_snapshot(&snapshot).await?;
    Ok(Setup::Ready(url))
}

pub async fn fall_back_rejected(services: &Services, description: String) -> Result<(), BotError> {
    tracing::warn!(error = %description, "Telegram не принял адрес вебхука, бот возвращается к опросу");
    let host = services
        .bot_webhook()
        .target()
        .and_then(|(url, _)| host_of(&url));
    services
        .bot_webhook()
        .fall_back(FallbackReason::Rejected, Some(description), host, None)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Fine,
    Unknown,
    Lost,
    Foreign(Option<String>),
    FellBack,
}

fn minutes_up(span: chrono::Duration) -> i64 {
    (span.num_seconds() + 59).div_euclid(60).max(1)
}

fn chrono_span(span: Duration) -> chrono::Duration {
    chrono::Duration::from_std(span).unwrap_or_else(|_| chrono::Duration::zero())
}

pub async fn check(
    services: &Services,
    api: &TelegramApi,
    url: &str,
    config: &BotConfig,
) -> Result<Verdict, BotError> {
    let info = match api.get_webhook_info().await {
        Ok(info) => info,
        Err(
            err @ TelegramError::Api {
                code: 401 | 404, ..
            },
        ) => return Err(err.into()),
        Err(err) => {
            tracing::debug!(error = %err, "сведения о вебхуке бота не получены");
            return Ok(Verdict::Unknown);
        }
    };
    let ops = services.bot_webhook();
    if info.url != url {
        if info.url.is_empty() || ops.is_ours(&info.url).await? {
            return Ok(Verdict::Lost);
        }
        return Ok(Verdict::Foreign(host_of(&info.url)));
    }
    let now = services.now();
    let mut snapshot = ops.snapshot().await?.unwrap_or_default();
    let last_error_at: Option<DateTime<Utc>> = info
        .last_error_date
        .and_then(|ts| DateTime::from_timestamp(ts, 0));
    let received = ops.last_received().await?;
    let window = chrono_span(config.webhook_fallback_after + config.webhook_check_every)
        .max(chrono::Duration::minutes(3));
    let failing = info.pending_update_count > 0
        && last_error_at.is_some_and(|at| now - at <= window && received.is_none_or(|r| r < at));
    let error = info
        .last_error_message
        .as_deref()
        .map(clip_error)
        .filter(|e| !e.is_empty());
    let host = host_of(url);
    let alert_after = chrono_span(config.webhook_alert_after);
    let fallback_after = chrono_span(config.webhook_fallback_after);
    let mut verdict = Verdict::Fine;
    if failing {
        let since = *snapshot.broken_since.get_or_insert(now);
        let broken_for = now - since;
        let minutes = broken_for.num_minutes().max(1);
        if !snapshot.reported && broken_for >= alert_after {
            ops.incident(
                BROKEN_ACTION,
                serde_json::json!({
                    "host": host,
                    "error": error,
                    "pending": info.pending_update_count,
                    "minutes": minutes,
                    "fallback_in": minutes_up(fallback_after - broken_for),
                }),
            )
            .await?;
            snapshot.reported = true;
        }
        if broken_for >= fallback_after {
            match api.delete_webhook().await {
                Ok(_) => {
                    tracing::warn!(
                        minutes,
                        "Telegram долго не доставляет обновления на вебхук, бот возвращается к опросу"
                    );
                    ops.fall_back(
                        FallbackReason::Undelivered,
                        error.clone(),
                        host.clone(),
                        Some(minutes),
                    )
                    .await?;
                    snapshot.broken_since = None;
                    snapshot.reported = false;
                    verdict = Verdict::FellBack;
                }
                Err(err) => {
                    tracing::warn!(error = %err, "вебхук бота не снят, к опросу пока не вернулись");
                }
            }
        }
    } else if snapshot.broken_since.take().is_some() {
        if snapshot.reported {
            ops.incident(RESTORED_ACTION, serde_json::json!({ "host": host }))
                .await?;
        }
        snapshot.reported = false;
    }
    snapshot.checked_at = Some(now);
    snapshot.pending = Some(info.pending_update_count);
    snapshot.last_error_at = last_error_at;
    snapshot.last_error = error;
    ops.save_snapshot(&snapshot).await?;
    Ok(verdict)
}

pub async fn forget_previous(services: &Services, api: &TelegramApi) {
    let ops = services.bot_webhook();
    let job = async {
        let hook = api.get_webhook_info().await.ok()?;
        if hook.url.is_empty() || !ops.is_ours(&hook.url).await.ok()? {
            return None;
        }
        api.delete_webhook().await.ok()
    };
    if let Ok(Some(_)) = tokio::time::timeout(Duration::from_secs(5), job).await {
        tracing::info!("вебхук прежнего бота снят");
    }
}

pub async fn take_batch(services: &Services, bot: i64) -> Result<Vec<Update>, BotError> {
    let mut tx = services.store().begin().await?;
    let rows = bot_webhook::take(&mut tx, bot, BATCH).await?;
    if rows.is_empty() {
        tx.rollback().await?;
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = rows.iter().map(|row| row.update_id).collect();
    let updates: Vec<Update> = rows
        .into_iter()
        .filter_map(|row| {
            parse_update(row.payload).or(Some(Update {
                update_id: row.update_id,
                message: None,
                callback_query: None,
                pre_checkout_query: None,
                widget_query: None,
                repeat: false,
            }))
        })
        .collect();
    let payments = stars::from_updates(&updates);
    stars::record_in(&mut tx, services, &payments).await?;
    bot_webhook::mark_taken(&mut tx, bot, &ids).await?;
    tx.commit().await?;
    Ok(updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn минуты_до_возврата_к_опросу_округляются_вверх() {
        let span = |ms: i64| chrono::Duration::milliseconds(ms);
        assert_eq!(minutes_up(span(299_900)), 5);
        assert_eq!(minutes_up(span(300_000)), 5);
        assert_eq!(minutes_up(span(300_001)), 5);
        assert_eq!(minutes_up(span(301_000)), 6);
        assert_eq!(minutes_up(span(0)), 1);
        assert_eq!(minutes_up(span(-5_000)), 1);
    }
}
