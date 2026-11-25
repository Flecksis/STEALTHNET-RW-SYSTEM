use super::*;
use crate::two_factor::CabinetLogin;
use sn_domain::access::ClientChannel;
use sn_domain::ids::SubscriptionId;
use sn_domain::settings::ClientAccess;
use sn_store::SessionChannel;

fn session_token(login: CabinetLogin) -> String {
    match login {
        CabinetLogin::Session(session) => session.token,
        CabinetLogin::TotpRequired(_) => panic!("ожидалась сессия"),
    }
}

#[tokio::test]
async fn вход_по_почте_и_паролю_без_подтверждения() {
    let Some(env) = env().await else { return };
    let auth = env.services.auth();
    let token = session_token(
        auth.email_register("Olga@Example.com", "пароль-ольги-1", &meta("203.0.113.20"))
            .await
            .unwrap(),
    );
    let principal = auth.client_session(&token).await.unwrap();
    assert_eq!(principal.channel, ClientChannel::Cabinet);
    assert_eq!(
        code(
            auth.email_register("olga@example.com", "другой-пароль-1", &meta("203.0.113.21"))
                .await
        ),
        "auth.email_taken"
    );
    assert_eq!(
        code(
            auth.email_register("new@example.com", "короткий", &meta("203.0.113.22"))
                .await
        ),
        "auth.weak_password"
    );
    for (email, password) in [
        ("olga@example.com", "неверный-пароль"),
        ("nobody@example.com", "пароль-ольги-1"),
    ] {
        assert_eq!(
            code(
                auth.email_password_login(email, password, &meta("203.0.113.23"))
                    .await
            ),
            "auth.wrong_password"
        );
    }
    let again = session_token(
        auth.email_password_login(
            " OLGA@example.com ",
            "пароль-ольги-1",
            &meta("203.0.113.24"),
        )
        .await
        .unwrap(),
    );
    assert_eq!(
        auth.client_session(&again).await.unwrap().client_id,
        principal.client_id
    );
    assert_eq!(
        code(
            auth.email_login_start("olga@example.com", &meta("203.0.113.25"))
                .await
        ),
        "auth.email_password_mode"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn выключенные_вход_на_сайт_и_мини_приложение_закрывают_сессии() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let auth = services.auth();
    let ops = services.settings_ops();
    let cabinet = session_token(
        auth.email_register("olga@example.com", "пароль-ольги-1", &meta("203.0.113.30"))
            .await
            .unwrap(),
    );
    let client = auth.client_session(&cabinet).await.unwrap().client_id;
    let mini = auth
        .client_session_for(
            client,
            SessionChannel::Miniapp,
            &meta("203.0.113.31"),
            false,
        )
        .await
        .unwrap()
        .token;
    auth.client_session(&mini).await.unwrap();
    ops.update_client_access(
        &ClientAccess {
            site_login: false,
            ..ClientAccess::default()
        },
        admin,
    )
    .await
    .unwrap();
    assert!(matches!(
        auth.client_session(&cabinet).await,
        Err(ServiceError::Unauthorized)
    ));
    auth.client_session(&mini).await.unwrap();
    assert_eq!(
        code(
            auth.email_password_login("olga@example.com", "пароль-ольги-1", &meta("203.0.113.32"))
                .await
        ),
        "auth.site_off"
    );
    assert_eq!(
        code(auth.telegram_login_start(&meta("203.0.113.33")).await),
        "auth.site_off"
    );
    ops.update_client_access(
        &ClientAccess {
            miniapp: false,
            ..ClientAccess::default()
        },
        admin,
    )
    .await
    .unwrap();
    auth.client_session(&cabinet).await.unwrap();
    assert!(matches!(
        auth.client_session(&mini).await,
        Err(ServiceError::Unauthorized)
    ));
    assert_eq!(
        code(
            auth.miniapp_login("query_id=x", &meta("203.0.113.34"))
                .await
        ),
        "auth.miniapp_off"
    );
    assert_eq!(services.settings().miniapp_url(), None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn клиент_не_отключает_устройства_и_не_меняет_ссылку_если_выключено() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let subscriptions = services.subscriptions();
    let client = Actor::Client(ClientId(1));
    let missing = SubscriptionId(999_999);
    services
        .settings_ops()
        .update_client_access(
            &ClientAccess {
                self_devices: false,
                self_reissue: false,
                ..ClientAccess::default()
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(
        code(subscriptions.reissue_link(missing, client).await),
        "client.action_off"
    );
    assert_eq!(
        code(
            subscriptions
                .disconnect_device(missing, "hwid", client)
                .await
        ),
        "client.action_off"
    );
    assert_eq!(
        code(subscriptions.disconnect_all_devices(missing, client).await),
        "client.action_off"
    );
    assert_ne!(
        code(
            subscriptions
                .reissue_link(missing, Actor::Admin(admin))
                .await
        ),
        "client.action_off"
    );
    services
        .settings_ops()
        .update_client_access(&ClientAccess::default(), admin)
        .await
        .unwrap();
    assert_ne!(
        code(subscriptions.reissue_link(missing, client).await),
        "client.action_off"
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn полный_экран_мини_приложения_включает_только_оператор() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.settings_ops();
    assert!(!ops.client_access().miniapp_fullscreen);
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::CLIENT_ACCESS,
                &serde_json::json!({
                    "miniapp": true,
                    "site_login": true,
                    "self_devices": false,
                    "self_reissue": true,
                }),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    let old = ops.client_access();
    assert!(!old.miniapp_fullscreen);
    assert!(!old.self_devices);
    let wanted = ClientAccess {
        miniapp_fullscreen: true,
        ..old
    };
    let on = ops.update_client_access(&wanted, admin).await.unwrap();
    assert_eq!(on, wanted);
    assert_eq!(
        ops.update_client_access(&wanted, admin).await.unwrap(),
        wanted
    );
    services.reload().await.unwrap();
    assert_eq!(ops.client_access(), wanted);
    ops.update_client_access(&old, admin).await.unwrap();
    assert_eq!(ops.client_access(), old);
    let mut conn = services.store().conn().await.unwrap();
    let details: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE action = 'settings.client_access' ORDER BY id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    let switched: Vec<_> = details
        .iter()
        .map(|d| {
            (
                d["from"]["miniapp_fullscreen"].as_bool(),
                d["to"]["miniapp_fullscreen"].as_bool(),
                d["from"]["self_devices"] == d["to"]["self_devices"],
            )
        })
        .collect();
    assert_eq!(
        switched,
        [
            (Some(false), Some(true), true),
            (Some(true), Some(false), true)
        ]
    );
    drop(conn);
    env.db.cleanup().await;
}
