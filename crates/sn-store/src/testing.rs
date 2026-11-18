use crate::db::{Store, StoreOptions};
use chrono::{DateTime, Utc};
use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection};
use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const TEST_DATABASE_ENV: &str = "SN_TEST_DATABASE_URL";
pub const SCHEMA_PREFIX: &str = "sn_t_";
pub const STALE_AFTER_SECS: i64 = 6 * 3600;
pub const UNDATED_STALE_AFTER_SECS: i64 = 24 * 3600;
pub const DAY: Duration = Duration::from_secs(24 * 3600);
pub const WINDOW_ROOM: Duration = Duration::from_secs(60);

const SEEN_NOTE: &str = "sn-test seen ";
const SWEEP_LOCK: i64 = 0x736e_7465_7374;
const CLEANUP_WAIT: Duration = Duration::from_secs(90);

const TERMINATE: &str = "SELECT pg_terminate_backend(a.pid) FROM pg_stat_activity a \
     WHERE a.pid <> pg_backend_pid() AND a.datname = current_database() \
       AND a.usename = current_user AND a.backend_type = 'client backend' \
       AND (a.application_name = $1 OR a.pid IN ( \
           SELECT l.pid FROM pg_locks l JOIN pg_class c ON c.oid = l.relation \
           WHERE l.database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
             AND c.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = $1)))";

static SWEPT: AtomicBool = AtomicBool::new(false);

pub struct TestDb {
    pub store: Store,
    schema: String,
    url: String,
    removed: bool,
}

