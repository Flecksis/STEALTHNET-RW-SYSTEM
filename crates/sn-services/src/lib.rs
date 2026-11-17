pub mod admin;
pub mod admins;
pub mod api_keys;
pub mod auth;
pub mod autorenew;
pub mod bot_content;
pub mod bot_runtime;
pub mod branding;
pub mod checkout;
pub mod clients;
pub mod clock;
pub mod core_api;
pub mod error;
pub mod events;
pub mod fulfillment;
pub mod inbox;
pub mod infra;
pub mod landing;
pub mod legacy;
pub mod limits;
pub mod login;
pub mod mail;
pub mod remna_sync;
pub mod services;
pub mod settings;
pub mod stars_review;
pub mod subscriptions;
pub mod support;
pub mod tariffs;
pub mod totp;
pub mod two_factor;
pub mod updates;

pub use bot_runtime::{BotProblem, BotRuntime, BotState};
pub use clock::{Clock, FixedClock, SystemClock};
pub use error::{ServiceError, ServiceResult};
pub use services::{EnvFallbacks, RemnaConnection, ServiceDeps, Services};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum Actor {
    Admin(sn_domain::ids::AdminId),
    Client(sn_domain::ids::ClientId),
    System,
}

impl Actor {
    pub const fn audit_kind(self) -> (sn_store::ActorKind, Option<i64>) {
        match self {
            Actor::Admin(id) => (sn_store::ActorKind::Admin, Some(id.0)),
            Actor::Client(id) => (sn_store::ActorKind::Client, Some(id.0)),
            Actor::System => (sn_store::ActorKind::System, None),
        }
    }

    pub fn audit(
        self,
        action: &str,
        entity_kind: &str,
        entity_id: Option<i64>,
        details: serde_json::Value,
    ) -> sn_store::AuditEntry {
        let (actor_kind, actor_id) = self.audit_kind();
        sn_store::AuditEntry {
            actor_kind,
            actor_id,
            action: action.to_string(),
            entity_kind: Some(entity_kind.to_string()),
            entity_id,
            details,
            ip: None,
        }
    }
}

#[cfg(test)]
mod scenario_tests;
#[cfg(test)]
mod tests;
