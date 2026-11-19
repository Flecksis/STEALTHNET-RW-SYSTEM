use super::archive::{self, SealWith, Writer, Written};
use super::crypt::KdfParams;
use super::format::{Manifest, SequenceEntry, TableEntry};
use crate::error::{ServiceError, ServiceResult};
use futures_util::StreamExt;
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use tokio::sync::mpsc;

pub const SKIPPED: [&str; 3] = ["_sqlx_migrations", "backup_runs", "bot_updates"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<Table>,
    pub sequences: Vec<String>,
}

impl Schema {
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name == name)
    }
}

pub fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

pub fn qualified(schema: &str, name: &str) -> String {
    format!("{}.{}", quote(schema), quote(name))
}

pub fn columns_sql(columns: &[String]) -> String {
    columns
        .iter()
        .map(|c| quote(c))
        .collect::<Vec<_>>()
        .join(", ")
}

fn internal(err: impl std::fmt::Display) -> ServiceError {
    ServiceError::Internal(err.to_string())
}

pub fn order(names: &[String], edges: &[(String, String)]) -> Result<Vec<String>, String> {
    let known: BTreeSet<&String> = names.iter().collect();
    let mut needs: BTreeMap<&String, BTreeSet<&String>> =
        names.iter().map(|n| (n, BTreeSet::new())).collect();
    for (child, parent) in edges {
        if child != parent && known.contains(child) && known.contains(parent) {
            needs.entry(child).or_default().insert(parent);
        }
    }
    let mut done: BTreeSet<&String> = BTreeSet::new();
    let mut out = Vec::with_capacity(names.len());
    while out.len() < names.len() {
        let ready: Vec<&String> = needs
            .iter()
            .filter(|(name, parents)| {
                !done.contains(*name) && parents.iter().all(|p| done.contains(p))
            })
            .map(|(name, _)| *name)
            .collect();
        if ready.is_empty() {
            let stuck: Vec<String> = names
                .iter()
                .filter(|n| !done.contains(n))
                .cloned()
                .collect();
            return Err(format!(
                "таблицы ссылаются друг на друга по кругу: {}",
                stuck.join(", ")
            ));
        }
        for name in ready {
            done.insert(name);
            out.push(name.clone());
        }
    }
    Ok(out)
}

pub async fn inspect(
    conn: &mut PgConnection,
    schema: &str,
    skip: &[&str],
) -> ServiceResult<Schema> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT c.oid::bigint, c.relname::text FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relkind IN ('r', 'p') AND NOT c.relispartition \
         ORDER BY c.relname",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await?;
    let rows: Vec<(i64, String)> = rows
        .into_iter()
        .filter(|(_, name)| !skip.contains(&name.as_str()))
        .collect();
    let oids: Vec<i64> = rows.iter().map(|(oid, _)| *oid).collect();
    let by_oid: HashMap<i64, String> = rows.iter().cloned().collect();
    let columns: Vec<(i64, String)> = sqlx::query_as(
        "SELECT attrelid::bigint, attname::text FROM pg_attribute \
         WHERE attrelid::bigint = ANY($1) AND attnum > 0 AND NOT attisdropped AND attgenerated = '' \
         ORDER BY attrelid, attnum",
    )
    .bind(&oids)
    .fetch_all(&mut *conn)
    .await?;
    let mut by_table: HashMap<i64, Vec<String>> = HashMap::new();
    for (oid, column) in columns {
        by_table.entry(oid).or_default().push(column);
    }
    let links: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT conrelid::bigint, confrelid::bigint FROM pg_constraint \
         WHERE contype = 'f' AND conrelid::bigint = ANY($1) AND confrelid::bigint = ANY($1)",
    )
    .bind(&oids)
    .fetch_all(&mut *conn)
    .await?;
    let names: Vec<String> = rows.iter().map(|(_, name)| name.clone()).collect();
    let edges: Vec<(String, String)> = links
        .iter()
        .filter_map(|(child, parent)| {
            Some((by_oid.get(child)?.clone(), by_oid.get(parent)?.clone()))
        })
        .collect();
    let ordered = order(&names, &edges).map_err(|e| ServiceError::rejected("backups.schema", e))?;
    let name_to_oid: HashMap<&String, i64> = rows.iter().map(|(oid, name)| (name, *oid)).collect();
    let tables = ordered
        .into_iter()
        .filter_map(|name| {
            let columns = name_to_oid
                .get(&name)
                .and_then(|oid| by_table.get(oid))
                .cloned()
                .unwrap_or_default();
            (!columns.is_empty()).then_some(Table { name, columns })
        })
        .collect();
    let skipped: Vec<String> = skip.iter().map(|s| s.to_string()).collect();
    let sequences: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relkind = 'S' AND NOT EXISTS ( \
             SELECT 1 FROM pg_depend d JOIN pg_class t ON t.oid = d.refobjid \
             WHERE d.classid = 'pg_class'::regclass AND d.objid = c.oid \
               AND d.refclassid = 'pg_class'::regclass AND d.deptype IN ('a', 'i') \
               AND t.relname = ANY($2)) \
         ORDER BY c.relname",
    )
    .bind(schema)
    .bind(&skipped)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Schema {
        name: schema.to_string(),
        tables,
        sequences,
    })
}

