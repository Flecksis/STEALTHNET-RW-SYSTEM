use crate::DomainError;
use crate::ids::{AdminId, ClientId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminRole {
    Owner,
    Admin,
    Support,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ClientsRead,
    ClientsWrite,
    Money,
    Refunds,
    Tariffs,
    Infrastructure,
    Support,
    Marketing,
    Modules,
    Settings,
    Admins,
    ClientsLogin,
}

impl Permission {
    pub const ALL: [Permission; 12] = [
        Permission::ClientsRead,
        Permission::ClientsWrite,
        Permission::Money,
        Permission::Refunds,
        Permission::Tariffs,
        Permission::Infrastructure,
        Permission::Support,
        Permission::Marketing,
        Permission::Modules,
        Permission::Settings,
        Permission::Admins,
        Permission::ClientsLogin,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Permission::ClientsRead => "clients_read",
            Permission::ClientsWrite => "clients_write",
            Permission::Money => "money",
            Permission::Refunds => "refunds",
            Permission::Tariffs => "tariffs",
            Permission::Infrastructure => "infrastructure",
            Permission::Support => "support",
            Permission::Marketing => "marketing",
            Permission::Modules => "modules",
            Permission::Settings => "settings",
            Permission::Admins => "admins",
            Permission::ClientsLogin => "clients_login",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Permission::ALL
            .into_iter()
            .find(|p| p.as_str() == value)
            .ok_or_else(|| DomainError::unknown("permission", value))
    }
}

impl AdminRole {
    pub const ALL: [AdminRole; 4] = [
        AdminRole::Owner,
        AdminRole::Admin,
        AdminRole::Support,
        AdminRole::ReadOnly,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            AdminRole::Owner => "owner",
            AdminRole::Admin => "admin",
            AdminRole::Support => "support",
            AdminRole::ReadOnly => "readonly",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        AdminRole::ALL
            .into_iter()
            .find(|r| r.as_str() == value)
            .ok_or_else(|| DomainError::unknown("admin_role", value))
    }

    pub const fn allows(self, permission: Permission) -> bool {
        match self {
            AdminRole::Owner => true,
            AdminRole::Admin => !matches!(permission, Permission::Admins),
            AdminRole::Support => matches!(
                permission,
                Permission::ClientsRead | Permission::ClientsWrite | Permission::Support
            ),
            AdminRole::ReadOnly => matches!(permission, Permission::ClientsRead),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminPrincipal {
    pub admin_id: AdminId,
    pub role: AdminRole,
    pub permissions: Vec<Permission>,
    pub session_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<i64>,
}

impl AdminPrincipal {
    pub fn can(&self, permission: Permission) -> bool {
        if self.api_key.is_some() {
            return self.permissions.contains(&permission);
        }
        is_allowed(self.role, &self.permissions, permission)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientChannel {
    Cabinet,
    Miniapp,
    Bot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPrincipal {
    pub client_id: ClientId,
    pub channel: ClientChannel,
    pub full_access: bool,
    pub session_id: Option<i64>,
}

pub fn is_allowed(role: AdminRole, extra: &[Permission], permission: Permission) -> bool {
    role.allows(permission) || extra.contains(&permission)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn роли_и_права() {
        assert!(AdminRole::Owner.allows(Permission::Admins));
        assert!(!AdminRole::Admin.allows(Permission::Admins));
        assert!(AdminRole::Support.allows(Permission::Support));
        assert!(!AdminRole::Support.allows(Permission::Refunds));
        assert!(is_allowed(
            AdminRole::Support,
            &[Permission::Refunds],
            Permission::Refunds
        ));
        assert!(!AdminRole::ReadOnly.allows(Permission::ClientsWrite));
        assert!(AdminRole::Owner.allows(Permission::ClientsLogin));
        assert!(AdminRole::Admin.allows(Permission::ClientsLogin));
        assert!(!AdminRole::Support.allows(Permission::ClientsLogin));
        assert!(!AdminRole::ReadOnly.allows(Permission::ClientsLogin));
        assert_eq!(
            Permission::parse("clients_login").unwrap(),
            Permission::ClientsLogin
        );
    }
}
