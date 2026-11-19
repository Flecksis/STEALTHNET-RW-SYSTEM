use crate::Actor;
use crate::auth::token_hash;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sn_domain::access::{AdminPrincipal, Permission, is_allowed};
use sn_store::{ApiKeyRecord, Db, NewApiKey};

pub const TOKEN_PREFIX: &str = "snk_";
pub(crate) const SHOWN_PREFIX_LEN: usize = 12;
const NAME_MAX: usize = 80;
const TOUCH_EVERY_SECONDS: i64 = 60;
pub const EXPIRY_DAYS_MAX: i64 = 3650;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiKeyView {
    pub id: i64,
    pub name: String,
    pub prefix: String,
    pub permissions: Vec<Permission>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked: bool,
    pub expired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssuedKey {
    pub key: ApiKeyView,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NewKeyRequest {
    pub name: String,
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

pub fn is_api_token(raw: &str) -> bool {
    raw.trim().starts_with(TOKEN_PREFIX)
}

pub(crate) fn new_token() -> String {
    let mut raw = [0u8; 24];
    OsRng.fill_bytes(&mut raw);
    format!("{TOKEN_PREFIX}{}", hex::encode(raw))
}

fn view(key: ApiKeyRecord, now: DateTime<Utc>) -> ApiKeyView {
    ApiKeyView {
        expired: key.expires_at.is_some_and(|at| at <= now),
        revoked: key.revoked_at.is_some(),
        id: key.id,
        name: key.name,
        prefix: key.token_prefix,
        permissions: key.permissions,
        created_by: key.created_by_email,
        created_at: key.created_at,
        expires_at: key.expires_at,
        last_used_at: key.last_used_at,
    }
}

fn rejected(code: &'static str, message: &str) -> ServiceError {
    ServiceError::rejected(code, message)
}

fn only_sessions(actor: &AdminPrincipal) -> ServiceResult<()> {
    if actor.api_key.is_some() {
        Err(ServiceError::Forbidden)
    } else {
        Ok(())
    }
}

pub struct ApiKeysOps<'a> {
    services: &'a Services,
}

impl<'a> ApiKeysOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        ApiKeysOps { services }
    }

    pub async fn list(&self) -> ServiceResult<Vec<ApiKeyView>> {
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .api_keys()
            .list()
            .await?
            .into_iter()
            .map(|k| view(k, now))
            .collect())
    }

    pub async fn create(
        &self,
        actor: &AdminPrincipal,
        request: &NewKeyRequest,
    ) -> ServiceResult<IssuedKey> {
        only_sessions(actor)?;
        let name = request.name.trim();
        if name.is_empty() || name.chars().count() > NAME_MAX {
            return Err(rejected(
                "api_keys.name",
                "Название ключа — от 1 до 80 символов",
            ));
        }
        let mut permissions: Vec<Permission> = Vec::new();
        for permission in &request.permissions {
            if !permissions.contains(permission) {
                permissions.push(*permission);
            }
        }
        if permissions.is_empty() {
            return Err(rejected(
                "api_keys.permissions",
                "Выберите хотя бы одно право",
            ));
        }
        if permissions.contains(&Permission::Admins) {
            return Err(rejected(
                "api_keys.permissions",
                "Ключу нельзя дать управление администраторами",
            ));
        }
        if permissions.contains(&Permission::Backups) {
            return Err(rejected(
                "api_keys.permissions",
                "Ключу нельзя дать доступ к резервным копиям",
            ));
        }
        if permissions.contains(&Permission::AuditLog) {
            return Err(rejected(
                "api_keys.permissions",
                "Ключу нельзя дать доступ к журналу действий",
            ));
        }
        if permissions.contains(&Permission::ClientsLogin) {
            return Err(rejected(
                "api_keys.permissions",
                "Ключу нельзя дать вход за клиента",
            ));
        }
        if permissions.contains(&Permission::RemnaSync) {
            return Err(rejected(
                "api_keys.permissions",
                "Ключу нельзя дать синхронизацию с Remnawave",
            ));
        }
        if permissions.iter().any(|p| !actor.can(*p)) {
            return Err(rejected(
                "api_keys.permissions",
                "Ключу можно дать только те права, которые есть у вас",
            ));
        }
        let expires_at = match request.expires_in_days {
            None => None,
            Some(days) if (1..=EXPIRY_DAYS_MAX).contains(&days) => {
                Some(self.services.now() + Duration::days(days))
            }
            Some(_) => {
                return Err(rejected(
                    "api_keys.expiry",
                    "Срок ключа — от 1 дня до 10 лет",
                ));
            }
        };
        let token = new_token();
        let mut tx = self.services.store().begin().await?;
        let key = tx
            .api_keys()
            .create(&NewApiKey {
                name: name.to_string(),
                token_prefix: token[..SHOWN_PREFIX_LEN].to_string(),
                token_hash: token_hash(&token),
                permissions: permissions.clone(),
                created_by: actor.admin_id,
                expires_at,
            })
            .await?;
        tx.audit()
            .record(&Actor::Admin(actor.admin_id).audit(
                "api_key.created",
                "api_key",
                Some(key.id),
                serde_json::json!({
                    "name": key.name,
                    "permissions": permissions.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(IssuedKey {
            key: view(key, self.services.now()),
            token,
        })
    }

    pub async fn revoke(&self, actor: &AdminPrincipal, id: i64) -> ServiceResult<ApiKeyView> {
        only_sessions(actor)?;
        let mut tx = self.services.store().begin().await?;
        let key = tx.api_keys().get(id).await?;
        if tx.api_keys().revoke(id).await? {
            tx.audit()
                .record(&Actor::Admin(actor.admin_id).audit(
                    "api_key.revoked",
                    "api_key",
                    Some(id),
                    serde_json::json!({ "name": key.name }),
                ))
                .await?;
        }
        let key = tx.api_keys().get(id).await?;
        tx.commit().await?;
        Ok(view(key, self.services.now()))
    }

    pub async fn principal(&self, token: &str) -> ServiceResult<AdminPrincipal> {
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        let owner = conn
            .api_keys()
            .owner_by_token(&token_hash(token), now)
            .await?
            .ok_or(ServiceError::Unauthorized)?;
        if owner
            .last_used_at
            .is_none_or(|at| now - at > Duration::seconds(TOUCH_EVERY_SECONDS))
        {
            conn.api_keys().touch(owner.key_id).await?;
        }
        let permissions = owner
            .permissions
            .iter()
            .copied()
            .filter(|p| {
                !matches!(
                    *p,
                    Permission::Admins
                        | Permission::Backups
                        | Permission::AuditLog
                        | Permission::ClientsLogin
                        | Permission::RemnaSync
                ) && is_allowed(owner.role, &owner.admin_permissions, *p)
            })
            .collect();
        Ok(AdminPrincipal {
            admin_id: owner.admin_id,
            role: owner.role,
            permissions,
            session_id: 0,
            api_key: Some(owner.key_id),
        })
    }
}
