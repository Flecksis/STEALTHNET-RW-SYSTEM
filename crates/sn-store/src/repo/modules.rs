use crate::error::StoreResult;
use chrono::{DateTime, Utc};
use sn_domain::ids::AdminId;
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ModuleStateRow {
    pub module: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}

pub struct ModulesRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> ModulesRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        ModulesRepo { conn }
    }

    pub async fn all(&mut self) -> StoreResult<Vec<ModuleStateRow>> {
        Ok(sqlx::query_as(
            "SELECT module, enabled, config, updated_at FROM module_states ORDER BY module",
        )
        .fetch_all(&mut *self.conn)
        .await?)
    }

    pub async fn ensure_defaults(&mut self, defaults: &[(&str, bool)]) -> StoreResult<u64> {
        let (names, enabled): (Vec<&str>, Vec<bool>) = defaults.iter().copied().unzip();
        let done = sqlx::query(
            "INSERT INTO module_states (module, enabled) SELECT * FROM unnest($1::text[], $2::bool[]) \
             ON CONFLICT (module) DO NOTHING",
        )
        .bind(&names)
        .bind(&enabled)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected())
    }

    pub async fn set_enabled(
        &mut self,
        module: &str,
        enabled: bool,
        by: Option<AdminId>,
    ) -> StoreResult<ModuleStateRow> {
        Ok(sqlx::query_as(
            "INSERT INTO module_states (module, enabled, updated_by) VALUES ($1, $2, $3) \
             ON CONFLICT (module) DO UPDATE SET enabled = EXCLUDED.enabled, updated_by = EXCLUDED.updated_by, updated_at = now() \
             RETURNING module, enabled, config, updated_at",
        )
        .bind(module)
        .bind(enabled)
        .bind(by.map(|a| a.0))
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn set_config(
        &mut self,
        module: &str,
        config: &serde_json::Value,
        by: Option<AdminId>,
    ) -> StoreResult<ModuleStateRow> {
        Ok(sqlx::query_as(
            "UPDATE module_states SET config = $2, updated_by = $3, updated_at = now() WHERE module = $1 \
             RETURNING module, enabled, config, updated_at",
        )
        .bind(module)
        .bind(config)
        .bind(by.map(|a| a.0))
        .fetch_one(&mut *self.conn)
        .await?)
    }
}
