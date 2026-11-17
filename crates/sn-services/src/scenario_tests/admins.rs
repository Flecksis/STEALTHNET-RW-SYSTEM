use super::*;
use crate::admins::NewAdminRequest;
use crate::auth::RequestMeta;
use sn_domain::access::AdminRole;

async fn add(
    services: &Services,
    actor: AdminId,
    email: &str,
    role: AdminRole,
) -> (AdminId, String) {
    let issued = services
        .admins()
        .create(
            actor,
            &NewAdminRequest {
                email: email.into(),
                role,
            },
        )
        .await
        .unwrap();
    assert!(issued.admin.must_change_password);
    (issued.admin.id, issued.password)
}

#[tokio::test]
async fn владелец_добавляет_администратора_с_временным_паролем() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let ops = services.admins();
    let (id, password) = add(services, owner, "olga@example.com", AdminRole::Support).await;
    assert_eq!(password.len(), 16);
    let login = services
        .auth()
        .admin_login("OLGA@example.com", &password, None, &RequestMeta::default())
        .await
        .unwrap();
    assert!(login.must_change_password);
    assert_eq!(login.principal.role, AdminRole::Support);
    assert_eq!(
        code(
            ops.create(
                owner,
                &NewAdminRequest {
                    email: "Olga@Example.com".into(),
                    role: AdminRole::Admin,
                },
            )
            .await
        ),
        "admins.exists"
    );
    assert_eq!(
        code(
            ops.create(
                owner,
                &NewAdminRequest {
                    email: "без почты".into(),
                    role: AdminRole::Admin,
                },
            )
            .await
        ),
        "admins.email"
    );
    let list = ops.list().await.unwrap();
    let olga = list.iter().find(|a| a.id == id).unwrap();
    assert!(olga.last_login_at.is_some());
    assert!(!olga.disabled);
    env.db.cleanup().await;
}

#[tokio::test]
async fn себя_и_последнего_владельца_не_отключить() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let ops = services.admins();
    assert_eq!(code(ops.disable(owner, owner).await), "admins.self");
    assert_eq!(
        code(ops.set_role(owner, owner, AdminRole::Admin).await),
        "admins.self"
    );
    assert_eq!(code(ops.reset_password(owner, owner).await), "admins.self");
    let (second, _) = add(services, owner, "second@example.com", AdminRole::Owner).await;
    let demoted = ops.set_role(second, owner, AdminRole::Admin).await.unwrap();
    assert_eq!(demoted.role, AdminRole::Admin);
    assert_eq!(code(ops.disable(owner, second).await), "admins.owner_only");
    assert_eq!(
        code(ops.set_role(owner, second, AdminRole::Support).await),
        "admins.owner_only"
    );
    let (third, _) = add(services, second, "third@example.com", AdminRole::Admin).await;
    assert_eq!(
        code(ops.set_role(third, second, AdminRole::Admin).await),
        "admins.owner_only"
    );
    assert_eq!(code(ops.disable(third, second).await), "admins.owner_only");
    let raised = ops.set_role(second, owner, AdminRole::Owner).await.unwrap();
    assert_eq!(raised.role, AdminRole::Owner);
    ops.disable(owner, second).await.unwrap();
    assert_eq!(
        code(ops.set_role(second, owner, AdminRole::Admin).await),
        "admins.last_owner"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn отключение_и_сброс_закрывают_сессии() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let owner = owner(services).await;
    let ops = services.admins();
    let (id, password) = add(services, owner, "olga@example.com", AdminRole::Admin).await;
    let auth = services.auth();
    let first = auth
        .admin_login("olga@example.com", &password, None, &RequestMeta::default())
        .await
        .unwrap();
    auth.admin_session(&first.token).await.unwrap();
    let disabled = ops.disable(owner, id).await.unwrap();
    assert!(disabled.disabled);
    assert!(auth.admin_session(&first.token).await.is_err());
    assert!(
        auth.admin_login("olga@example.com", &password, None, &RequestMeta::default())
            .await
            .is_err()
    );
    assert_eq!(code(ops.reset_password(owner, id).await), "admins.disabled");
    assert!(!ops.enable(owner, id).await.unwrap().disabled);
    let second = auth
        .admin_login("olga@example.com", &password, None, &RequestMeta::default())
        .await
        .unwrap();
    let issued = ops.reset_password(owner, id).await.unwrap();
    assert_ne!(issued.password, password);
    assert!(issued.admin.must_change_password);
    assert!(auth.admin_session(&second.token).await.is_err());
    assert!(
        auth.admin_login("olga@example.com", &password, None, &RequestMeta::default())
            .await
            .is_err()
    );
    let fresh = auth
        .admin_login(
            "olga@example.com",
            &issued.password,
            None,
            &RequestMeta::default(),
        )
        .await
        .unwrap();
    let changed = auth
        .change_admin_password(
            id,
            &issued.password,
            "новый-пароль-ольги",
            &RequestMeta::default(),
        )
        .await
        .unwrap();
    assert!(!changed.must_change_password);
    assert!(auth.admin_session(&fresh.token).await.is_err());
    assert!(!ops.reset_two_factor(owner, id).await.unwrap().two_factor);
    env.db.cleanup().await;
}
