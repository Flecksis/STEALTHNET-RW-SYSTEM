use super::archive::{self, ArchiveError, ReadInfo, Visitor, stop};
use super::dump::{self, Schema, columns_sql, qualified, quote};
use super::format::{FORMAT, FORMAT_VERSION, Manifest};
use super::sealed::{self, KEEP_PREFIX, Outcome, Plan};
use crate::error::{ServiceError, ServiceResult};
use sn_store::SecretBox;
use sqlx::PgConnection;
use std::collections::BTreeSet;
use std::path::PathBuf;
use tokio::sync::mpsc;

pub const RESTORE_SKIPPED: [&str; 3] = ["_sqlx_migrations", "backup_runs", "bot_updates"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Load { table: usize, tables: usize },
    Migrate,
    Apply,
    Keys,
}

pub struct CurrentKey {
    pub raw: Vec<u8>,
    pub check: String,
    pub sealed: SecretBox,
}

pub struct LoadPlan {
    pub path: PathBuf,
    pub password: Option<String>,
    pub current_key: Option<CurrentKey>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub rows: u64,
    pub warnings: Vec<String>,
    pub secrets: Outcome,
    pub migrated: Vec<i64>,
    pub file_sha256: String,
}

pub fn known_migrations() -> Vec<(i64, bool)> {
    let mut out: Vec<(i64, bool)> = sn_store::MIGRATOR
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .map(|m| (m.version, m.no_tx))
        .collect();
    out.sort();
    out
}

pub fn check_manifest(manifest: &Manifest) -> Result<(), String> {
    if manifest.format != FORMAT {
        return Err("это не копия STEALTHNET".into());
    }
    if manifest.format_version > FORMAT_VERSION {
        return Err(format!(
            "копия сделана более новой версией панели ({}): обновите панель и повторите",
            manifest.app.shown()
        ));
    }
    let known: BTreeSet<i64> = known_migrations().into_iter().map(|(v, _)| v).collect();
    if manifest.migrations.iter().any(|v| !known.contains(v)) {
        return Err(format!(
            "копия сделана более новой версией панели ({}): обновите панель и повторите",
            manifest.app.shown()
        ));
    }
    Ok(())
}

pub fn pending_after(manifest: &Manifest) -> Vec<i64> {
    let done: BTreeSet<i64> = manifest.migrations.iter().copied().collect();
    known_migrations()
        .into_iter()
        .map(|(v, _)| v)
        .filter(|v| !done.contains(v))
        .collect()
}

enum Msg {
    Manifest(Vec<u8>),
    Start(usize),
    Data(bytes::Bytes),
    End(usize),
    Remna,
    Key(Vec<u8>),
}

struct Forward {
    tx: mpsc::Sender<Msg>,
}

impl Forward {
    fn send(&self, msg: Msg) -> std::io::Result<()> {
        self.tx
            .blocking_send(msg)
            .map_err(|_| stop("восстановление остановлено"))
    }
}

impl Visitor for Forward {
    fn manifest(&mut self, raw: &[u8]) -> std::io::Result<()> {
        self.send(Msg::Manifest(raw.to_vec()))
    }

    fn table_start(&mut self, index: usize) -> std::io::Result<()> {
        self.send(Msg::Start(index))
    }

    fn table(&mut self, _index: usize, chunk: &[u8]) -> std::io::Result<()> {
        self.send(Msg::Data(bytes::Bytes::copy_from_slice(chunk)))
    }

    fn table_end(&mut self, index: usize) -> std::io::Result<()> {
        self.send(Msg::End(index))
    }

    fn remna(&mut self, _chunk: &[u8]) -> std::io::Result<()> {
        self.send(Msg::Remna)
    }

    fn secret_key(&mut self, raw: &[u8]) -> std::io::Result<()> {
        self.send(Msg::Key(raw.to_vec()))
    }
}

