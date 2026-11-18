use super::model::Legacy;
use crate::Services;
use crate::error::ServiceResult;
use serde::{Deserialize, Serialize};
use sn_domain::settings::keys;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum MultiPlan {
    Enable {
        setting: bool,
        clients: u64,
    },
    Same,
    Differs {
        setting: bool,
        clients: u64,
        recommended: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OldMulti {
    pub setting: bool,
    pub clients: u64,
    pub gift_limit: Option<u32>,
}

impl OldMulti {
    pub fn wanted(&self) -> bool {
        self.setting || self.clients > 0
    }
}

pub fn old_multi(legacy: &Legacy) -> OldMulti {
    let setting = legacy
        .settings
        .get("multi_subscriptions_enabled")
        .is_some_and(|v| v.trim() != "false");
    let mut per_client: HashMap<&str, u32> = HashMap::new();
    for sub in &legacy.subscriptions {
        *per_client.entry(sub.client_old_id.as_str()).or_default() += 1;
    }
    let clients = per_client.values().filter(|n| **n > 1).count() as u64;
    let gift_limit = legacy
        .settings
        .get("max_additional_subscriptions")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n > 0);
    OldMulti {
        setting,
        clients,
        gift_limit,
    }
}

async fn chosen(services: &Services) -> ServiceResult<bool> {
    let mut conn = services.store().conn().await?;
    let chosen: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM settings WHERE key = $1)")
        .bind(keys::MULTI_SUBSCRIPTIONS)
        .fetch_one(sn_store::Db::conn(&mut conn))
        .await?;
    Ok(chosen)
}

pub async fn plan(
    services: &Services,
    entity: &str,
    legacy: &Legacy,
) -> ServiceResult<Option<MultiPlan>> {
    let old = old_multi(legacy);
    if !old.wanted() {
        return Ok(None);
    }
    if services.settings().multi_subscriptions {
        return Ok(Some(MultiPlan::Same));
    }
    let moved = super::mail::moved_before(services, entity).await?;
    if !moved && !chosen(services).await? {
        return Ok(Some(MultiPlan::Enable {
            setting: old.setting,
            clients: old.clients,
        }));
    }
    Ok(Some(MultiPlan::Differs {
        setting: old.setting,
        clients: old.clients,
        recommended: old.clients > 0 && !moved,
    }))
}

pub fn reason(setting: bool, clients: u64) -> String {
    let many = match clients {
        0 => None,
        n => Some(format!("у клиентов ({n}) уже по несколько подписок")),
    };
    match (setting, many) {
        (true, Some(many)) => format!("в старой панели разрешалось несколько подписок, {many}"),
        (true, None) => "в старой панели разрешалось несколько подписок".to_string(),
        (false, Some(many)) => format!("в старой панели {many}"),
        (false, None) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::model::{LSubscription, Source};
    use sn_domain::money::Currency;
    use sn_domain::subscription::SubscriptionKind;

    fn sub(id: &str, client: &str) -> LSubscription {
        LSubscription {
            old_id: id.into(),
            client_old_id: client.into(),
            tariff_old_id: None,
            kind: SubscriptionKind::Paid,
            remna_id: None,
            remna_uuid: None,
            short_uuid: None,
            expire_at: None,
            extra_devices: 0,
            autorenew: false,
            autorenew_days: None,
            autorenew_period_old_id: None,
            ordinal: 0,
            gifted: false,
            created_at: None,
        }
    }

    #[test]
    fn несколько_подписок_нужны_по_настройке_или_по_клиентам() {
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Rub);
        assert!(!old_multi(&legacy).wanted());
        legacy.subscriptions = vec![sub("s1", "k1"), sub("s2", "k2")];
        assert!(!old_multi(&legacy).wanted());
        legacy.subscriptions.push(sub("s3", "k1"));
        let old = old_multi(&legacy);
        assert_eq!((old.setting, old.clients), (false, 1));
        assert!(old.wanted());
        legacy.subscriptions.truncate(2);
        for (value, on) in [
            ("true", true),
            ("false", false),
            (" false ", false),
            ("1", true),
        ] {
            legacy
                .settings
                .insert("multi_subscriptions_enabled".into(), value.into());
            assert_eq!(old_multi(&legacy).setting, on, "{value}");
        }
        legacy
            .settings
            .insert("max_additional_subscriptions".into(), "5".into());
        assert_eq!(old_multi(&legacy).gift_limit, Some(5));
        assert_eq!(
            reason(true, 2),
            "в старой панели разрешалось несколько подписок, у клиентов (2) уже по несколько подписок"
        );
        assert_eq!(
            reason(false, 1),
            "в старой панели у клиентов (1) уже по несколько подписок"
        );
    }
}
