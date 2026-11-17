pub mod broadcast;
pub mod mail;
pub mod options;
pub mod promo;
pub mod referral;
pub mod start_param;
pub mod utm;
pub mod web;

#[cfg(test)]
mod testing;

use crate::module::{Module, ModuleConfig};
use chrono::{DateTime, Utc};
use sn_domain::module::ModuleId;
use sn_domain::money::Currency;
use sn_domain::settings::keys;
use sn_store::{Db, StoreResult};
use std::sync::Arc;

pub fn all() -> Vec<Arc<dyn Module>> {
    vec![
        Arc::new(promo::PromoModule),
        Arc::new(options::OptionsModule),
        Arc::new(referral::ReferralModule),
        Arc::new(utm::UtmModule),
        Arc::new(mail::MailModule),
        Arc::new(broadcast::BroadcastModule),
    ]
}

pub async fn module_config(db: &mut impl Db, id: ModuleId) -> StoreResult<ModuleConfig> {
    let value: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT config FROM module_states WHERE module = $1")
            .bind(id.as_str())
            .fetch_optional(db.conn())
            .await?;
    Ok(value
        .map(|v| ModuleConfig::from_value(&v))
        .unwrap_or_default())
}

pub async fn installation_currency(db: &mut impl Db) -> StoreResult<Currency> {
    Ok(db
        .settings()
        .get(keys::CURRENCY)
        .await?
        .and_then(|v| serde_json::from_value::<Currency>(v).ok())
        .unwrap_or_default())
}

pub async fn db_now(db: &mut impl Db) -> StoreResult<DateTime<Utc>> {
    Ok(sqlx::query_scalar("SELECT now()")
        .fetch_one(db.conn())
        .await?)
}

pub fn config_number(config: &ModuleConfig, key: &str) -> Option<i64> {
    match config.get(key)? {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn config_flag(config: &ModuleConfig, key: &str) -> Option<bool> {
    match config.get(key)? {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::String(s) => match s.trim() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ModuleRegistry;

    #[test]
    fn каталог_подменяет_заглушки_реализованными_модулями() {
        let modules = all();
        let ids: Vec<ModuleId> = modules.iter().map(|m| m.id()).collect();
        assert_eq!(
            ids,
            [
                ModuleId::Promo,
                ModuleId::Options,
                ModuleId::Referral,
                ModuleId::Utm,
                ModuleId::Mail,
                ModuleId::Broadcast
            ]
        );
        let registry = ModuleRegistry::new(modules).unwrap();
        let referral = registry
            .views()
            .into_iter()
            .find(|v| v.info.id == ModuleId::Referral)
            .unwrap();
        assert!(referral.has_settings);
    }

    #[test]
    fn числа_и_флаги_из_настроек_модуля() {
        let config = ModuleConfig::from_value(&serde_json::json!({
            "a": 30, "b": " 12 ", "c": "x", "d": true, "e": "false"
        }));
        assert_eq!(config_number(&config, "a"), Some(30));
        assert_eq!(config_number(&config, "b"), Some(12));
        assert_eq!(config_number(&config, "c"), None);
        assert_eq!(config_number(&config, "zz"), None);
        assert_eq!(config_flag(&config, "d"), Some(true));
        assert_eq!(config_flag(&config, "e"), Some(false));
    }
}