fn rejected(message: impl Into<String>) -> ServiceError {
    ServiceError::rejected("backups.restore", message)
}

pub fn archive_error(err: ArchiveError) -> ServiceError {
    match err {
        ArchiveError::PasswordNeeded => ServiceError::rejected(
            "backups.password_needed",
            "Копия зашифрована: введите её пароль",
        ),
        ArchiveError::Crypt(super::crypt::CryptError::WrongPassword) => {
            ServiceError::rejected("backups.wrong_password", "Пароль копии не подошёл")
        }
        ArchiveError::Stopped(reason) => ServiceError::rejected("backups.restore", reason),
        ArchiveError::Io(err) => ServiceError::Internal(format!("файл копии не читается: {err}")),
        other => ServiceError::rejected("backups.damaged", other.to_string()),
    }
}

async fn run_sql(conn: &mut PgConnection, sql: &str) -> ServiceResult<()> {
    sqlx::Executor::execute(&mut *conn, sql).await?;
    Ok(())
}

async fn set_path(conn: &mut PgConnection, value: &str) -> ServiceResult<()> {
    sqlx::query("SELECT set_config('search_path', $1, true)")
        .bind(value)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

fn migration_sql(version: i64) -> Option<(String, bool)> {
    sn_store::MIGRATOR
        .iter()
        .find(|m| m.version == version && !m.migration_type.is_down_migration())
        .map(|m| (m.sql.to_string(), m.no_tx))
}

fn plain_sql(sql: &str, no_tx: bool) -> String {
    if !no_tx {
        return sql.to_string();
    }
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(at) = rest.to_ascii_uppercase().find("CONCURRENTLY") {
        out.push_str(&rest[..at]);
        rest = &rest[at + "CONCURRENTLY".len()..];
    }
    out.push_str(rest);
    out
}

async fn migrate(conn: &mut PgConnection, versions: &[i64]) -> ServiceResult<()> {
    for version in versions {
        let (sql, no_tx) = migration_sql(*version).ok_or_else(|| {
            rejected(format!(
                "в этой версии панели нет обновления базы №{version}"
            ))
        })?;
        sqlx::Executor::execute(&mut *conn, plain_sql(&sql, no_tx).as_str())
            .await
            .map_err(|err| {
                ServiceError::Internal(format!("обновление базы №{version} не применилось: {err}"))
            })?;
    }
    Ok(())
}

pub fn key_plan<'a>(
    manifest: &Manifest,
    current: Option<&'a CurrentKey>,
    backup_key: Option<&'a SecretBox>,
) -> Plan<'a> {
    match (current, backup_key) {
        (Some(current), _)
            if manifest.secret_key.check.as_deref() == Some(current.check.as_str()) =>
        {
            Plan::Keep
        }
        (Some(current), Some(from)) => Plan::Rekey {
            from,
            to: &current.sealed,
        },
        _ => Plan::Clear,
    }
}

pub fn backup_key(manifest: &Manifest, raw: Option<&[u8]>) -> Option<SecretBox> {
    let raw = std::str::from_utf8(raw?).ok()?.trim();
    use base64::Engine;
    let key = base64::engine::general_purpose::STANDARD.decode(raw).ok()?;
    if manifest.secret_key.check.as_deref() != Some(super::crypt::key_check(&key).as_str()) {
        return None;
    }
    SecretBox::from_key(&key).ok()
}

