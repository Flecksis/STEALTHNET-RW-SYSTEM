use super::*;
use crate::admins::NewAdminRequest;
use crate::api_keys::{NewKeyRequest, TOKEN_PREFIX};
use sn_domain::access::{AdminPrincipal, AdminRole, Permission};

fn session(admin_id: AdminId, role: AdminRole) -> AdminPrincipal {
    AdminPrincipal {
        admin_id,
        role,
        permissions: Vec::new(),
        session_id: 1,
        api_key: None,
    }
}

fn request(name: &str, permissions: &[Permission], days: Option<i64>) -> NewKeyRequest {
    NewKeyRequest {
        name: name.into(),
        permissions: permissions.to_vec(),
        expires_in_days: days,
    }
}

async fn member(services: &Services, owner: AdminId, email: &str, role: AdminRole) -> AdminId {
    services
        .admins()
        .create(
            owner,
            &NewAdminRequest {
                email: email.into(),
                role,
            },
        )
        .await
        .unwrap()
        .admin
        .id
}

#[tokio::test]
async fn ключ_api_действует_в_пределах_прав_создателя() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let keys = services.api_keys();
    let by_owner = session(owner, AdminRole::Owner);
    for (bad, expected) in [
        (
            request("  ", &[Permission::ClientsRead], None),
            "api_keys.name",
        ),
        (request("Выгрузка", &[], None), "api_keys.permissions"),
        (
            request("Выгрузка", &[Permission::Admins], None),
            "api_keys.permissions",
        ),
        (
            request("Выгрузка", &[Permission::ClientsRead], Some(0)),
            "api_keys.expiry",
        ),
    ] {
        assert_eq!(code(keys.create(&by_owner, &bad).await), expected);
    }
    let support = member(services, owner, "support@example.com", AdminRole::Support).await;
    assert_eq!(
        code(
            keys.create(
                &session(support, AdminRole::Support),
                &request("Деньги", &[Permission::Money], None)
            )
            .await
        ),
        "api_keys.permissions"
    );
    let admin = member(services, owner, "admin@example.com", AdminRole::Admin).await;
    let issued = keys
        .create(
            &session(admin, AdminRole::Admin),
            &request(
                "Выгрузка",
                &[
                    Permission::ClientsRead,
                    Permission::Money,
                    Permission::ClientsRead,
                ],
                Some(30),
            ),
        )
        .await
        .unwrap();
    assert!(issued.token.starts_with(TOKEN_PREFIX));
    assert_eq!(issued.key.prefix, issued.token[..12]);
    assert_eq!(
        issued.key.permissions,
        [Permission::ClientsRead, Permission::Money]
    );
    let principal = keys.principal(&issued.token).await.unwrap();
    assert_eq!(principal.api_key, Some(issued.key.id));
    assert_eq!(principal.admin_id, admin);
    assert!(principal.can(Permission::ClientsRead));
    assert!(principal.can(Permission::Money));
    assert!(!principal.can(Permission::ClientsWrite));
    assert!(!principal.can(Permission::Settings));
    assert_eq!(
        code(
            keys.create(
                &principal,
                &request("Ещё", &[Permission::ClientsRead], None)
            )
            .await
        ),
        "auth.forbidden"
    );
    services
        .admins()
        .set_role(owner, admin, AdminRole::Support)
        .await
        .unwrap();
    let demoted = keys.principal(&issued.token).await.unwrap();
    assert!(demoted.can(Permission::ClientsRead));
    assert!(!demoted.can(Permission::Money));
    services.admins().disable(owner, admin).await.unwrap();
    assert!(keys.principal(&issued.token).await.is_err());
    services.admins().enable(owner, admin).await.unwrap();
    assert!(keys.principal(&issued.token).await.is_ok());
    env.clock.set(start() + Duration::days(31));
    assert!(keys.principal(&issued.token).await.is_err());
    assert!(keys.list().await.unwrap()[0].expired);
    env.clock.set(start());
    let revoked = keys.revoke(&by_owner, issued.key.id).await.unwrap();
    assert!(revoked.revoked);
    assert!(keys.principal(&issued.token).await.is_err());
    assert!(keys.principal("snk_не_ключ").await.is_err());
    env.db.cleanup().await;
}

#[tokio::test]
async fn ключу_api_не_выдаются_вход_за_клиента_и_синхронизация() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let keys = services.api_keys();
    let by_owner = session(owner, AdminRole::Owner);
    for permission in [Permission::ClientsLogin, Permission::RemnaSync] {
        assert_eq!(
            code(
                keys.create(
                    &by_owner,
                    &request("Интеграция", &[Permission::ClientsRead, permission], None)
                )
                .await
            ),
            "api_keys.permissions",
            "{permission:?}"
        );
    }
    let token = crate::api_keys::new_token();
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.api_keys()
            .create(&sn_store::NewApiKey {
                name: "Старый ключ".into(),
                token_prefix: token[..12].to_string(),
                token_hash: crate::auth::token_hash(&token),
                permissions: vec![
                    Permission::ClientsRead,
                    Permission::ClientsLogin,
                    Permission::RemnaSync,
                ],
                created_by: owner,
                expires_at: None,
            })
            .await
            .unwrap();
    }
    let principal = keys.principal(&token).await.unwrap();
    assert!(principal.can(Permission::ClientsRead));
    assert!(!principal.can(Permission::ClientsLogin));
    assert!(!principal.can(Permission::RemnaSync));
    env.db.cleanup().await;
}