pub fn test_database_url() -> Option<String> {
    std::env::var(TEST_DATABASE_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
}

pub async fn test_store() -> Option<TestDb> {
    let db = empty_test_store().await?;
    db.store.migrate().await.expect("миграции не применились");
    Some(db)
}

pub async fn empty_test_store() -> Option<TestDb> {
    let url = test_database_url()?;
    let database = PgConnectOptions::from_str(&url)
        .ok()
        .and_then(|o| o.get_database().map(str::to_string))
        .unwrap_or_default();
    assert!(
        database.starts_with("sn_test"),
        "{TEST_DATABASE_ENV} должен указывать на базу с именем sn_test*, сейчас: {database}"
    );
    let schema = schema_name(Utc::now().timestamp());
    let store = Store::connect_lazy(
        &url,
        StoreOptions {
            max_connections: 4,
            search_path: Some(schema.clone()),
            application_name: schema.clone(),
            ..StoreOptions::default()
        },
    )
    .expect("строка подключения к тестовой базе не читается");
    let db = TestDb {
        store,
        schema,
        url,
        removed: false,
    };
    sweep_once(&db.url);
    let mut admin = admin(&db.url, "10s")
        .await
        .expect("тестовая база недоступна");
    sqlx::query(&format!("CREATE SCHEMA {}", quoted(&db.schema)))
        .execute(&mut admin)
        .await
        .expect("не удалось создать схему для теста");
    let _ = admin.close().await;
    Some(db)
}

pub async fn wait_for_window_room(store: &Store, window: Duration) {
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(store.pool())
        .await
        .expect("тестовая база недоступна");
    if let Some(wait) = window_wait(now, window, WINDOW_ROOM) {
        tokio::time::sleep(wait).await;
    }
}

fn window_wait(now: DateTime<Utc>, window: Duration, needed: Duration) -> Option<Duration> {
    let window = i64::try_from(window.as_micros()).ok().filter(|w| *w > 0)?;
    let into = now.timestamp_micros().rem_euclid(window);
    let left = Duration::from_micros(u64::try_from(window - into).ok()?);
    (left < needed).then(|| left + Duration::from_millis(200))
}

impl TestDb {
    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub async fn cleanup(mut self) {
        self.store.close().await;
        self.removed = remove(&self.url, &self.schema).await;
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        if self.removed {
            return;
        }
        let url = self.url.clone();
        let schema = self.schema.clone();
        if let Ok(worker) = std::thread::Builder::new()
            .name("sn-test-cleanup".into())
            .spawn(move || on_own_runtime(remove(&url, &schema)))
        {
            let _ = worker.join();
        }
    }
}

fn schema_name(born: i64) -> String {
    format!(
        "{SCHEMA_PREFIX}{born}_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    )
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn on_own_runtime<F: Future>(work: F) -> Option<F::Output> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async { tokio::time::timeout(CLEANUP_WAIT, work).await.ok() })
}

async fn admin(url: &str, lock_timeout: &str) -> Result<PgConnection, sqlx::Error> {
    let options = PgConnectOptions::from_str(url)?
        .application_name("sn-test-admin")
        .options([
            ("lock_timeout", lock_timeout),
            ("statement_timeout", "120s"),
        ]);
    PgConnection::connect_with(&options).await
}

async fn remove(url: &str, schema: &str) -> bool {
    let Ok(mut admin) = admin(url, "10s").await else {
        return false;
    };
    let mut removed = false;
    for _ in 0..3 {
        let _ = sqlx::query(TERMINATE)
            .bind(schema)
            .execute(&mut admin)
            .await;
        if sqlx::query(&format!("DROP SCHEMA IF EXISTS {} CASCADE", quoted(schema)))
            .execute(&mut admin)
            .await
            .is_ok()
        {
            removed = true;
            break;
        }
    }
    let _ = admin.close().await;
    removed
}

fn sweep_once(url: &str) {
    if SWEPT.swap(true, Ordering::SeqCst) {
        return;
    }
    let url = url.to_owned();
    let _ = std::thread::Builder::new()
        .name("sn-test-sweep".into())
        .spawn(move || on_own_runtime(sweep(&url)));
}

async fn sweep(url: &str) {
    let Ok(mut admin) = admin(url, "2s").await else {
        return;
    };
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(SWEEP_LOCK)
        .fetch_one(&mut admin)
        .await
        .unwrap_or(false);
    if locked {
        sweep_stale(&mut admin, Utc::now().timestamp()).await;
    }
    let _ = admin.close().await;
}

async fn sweep_stale(admin: &mut PgConnection, now: i64) {
    let found: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT n.nspname::text, obj_description(n.oid, 'pg_namespace') \
         FROM pg_namespace n WHERE n.nspname LIKE 'sn\\_t\\_%' ORDER BY n.nspname",
    )
    .fetch_all(&mut *admin)
    .await
    .unwrap_or_default();
    for (schema, note) in found {
        let statement = match verdict(&schema, note.as_deref(), now) {
            Verdict::Keep => continue,
            Verdict::Mark => format!(
                "COMMENT ON SCHEMA {} IS '{SEEN_NOTE}{now}'",
                quoted(&schema)
            ),
            Verdict::Remove => format!("DROP SCHEMA IF EXISTS {} CASCADE", quoted(&schema)),
        };
        let _ = sqlx::query(&statement).execute(&mut *admin).await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Keep,
    Mark,
    Remove,
}

fn verdict(schema: &str, note: Option<&str>, now: i64) -> Verdict {
    let Some(rest) = schema.strip_prefix(SCHEMA_PREFIX) else {
        return Verdict::Keep;
    };
    if let Some(born) = born_at(rest) {
        return if now - born > STALE_AFTER_SECS {
            Verdict::Remove
        } else {
            Verdict::Keep
        };
    }
    let seen = note
        .and_then(|n| n.strip_prefix(SEEN_NOTE))
        .and_then(|at| at.trim().parse::<i64>().ok());
    match seen {
        Some(seen) if now - seen > UNDATED_STALE_AFTER_SECS => Verdict::Remove,
        Some(_) => Verdict::Keep,
        None => Verdict::Mark,
    }
}

fn born_at(rest: &str) -> Option<i64> {
    let (born, tail) = rest.split_once('_')?;
    if born.is_empty() || tail.is_empty() || !born.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    born.parse().ok()
}

#[cfg(test)]
mod tests;