pub async fn load(
    conn: &mut PgConnection,
    plan: LoadPlan,
    progress: &(dyn Fn(Stage) + Send + Sync),
) -> ServiceResult<(Manifest, Report)> {
    let (tx, mut rx) = mpsc::channel::<Msg>(16);
    let path = plan.path.clone();
    let password = plan.password.clone();
    let reader = tokio::task::spawn_blocking(move || -> Result<ReadInfo, ArchiveError> {
        let mut forward = Forward { tx };
        archive::read(&path, password.as_deref(), &mut forward)
    });
    let outcome = apply(conn, &mut rx, progress).await;
    rx.close();
    while rx.recv().await.is_some() {}
    let read = reader
        .await
        .map_err(|e| ServiceError::Internal(e.to_string()))?;
    match (outcome, read) {
        (Ok(mut pending), Ok(info)) => {
            pending.report.file_sha256 = info.file_sha256;
            finish(conn, &plan, pending, progress).await
        }
        (Err(err), Ok(_)) => Err(err),
        (Ok(_), Err(err)) => Err(archive_error(err)),
        (Err(err), Err(ArchiveError::Stopped(_))) => Err(err),
        (Err(_), Err(err)) => Err(archive_error(err)),
    }
}

struct Pending {
    manifest: Manifest,
    tmp: String,
    main: String,
    original_path: String,
    report: Report,
    key: Option<Vec<u8>>,
}

async fn apply(
    conn: &mut PgConnection,
    rx: &mut mpsc::Receiver<Msg>,
    progress: &(dyn Fn(Stage) + Send + Sync),
) -> ServiceResult<Pending> {
    let manifest = match rx.recv().await {
        Some(Msg::Manifest(raw)) => serde_json::from_slice::<Manifest>(&raw)
            .map_err(|_| rejected("описание копии не читается"))?,
        Some(_) => return Err(rejected("файл копии повреждён: нет описания")),
        None => {
            return Err(rejected("файл копии не читается"));
        }
    };
    check_manifest(&manifest).map_err(rejected)?;
    let main: String = sqlx::query_scalar("SELECT current_schema()::text")
        .fetch_one(&mut *conn)
        .await?;
    let original_path: String = sqlx::query_scalar("SELECT current_setting('search_path')")
        .fetch_one(&mut *conn)
        .await?;
    let tmp = format!(
        "sn_restore_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    run_sql(conn, &format!("CREATE SCHEMA {}", quote(&tmp))).await?;
    set_path(conn, &quote(&tmp)).await?;
    let mut done: Vec<i64> = manifest.migrations.clone();
    done.sort();
    migrate(conn, &done).await?;
    let scratch = dump::inspect(conn, &tmp, &RESTORE_SKIPPED).await?;
    if !scratch.tables.is_empty() {
        let list = scratch
            .tables
            .iter()
            .map(|t| qualified(&tmp, &t.name))
            .collect::<Vec<_>>()
            .join(", ");
        run_sql(conn, &format!("TRUNCATE {list}")).await?;
    }
    let mut report = Report::default();
    let mut key = None;
    let tables = manifest.tables.len();
    let mut loaded: BTreeSet<usize> = BTreeSet::new();
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Start(index) => {
                let entry = manifest
                    .tables
                    .get(index)
                    .ok_or_else(|| rejected("файл копии повреждён: лишняя таблица"))?;
                let target = scratch.table(&entry.name).ok_or_else(|| {
                    rejected(format!(
                        "в копии есть таблица {}, которой не было в её версии панели",
                        entry.name
                    ))
                })?;
                if let Some(column) = entry.columns.iter().find(|c| !target.columns.contains(c)) {
                    return Err(rejected(format!(
                        "в копии у таблицы {} есть столбец {column}, которого не было в её версии панели",
                        entry.name
                    )));
                }
                progress(Stage::Load {
                    table: index + 1,
                    tables,
                });
                let statement = format!(
                    "COPY {} ({}) FROM STDIN",
                    qualified(&tmp, &entry.name),
                    columns_sql(&entry.columns)
                );
                let mut copy = conn.copy_in_raw(&statement).await?;
                let closed = loop {
                    match rx.recv().await {
                        Some(Msg::Data(bytes)) => {
                            if let Err(err) = copy.send(bytes).await {
                                return Err(ServiceError::Internal(format!(
                                    "таблица {} не загрузилась: {err}",
                                    entry.name
                                )));
                            }
                        }
                        Some(Msg::End(end)) if end == index => break true,
                        _ => break false,
                    }
                };
                if !closed {
                    let _ = copy.abort("файл копии повреждён").await;
                    return Err(rejected("файл копии повреждён: таблица оборвалась"));
                }
                let rows = copy.finish().await.map_err(|err| {
                    ServiceError::Internal(format!("таблица {} не загрузилась: {err}", entry.name))
                })?;
                if rows != entry.rows {
                    return Err(rejected(format!(
                        "в таблице {} {rows} строк вместо {}: файл копии повреждён",
                        entry.name, entry.rows
                    )));
                }
                report.rows += rows;
                loaded.insert(index);
            }
            Msg::Remna => {}
            Msg::Key(raw) => key = Some(raw),
            Msg::Manifest(_) | Msg::Data(_) | Msg::End(_) => {
                return Err(rejected("файл копии повреждён: данные не по порядку"));
            }
        }
    }
    if loaded.len() != tables {
        return Err(rejected("файл копии обрезан: загружены не все таблицы"));
    }
    Ok(Pending {
        manifest,
        tmp,
        main,
        original_path,
        report,
        key,
    })
}

