use super::labels;
use super::metric::{Point, points};
use super::query::starts;
use super::tab::Ctx;
use crate::context::CoreApi;
use crate::error::ModuleError;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use sn_domain::money::Currency;
use sn_store::{Db, Store, StoreResult};

const KEEP_SAMPLES_DAYS: i64 = 400;

const SNAPSHOT: &str = "WITH active AS ( \
       SELECT s.* FROM subscriptions s WHERE s.ended_at IS NULL AND COALESCE(s.status, 'active') IN ('active', 'limited') \
         AND (s.expire_at IS NULL OR s.expire_at > now())) \
     INSERT INTO analytics_snapshots (day, key, value) \
     SELECT $1, k, v FROM ( \
       SELECT 'subs.' || kind AS k, count(*)::bigint AS v FROM active GROUP BY kind \
       UNION ALL SELECT 'multi', count(*) FROM (SELECT client_id FROM active GROUP BY client_id HAVING count(*) > 1) m \
       UNION ALL SELECT 'devices.limit', count(*) FROM active WHERE hwid_limit > 0 AND COALESCE(devices_count, 0) >= hwid_limit \
       UNION ALL SELECT 'traffic.near', count(*) FROM active WHERE traffic_limit_bytes > 0 \
         AND COALESCE(traffic_used_bytes, 0) * 10 >= traffic_limit_bytes * 9 \
       UNION ALL SELECT 'never', count(*) FROM active WHERE first_connected_at IS NULL \
       UNION ALL SELECT 'autorenew.on', count(*) FROM active WHERE kind = 'paid' AND autorenew_enabled \
       UNION ALL SELECT 'paying', count(DISTINCT client_id) FROM active WHERE kind = 'paid' \
       UNION ALL SELECT 'balance.main', COALESCE(sum(amount_minor), 0)::bigint FROM client_balances \
         WHERE wallet = 'main' AND currency = $2 \
       UNION ALL SELECT 'balance.referral', COALESCE(sum(amount_minor), 0)::bigint FROM client_balances \
         WHERE wallet = 'referral' AND currency = $2 \
       UNION ALL SELECT 'blocked', count(*) FROM clients WHERE status = 'blocked' AND deleted_at IS NULL \
       UNION ALL SELECT 'bot_blocked', count(*) FROM clients WHERE bot_unreachable AND deleted_at IS NULL \
     ) x ON CONFLICT (day, key) DO NOTHING";

pub async fn daily(db: &mut impl Db, day: NaiveDate, currency: Currency) -> StoreResult<bool> {
    let done: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM analytics_snapshots WHERE day = $1)")
            .bind(day)
            .fetch_one(db.conn())
            .await?;
    if done {
        return Ok(false);
    }
    sqlx::query(SNAPSHOT)
        .bind(day)
        .bind(currency.code())
        .execute(db.conn())
        .await?;
    Ok(true)
}

pub async fn series(c: &Ctx<'_>, keys: &[&str]) -> StoreResult<(Vec<Point>, Option<String>)> {
    let keys: Vec<String> = keys.iter().map(|k| (*k).to_string()).collect();
    let mut conn = c.store.conn().await?;
    let found: Vec<(NaiveDate, i64)> = sqlx::query_as(
        "SELECT day, sum(value)::bigint FROM analytics_snapshots \
         WHERE key = ANY($1) AND day >= $2 AND day <= $3 GROUP BY day ORDER BY day",
    )
    .bind(&keys)
    .bind(c.range.dates.from)
    .bind(c.range.dates.to)
    .fetch_all(conn.conn())
    .await?;
    let first: Option<NaiveDate> =
        sqlx::query_scalar("SELECT min(day) FROM analytics_snapshots WHERE key = ANY($1)")
            .bind(&keys)
            .fetch_one(conn.conn())
            .await?;
    let hint = match first {
        Some(day) if day > c.range.dates.from => Some(format!(
            "По дням — с {}, раньше снимков не было",
            labels::date_text(day)
        )),
        None => Some("По дням — с первого ночного снимка после обновления".into()),
        _ => None,
    };
    if found.is_empty() {
        return Ok((Vec::new(), hint));
    }
    let labels = starts(c.range.dates, c.range.step);
    let mut values = vec![0.0; labels.len()];
    let mut seen = vec![false; labels.len()];
    for (day, value) in found {
        if let Some(i) = labels.iter().rposition(|l| *l <= day) {
            values[i] = value as f64;
            seen[i] = true;
        }
    }
    let kept: Vec<(NaiveDate, f64)> = labels
        .into_iter()
        .zip(values)
        .zip(seen)
        .filter(|(_, s)| *s)
        .map(|(p, _)| p)
        .collect();
    let (l, v): (Vec<NaiveDate>, Vec<f64>) = kept.into_iter().unzip();
    Ok((points(&l, &v), hint))
}

pub async fn sample_nodes(
    store: &Store,
    core: &dyn CoreApi,
    now: DateTime<Utc>,
) -> Result<(), ModuleError> {
    let nodes = core.nodes_online().await?;
    let mut conn = store.conn().await?;
    let at = now - Duration::seconds(now.timestamp() % 60);
    for n in nodes {
        sqlx::query(
            "INSERT INTO analytics_node_samples (at, node_uuid, name, online) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (at, node_uuid) DO UPDATE SET online = EXCLUDED.online, name = EXCLUDED.name",
        )
        .bind(at)
        .bind(n.uuid)
        .bind(&n.name)
        .bind(i32::try_from(n.online.max(0)).unwrap_or(i32::MAX))
        .execute(conn.conn())
        .await
        .map_err(sn_store::StoreError::from)?;
    }
    sqlx::query("DELETE FROM analytics_node_samples WHERE at < $1")
        .bind(now - Duration::days(KEEP_SAMPLES_DAYS))
        .execute(conn.conn())
        .await
        .map_err(sn_store::StoreError::from)?;
    Ok(())
}