pub async fn sequence_state(
    conn: &mut PgConnection,
    schema: &str,
    name: &str,
) -> ServiceResult<(i64, bool)> {
    Ok(sqlx::query_as(&format!(
        "SELECT last_value, is_called FROM {}",
        qualified(schema, name)
    ))
    .fetch_one(&mut *conn)
    .await?)
}

pub async fn table_entries(
    conn: &mut PgConnection,
    schema: &Schema,
) -> ServiceResult<Vec<TableEntry>> {
    let mut out = Vec::with_capacity(schema.tables.len());
    for table in &schema.tables {
        let rows: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {}",
            qualified(&schema.name, &table.name)
        ))
        .fetch_one(&mut *conn)
        .await?;
        out.push(TableEntry {
            name: table.name.clone(),
            columns: table.columns.clone(),
            rows: rows.max(0) as u64,
        });
    }
    Ok(out)
}

pub async fn sequence_entries(
    conn: &mut PgConnection,
    schema: &Schema,
) -> ServiceResult<Vec<SequenceEntry>> {
    let mut out = Vec::with_capacity(schema.sequences.len());
    for name in &schema.sequences {
        let (value, called) = sequence_state(conn, &schema.name, name).await?;
        out.push(SequenceEntry {
            name: name.clone(),
            value,
            called,
        });
    }
    Ok(out)
}

enum Piece {
    File(&'static str, Vec<u8>),
    Begin(usize, String),
    Data(bytes::Bytes),
}

pub struct Output {
    pub path: PathBuf,
    pub password: Option<String>,
    pub params: KdfParams,
    pub mtime: u64,
}

pub struct Extras {
    pub remna: Vec<u8>,
    pub secret_key: Option<Vec<u8>>,
}

pub async fn write(
    conn: &mut PgConnection,
    schema: &Schema,
    manifest: &Manifest,
    extras: Extras,
    output: Output,
) -> ServiceResult<Written> {
    let manifest_raw = serde_json::to_vec_pretty(manifest).map_err(internal)?;
    let public = serde_json::to_vec(&manifest.public()).map_err(internal)?;
    let (tx, mut rx) = mpsc::channel::<Piece>(8);
    let worker = tokio::task::spawn_blocking(move || -> std::io::Result<Written> {
        let seal = output.password.as_deref().map(|password| SealWith {
            password,
            params: output.params,
            public: &public,
        });
        let mut writer = Writer::create(&output.path, seal, output.mtime)?;
        while let Some(piece) = rx.blocking_recv() {
            match piece {
                Piece::File(name, bytes) => writer.file(name, &bytes)?,
                Piece::Begin(index, name) => writer.begin_table(index, &name)?,
                Piece::Data(bytes) => writer.table_data(&bytes)?,
            }
        }
        writer.finish()
    });
    let sent = async {
        let lost = || ServiceError::Internal("запись копии остановилась".into());
        tx.send(Piece::File(archive::MANIFEST, manifest_raw))
            .await
            .map_err(|_| lost())?;
        for (index, table) in schema.tables.iter().enumerate() {
            tx.send(Piece::Begin(index, table.name.clone()))
                .await
                .map_err(|_| lost())?;
            let statement = format!(
                "COPY {} ({}) TO STDOUT",
                qualified(&schema.name, &table.name),
                columns_sql(&table.columns)
            );
            let mut stream = conn.copy_out_raw(&statement).await?;
            while let Some(chunk) = stream.next().await {
                tx.send(Piece::Data(chunk?)).await.map_err(|_| lost())?;
            }
        }
        tx.send(Piece::File(archive::REMNA, extras.remna))
            .await
            .map_err(|_| lost())?;
        if let Some(key) = extras.secret_key {
            tx.send(Piece::File(archive::SECRET_KEY, key))
                .await
                .map_err(|_| lost())?;
        }
        Ok::<(), ServiceError>(())
    }
    .await;
    drop(tx);
    let written = worker.await.map_err(internal)?;
    match (sent, written) {
        (_, Err(err)) => Err(ServiceError::Internal(format!(
            "файл копии не записан: {err}"
        ))),
        (Err(err), Ok(_)) => Err(err),
        (Ok(()), Ok(written)) => Ok(written),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn таблицы_упорядочиваются_от_родителей_к_детям() {
        let names = vec![s("payments"), s("clients"), s("orders"), s("settings")];
        let edges = vec![
            (s("payments"), s("orders")),
            (s("orders"), s("clients")),
            (s("clients"), s("clients")),
        ];
        assert_eq!(
            order(&names, &edges).unwrap(),
            vec![s("clients"), s("settings"), s("orders"), s("payments")]
        );
        let cycle = vec![(s("a"), s("b")), (s("b"), s("a"))];
        let err = order(&[s("a"), s("b"), s("c")], &cycle).unwrap_err();
        assert!(err.contains("a, b"), "{err}");
    }

    #[test]
    fn имена_в_запросах_экранируются() {
        assert_eq!(quote("clients"), "\"clients\"");
        assert_eq!(quote("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(qualified("sn", "t"), "\"sn\".\"t\"");
        assert_eq!(columns_sql(&[s("a"), s("b")]), "\"a\", \"b\"");
    }
}