async fn finish(
    conn: &mut PgConnection,
    plan: &LoadPlan,
    pending: Pending,
    progress: &(dyn Fn(Stage) + Send + Sync),
) -> ServiceResult<(Manifest, Report)> {
    let Pending {
        manifest,
        tmp,
        main,
        original_path,
        mut report,
        key,
    } = pending;
    for sequence in &manifest.sequences {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind = 'S')",
        )
        .bind(&tmp)
        .bind(&sequence.name)
        .fetch_one(&mut *conn)
        .await?;
        if exists {
            sqlx::query("SELECT setval($1::regclass, $2, $3)")
                .bind(qualified(&tmp, &sequence.name))
                .bind(sequence.value)
                .bind(sequence.called)
                .execute(&mut *conn)
                .await?;
        }
    }
    progress(Stage::Migrate);
    let rest = pending_after(&manifest);
    migrate(conn, &rest).await?;
    report.migrated = rest;
    progress(Stage::Apply);
    set_path(conn, &original_path).await?;
    let scratch = dump::inspect(conn, &tmp, &RESTORE_SKIPPED).await?;
    let target = dump::inspect(conn, &main, &RESTORE_SKIPPED).await?;
    replace(conn, &scratch, &target, &mut report).await?;
    progress(Stage::Keys);
    let backup_key = backup_key(&manifest, key.as_deref());
    if key.is_some() && backup_key.is_none() {
        report
            .warnings
            .push("ключ шифрования в копии не подошёл к её данным и не использован".into());
    }
    let plan_keys = key_plan(&manifest, plan.current_key.as_ref(), backup_key.as_ref());
    report.secrets = sealed::apply(conn, &main, plan_keys).await?;
    run_sql(conn, &format!("DROP SCHEMA {} CASCADE", quote(&tmp))).await?;
    Ok((manifest, report))
}

