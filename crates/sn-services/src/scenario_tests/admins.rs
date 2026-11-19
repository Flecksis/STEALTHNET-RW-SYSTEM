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

#[tokio::test]
async fn старые_хеши_bcrypt_заменяются_на_argon2_при_входе() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let auth = services.auth();
    let legacy = bcrypt::hash("старый-пароль-админа", 4).unwrap();
    let admin = {
        let mut conn = services.store().conn().await.unwrap();
        conn.admins()
            .create(&sn_store::NewAdmin {
                email: "legacy@example.com".into(),
                password_hash: legacy,
                role: AdminRole::Admin,
                must_change_password: false,
            })
            .await
            .unwrap()
            .id
    };
    auth.admin_login(
        "legacy@example.com",
        "старый-пароль-админа",
        None,
        &meta("203.0.113.71"),
    )
    .await
    .unwrap();
    let stored = {
        let mut conn = services.store().conn().await.unwrap();
        conn.admins().get(admin).await.unwrap().password_hash
    };
    assert!(stored.starts_with("$argon2"), "{stored}");
    auth.admin_login(
        "legacy@example.com",
        "старый-пароль-админа",
        None,
        &meta("203.0.113.71"),
    )
    .await
    .unwrap();

    auth.email_register(
        "legacy-client@example.com",
        "пароль-клиента-1",
        &meta("203.0.113.72"),
    )
    .await
    .unwrap();
    let client = {
        let mut conn = services.store().conn().await.unwrap();
        let client = conn
            .clients()
            .find_by_identity(IdentityKind::Email, "legacy-client@example.com")
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE client_passwords SET password_hash = $2 WHERE client_id = $1")
            .bind(client.id.0)
            .bind(bcrypt::hash("старый-пароль-клиента", 4).unwrap())
            .execute(conn.conn())
            .await
            .unwrap();
        client.id
    };
    auth.email_password_login(
        "legacy-client@example.com",
        "старый-пароль-клиента",
        &meta("203.0.113.72"),
    )
    .await
    .unwrap();
    let stored: String = {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT password_hash FROM client_passwords WHERE client_id = $1")
            .bind(client.0)
            .fetch_one(conn.conn())
            .await
            .unwrap()
    };
    assert!(stored.starts_with("$argon2"), "{stored}");
    env.db.cleanup().await;
}
