use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SETTING_KEY: &str = "alerts.telegram";
pub const START_PREFIX: &str = "alerts_";
pub const TOPIC_MAX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    Support,
    Orders,
    Stars,
    Refunds,
    Withdrawals,
    Payments,
    Traffic,
}

impl AlertKind {
    pub const ALL: [AlertKind; 7] = [
        AlertKind::Support,
        AlertKind::Orders,
        AlertKind::Stars,
        AlertKind::Refunds,
        AlertKind::Withdrawals,
        AlertKind::Payments,
        AlertKind::Traffic,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            AlertKind::Support => "support",
            AlertKind::Orders => "orders",
            AlertKind::Stars => "stars",
            AlertKind::Refunds => "refunds",
            AlertKind::Withdrawals => "withdrawals",
            AlertKind::Payments => "payments",
            AlertKind::Traffic => "traffic",
        }
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            AlertKind::Support => "Обращения в поддержку",
            AlertKind::Orders => "Заказы на ручной проверке",
            AlertKind::Stars => "Оплаты звёздами на разборе",
            AlertKind::Refunds => "Возвраты: автоматические и незавершённые",
            AlertKind::Withdrawals => "Заявки на вывод",
            AlertKind::Payments => "Споры и возвраты в платёжных системах",
            AlertKind::Traffic => "Превышения трафика",
        }
    }

    pub const fn topic_ru(self) -> &'static str {
        match self {
            AlertKind::Support => "Поддержка",
            AlertKind::Orders => "Заказы",
            AlertKind::Stars => "Звёзды",
            AlertKind::Refunds => "Возвраты",
            AlertKind::Withdrawals => "Выводы",
            AlertKind::Payments => "Платежи",
            AlertKind::Traffic => "Трафик",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertRoute {
    pub on: bool,
    #[serde(default)]
    pub topic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AlertsConfig {
    #[serde(default)]
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub forum: bool,
    #[serde(default)]
    pub routes: BTreeMap<AlertKind, AlertRoute>,
}

pub type TopicMap = BTreeMap<String, i64>;

impl AlertsConfig {
    pub fn route(&self, kind: AlertKind) -> AlertRoute {
        self.routes.get(&kind).cloned().unwrap_or(AlertRoute {
            on: true,
            topic: Some(kind.topic_ru().to_string()),
        })
    }

    pub fn topic_for(&self, kind: AlertKind) -> Option<String> {
        if !self.forum {
            return None;
        }
        self.route(kind)
            .topic
            .map(|t| clean_topic(&t))
            .filter(|t| !t.is_empty())
    }
}

pub fn clean_topic(raw: &str) -> String {
    let joined: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(TOPIC_MAX).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn по_умолчанию_всё_включено_и_разложено_по_темам() {
        let mut config = AlertsConfig {
            forum: true,
            ..AlertsConfig::default()
        };
        assert!(config.route(AlertKind::Stars).on);
        assert_eq!(
            config.topic_for(AlertKind::Stars).as_deref(),
            Some("Звёзды")
        );
        config.routes.insert(
            AlertKind::Orders,
            AlertRoute {
                on: true,
                topic: Some("  Деньги   и заказы ".into()),
            },
        );
        assert_eq!(
            config.topic_for(AlertKind::Orders).as_deref(),
            Some("Деньги и заказы")
        );
        config.routes.insert(
            AlertKind::Refunds,
            AlertRoute {
                on: true,
                topic: Some("   ".into()),
            },
        );
        assert_eq!(config.topic_for(AlertKind::Refunds), None);
        config.forum = false;
        assert_eq!(config.topic_for(AlertKind::Stars), None);
    }
}
