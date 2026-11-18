use crate::Actor;
use crate::auth::hash_password_async;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sn_domain::access::{AdminRole, Permission, is_allowed};
use sn_domain::ids::AdminId;
use sn_store::{AdminRecord, Db, NewAdmin};
use std::collections::HashMap;

const PASSWORD_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const PASSWORD_LEN: usize = 16;
const MAX_EMAIL_LEN: usize = 254;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdminView {
    pub id: AdminId,
    pub email: String,
    pub role: AdminRole,
    pub two_factor: bool,
    pub disabled: bool,
    pub must_change_password: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub remna_sync: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssuedPassword {
    pub admin: AdminView,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NewAdminRequest {
    pub email: String,
    pub role: AdminRole,
}

pub fn temporary_password() -> String {
    let limit = 256 - 256 % PASSWORD_ALPHABET.len();
    let mut out = String::with_capacity(PASSWORD_LEN);
    let mut buf = [0u8; 64];
    while out.len() < PASSWORD_LEN {
        OsRng.fill_bytes(&mut buf);
        for byte in buf {
            let byte = byte as usize;
            if byte < limit && out.len() < PASSWORD_LEN {
                out.push(PASSWORD_ALPHABET[byte % PASSWORD_ALPHABET.len()] as char);
            }
        }
    }
    out
}

fn clean_email(raw: &str) -> ServiceResult<String> {
    let email = raw.trim();
    let valid = email.len() <= MAX_EMAIL_LEN
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if valid {
        Ok(email.to_string())
    } else {
        Err(ServiceError::rejected(
            "admins.email",
            "Почта записана неверно",
        ))
    }
}

fn view(admin: AdminRecord, logins: &HashMap<AdminId, DateTime<Utc>>) -> AdminView {
    AdminView {
        id: admin.id,
        last_login_at: logins.get(&admin.id).copied(),
        email: admin.email,
        role: admin.role,
        two_factor: admin.totp.is_some(),
        disabled: admin.disabled_at.is_some(),
        must_change_password: admin.must_change_password,
        created_at: admin.created_at,
        remna_sync: is_allowed(admin.role, &admin.permissions, Permission::RemnaSync),
    }
}

fn not_self(actor: AdminId, id: AdminId, message: &str) -> ServiceResult<()> {
    if actor == id {
        Err(ServiceError::rejected("admins.self", message))
    } else {
        Ok(())
    }
}

fn last_owner() -> ServiceError {
    ServiceError::rejected(
        "admins.last_owner",
        "Это единственный владелец: сначала назначьте владельцем кого-то ещё",
    )
}

fn owners_only() -> ServiceError {
    ServiceError::rejected(
        "admins.owner_only",
        "Владельцами распоряжается только владелец",
    )
}

pub struct AdminsOps<'a> {
    services: &'a Services,
}

impl<'a> AdminsOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        AdminsOps { services }
    }

    pub async fn list(&self) -> ServiceResult<Vec<AdminView>> {
        let mut conn = self.services.store().conn().await?;
        let admins = conn.admins().list().await?;
        let logins = conn.admins().last_logins().await?;
        Ok(admins.into_iter().map(|a| view(a, &logins)).collect())
    }

    async fn find(&self, id: AdminId) -> ServiceResult<AdminView> {
        let mut conn = self.services.store().conn().await?;
        let admin = conn.admins().get(id).await?;
        let logins = conn.admins().last_logins().await?;
        Ok(view(admin, &logins))
    }

    async fn actor_role(db: &mut impl Db, actor: AdminId) -> ServiceResult<AdminRole> {
        Ok(db.admins().get(actor).await?.role)
    }

    pub async fn create(
        &self,
        actor: AdminId,
        request: &NewAdminRequest,
    ) -> ServiceResult<IssuedPassword> {
        let email = clean_email(&request.email)?;
        let password = temporary_password();
        let hash = hash_password_async(&password).await?;
        let mut tx = self.services.store().begin().await?;
        if request.role == AdminRole::Owner
            && Self::actor_role(&mut tx, actor).await? != AdminRole::Owner
        {
            return Err(owners_only());
        }
        if tx.admins().by_email(&email).await?.is_some() {
            return Err(ServiceError::rejected(
                "admins.exists",
                "Администратор с такой почтой уже есть",
            ));
        }
        let admin = tx
            .admins()
            .create(&NewAdmin {
                email,
                password_hash: hash,
                role: request.role,
                must_change_password: true,
            })
            .await?;
        tx.audit()
            .record(&Actor::Admin(actor).audit(
                "admin.created",
                "admin",
                Some(admin.id.0),
                serde_json::json!({ "role": admin.role.as_str() }),
            ))
            .await?;
        tx.commit().await?;
        Ok(IssuedPassword {
            admin: view(admin, &HashMap::new()),
            password,
        })
    }

    pub async fn set_role(
        &self,
        actor: AdminId,
        id: AdminId,
        role: AdminRole,
    ) -> ServiceResult<AdminView> {
        not_self(actor, id, "Свою роль поменять нельзя")?;
        let mut tx = self.services.store().begin().await?;
        let owners = tx.admins().lock_owners().await?;
        let admin = tx.admins().get(id).await?;
        if admin.role != role {
            let by_owner = Self::actor_role(&mut tx, actor).await? == AdminRole::Owner;
            if !by_owner && (admin.role == AdminRole::Owner || role == AdminRole::Owner) {
                return Err(owners_only());
            }
            if admin.role == AdminRole::Owner && admin.disabled_at.is_none() && owners <= 1 {
                return Err(last_owner());
            }
            tx.admins().set_role(id, role, &admin.permissions).await?;
            tx.audit()
                .record(&Actor::Admin(actor).audit(
                    "admin.role_changed",
                    "admin",
                    Some(id.0),
                    serde_json::json!({ "from": admin.role.as_str(), "to": role.as_str() }),
                ))
                .await?;
        }
        tx.commit().await?;
        self.find(id).await
    }

    pub async fn set_remna_sync(
        &self,
        actor: AdminId,
        id: AdminId,
        on: bool,
    ) -> ServiceResult<AdminView> {
        not_self(actor, id, "Свои права поменять нельзя")?;
        let mut tx = self.services.store().begin().await?;
        if Self::actor_role(&mut tx, actor).await? != AdminRole::Owner {
            return Err(owners_only());
        }
        let admin = tx.admins().get(id).await?;
        if admin.role == AdminRole::Owner {
            return Err(ServiceError::rejected(
                "admins.owner_has_all",
                "У владельца это право есть всегда",
            ));
        }
        let had = admin.permissions.contains(&Permission::RemnaSync);
        if had != on {
            let mut permissions: Vec<Permission> = admin
                .permissions
                .iter()
                .copied()
                .filter(|p| *p != Permission::RemnaSync)
                .collect();
            if on {
                permissions.push(Permission::RemnaSync);
            }
            tx.admins().set_role(id, admin.role, &permissions).await?;
            tx.audit()
                .record(&Actor::Admin(actor).audit(
                    "admin.permission_changed",
                    "admin",
                    Some(id.0),
                    serde_json::json!({ "permission": Permission::RemnaSync.as_str(), "on": on }),
                ))
                .await?;
        }
        tx.commit().await?;
        self.find(id).await
    }

    pub async fn disable(&self, actor: AdminId, id: AdminId) -> ServiceResult<AdminView> {
        not_self(actor, id, "Свой доступ отключить нельзя")?;
        let mut tx = self.services.store().begin().await?;
        let owners = tx.admins().lock_owners().await?;
        let admin = tx.admins().get(id).await?;
        if admin.disabled_at.is_none() {
            if admin.role == AdminRole::Owner {
                if Self::actor_role(&mut tx, actor).await? != AdminRole::Owner {
                    return Err(owners_only());
                }
                if owners <= 1 {
                    return Err(last_owner());
                }
            }
            tx.admins().disable(id).await?;
            tx.sessions().revoke_all_admin(id).await?;
            tx.audit()
                .record(&Actor::Admin(actor).audit(
                    "admin.disabled",
                    "admin",
                    Some(id.0),
                    serde_json::json!({}),
                ))
                .await?;
        }
        tx.commit().await?;
        self.find(id).await
    }

    pub async fn enable(&self, actor: AdminId, id: AdminId) -> ServiceResult<AdminView> {
        let mut tx = self.services.store().begin().await?;
        let admin = tx.admins().get(id).await?;
        if admin.disabled_at.is_some() {
            if admin.role == AdminRole::Owner
                && Self::actor_role(&mut tx, actor).await? != AdminRole::Owner
            {
                return Err(owners_only());
            }
            tx.admins().enable(id).await?;
            tx.audit()
                .record(&Actor::Admin(actor).audit(
                    "admin.enabled",
                    "admin",
                    Some(id.0),
                    serde_json::json!({}),
                ))
                .await?;
        }
        tx.commit().await?;
        self.find(id).await
    }

    pub async fn reset_password(
        &self,
        actor: AdminId,
        id: AdminId,
    ) -> ServiceResult<IssuedPassword> {
        not_self(actor, id, "Свой пароль меняется в разделе «Безопасность»")?;
        let password = temporary_password();
        let hash = hash_password_async(&password).await?;
        let mut tx = self.services.store().begin().await?;
        let admin = tx.admins().get(id).await?;
        if admin.disabled_at.is_some() {
            return Err(ServiceError::rejected(
                "admins.disabled",
                "Доступ отключён: сначала включите его",
            ));
        }
        if admin.role == AdminRole::Owner
            && Self::actor_role(&mut tx, actor).await? != AdminRole::Owner
        {
            return Err(owners_only());
        }
        tx.admins().set_password(id, &hash, true).await?;
        tx.sessions().revoke_all_admin(id).await?;
        tx.audit()
            .record(&Actor::Admin(actor).audit(
                "admin.password_reset",
                "admin",
                Some(id.0),
                serde_json::json!({}),
            ))
            .await?;
        tx.commit().await?;
        Ok(IssuedPassword {
            admin: self.find(id).await?,
            password,
        })
    }

    pub async fn reset_two_factor(&self, actor: AdminId, id: AdminId) -> ServiceResult<AdminView> {
        not_self(
            actor,
            id,
            "Свой вход по коду выключается в разделе «Безопасность»",
        )?;
        let mut tx = self.services.store().begin().await?;
        let admin = tx.admins().get(id).await?;
        if admin.totp.is_some() {
            if admin.role == AdminRole::Owner
                && Self::actor_role(&mut tx, actor).await? != AdminRole::Owner
            {
                return Err(owners_only());
            }
            tx.admins().set_totp(id, None).await?;
            tx.sessions().revoke_all_admin(id).await?;
            tx.audit()
                .record(&Actor::Admin(actor).audit(
                    "admin.totp_reset",
                    "admin",
                    Some(id.0),
                    serde_json::json!({}),
                ))
                .await?;
        }
        tx.commit().await?;
        self.find(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn временный_пароль_длинный_и_без_похожих_символов() {
        let first = temporary_password();
        let second = temporary_password();
        assert_eq!(first.len(), PASSWORD_LEN);
        assert_ne!(first, second);
        assert!(first.bytes().all(|b| PASSWORD_ALPHABET.contains(&b)));
        assert!(!first.contains(['0', 'O', 'o', '1', 'l', 'I']));
    }

    #[test]
    fn почта_проверяется_до_записи() {
        assert_eq!(
            clean_email("  olga@example.com ").unwrap(),
            "olga@example.com"
        );
        for bad in [
            "",
            "olga",
            "olga@",
            "@example.com",
            "olga@example",
            "ol ga@example.com",
            "a@b@c.com",
            "olga@.com",
        ] {
            assert!(clean_email(bad).is_err(), "{bad}");
        }
    }
}