async fn replace(
    conn: &mut PgConnection,
    scratch: &Schema,
    target: &Schema,
    report: &mut Report,
) -> ServiceResult<()> {
    let main = &target.name;
    let kept_settings: Vec<(String, serde_json::Value)> = if target.table("settings").is_some() {
        sqlx::query_as(&format!(
            "SELECT key, value FROM {} WHERE starts_with(key, $1)",
            qualified(main, "settings")
        ))
        .bind(KEEP_PREFIX)
        .fetch_all(&mut *conn)
        .await?
    } else {
        Vec::new()
    };
    let kept_secrets: Vec<(String, Vec<u8>, Vec<u8>)> = if target.table("secrets").is_some() {
        sqlx::query_as(&format!(
            "SELECT key, nonce, ciphertext FROM {} WHERE starts_with(key, $1)",
            qualified(main, "secrets")
        ))
        .bind(KEEP_PREFIX)
        .fetch_all(&mut *conn)
        .await?
    } else {
        Vec::new()
    };
    if !target.tables.is_empty() {
        run_sql(conn, "SET LOCAL lock_timeout = '30s'").await?;
        let list = target
            .tables
            .iter()
            .map(|t| qualified(main, &t.name))
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::Executor::execute(&mut *conn, format!("TRUNCATE {list}").as_str())
            .await
            .map_err(|err| {
                if err
                    .as_database_error()
                    .and_then(|e| e.code())
                    .is_some_and(|code| code == "55P03")
                {
                    rejected("Панель занята долгой операцией, таблицы не освободились за 30 секунд. Повторите восстановление чуть позже")
                } else {
                    ServiceError::from(sn_store::StoreError::from(err))
                }
            })?;
        run_sql(conn, "SET LOCAL lock_timeout = 0").await?;
    }
    for table in &target.tables {
        let Some(source) = scratch.table(&table.name) else {
            report.warnings.push(format!(
                "таблица {} есть только в этой версии и осталась пустой",
                table.name
            ));
            continue;
        };
        let columns: Vec<String> = table
            .columns
            .iter()
            .filter(|c| source.columns.contains(c))
            .cloned()
            .collect();
        if columns.is_empty() {
            continue;
        }
        let list = columns_sql(&columns);
        run_sql(
            conn,
            &format!(
                "INSERT INTO {} ({list}) OVERRIDING SYSTEM VALUE SELECT {list} FROM {}",
                qualified(main, &table.name),
                qualified(&scratch.name, &table.name)
            ),
        )
        .await?;
    }
    let scratch_sequences: BTreeSet<&String> = scratch.sequences.iter().collect();
    for name in &target.sequences {
        if !scratch_sequences.contains(name) {
            continue;
        }
        let (value, called) = dump::sequence_state(conn, &scratch.name, name).await?;
        sqlx::query("SELECT setval($1::regclass, $2, $3)")
            .bind(qualified(main, name))
            .bind(value)
            .bind(called)
            .execute(&mut *conn)
            .await?;
    }
    if target.table("settings").is_some() {
        sqlx::query(&format!(
            "DELETE FROM {} WHERE starts_with(key, $1)",
            qualified(main, "settings")
        ))
        .bind(KEEP_PREFIX)
        .execute(&mut *conn)
        .await?;
        for (key, value) in kept_settings {
            sqlx::query(&format!(
                "INSERT INTO {} (key, value) VALUES ($1, $2)",
                qualified(main, "settings")
            ))
            .bind(key)
            .bind(value)
            .execute(&mut *conn)
            .await?;
        }
    }
    if target.table("secrets").is_some() {
        sqlx::query(&format!(
            "DELETE FROM {} WHERE starts_with(key, $1)",
            qualified(main, "secrets")
        ))
        .bind(KEEP_PREFIX)
        .execute(&mut *conn)
        .await?;
        for (key, nonce, ciphertext) in kept_secrets {
            sqlx::query(&format!(
                "INSERT INTO {} (key, nonce, ciphertext) VALUES ($1, $2, $3)",
                qualified(main, "secrets")
            ))
            .bind(key)
            .bind(nonce)
            .bind(ciphertext)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrently_убирается_только_из_миграций_вне_транзакции() {
        let sql = "CREATE INDEX CONCURRENTLY a ON t (x);\ncreate index concurrently b on t (y);";
        assert_eq!(
            plain_sql(sql, true),
            "CREATE INDEX  a ON t (x);\ncreate index  b on t (y);"
        );
        assert_eq!(plain_sql(sql, false), sql);
    }

    #[test]
    fn известные_миграции_идут_по_возрастанию() {
        let known = known_migrations();
        assert!(!known.is_empty());
        assert!(known.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
