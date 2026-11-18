use super::*;
use crate::auth::{ClientLogin, sign_init_data};
use crate::two_factor::CabinetLogin;
use sn_modules::catalog::start_param::Utm;
use sn_modules::catalog::utm::{UtmModule, for_client as origin_of};

const BOT: &str = "123456:TEST-token";

async fn utm_env() -> Option<Env> {
    let env = env_with(ModuleRegistry::new(vec![Arc::new(UtmModule)]).unwrap()).await?;
    {
        let mut conn = env.services.store().conn().await.unwrap();
        settings::write_secret(
            &mut conn,
            env.services.secrets(),
            secret_keys::BOT_TOKEN,
            Some(BOT),
            None,
        )
        .await
        .unwrap();
    }
    env.services.reload().await.unwrap();
    Some(env)
}

fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn init_data(env: &Env, telegram_id: i64, start_param: Option<&str>) -> String {
    let user = format!(
        r#"{{"id":{telegram_id},"first_name":"Алексей","username":"morozov_a","language_code":"ru"}}"#
    );
    let auth_date = env.services.now().timestamp().to_string();
    let mut pairs = vec![("auth_date", auth_date.as_str()), ("user", user.as_str())];
    if let Some(param) = start_param {
        pairs.push(("start_param", param));
    }
    let hash = sign_init_data(&pairs, BOT);
    let mut query: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{key}={}", encoded(value)))
        .collect();
    query.push(format!("hash={hash}"));
    query.join("&")
}

async fn clicks(services: &Services, channel: &str) -> Vec<(String, String, i32)> {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT source, medium, clicks FROM utm_clicks WHERE channel = $1 ORDER BY source, medium",
    )
    .bind(channel)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

fn session(login: CabinetLogin) -> ClientLogin {
    match login {
        CabinetLogin::Session(session) => session,
        CabinetLogin::TotpRequired(_) => panic!("ожидалась сессия"),
    }
}

fn tags(source: &str, medium: Option<&str>, campaign: Option<&str>) -> Utm {
    Utm {
        source: Some(source.into()),
        medium: medium.map(str::to_string),
        campaign: campaign.map(str::to_string),
        ..Utm::default()
    }
}

#[tokio::test]
async fn вход_из_мини_приложения_по_ссылке_с_меткой_считается_переходом() {
    let Some(env) = utm_env().await else { return };
    let auth = env.services.auth();
    let who = meta("203.0.113.40");
    let first = auth
        .miniapp_login(
            &init_data(&env, 5_093_118_501, Some("s_tiktok__m_cpc")),
            &who,
        )
        .await
        .unwrap();
    assert!(first.created);
    assert_eq!(
        clicks(&env.services, "miniapp").await,
        vec![("tiktok".into(), "cpc".into(), 1)]
    );
    auth.miniapp_login(
        &init_data(&env, 5_093_118_501, Some("s_tiktok__m_cpc")),
        &who,
    )
    .await
    .unwrap();
    auth.miniapp_login(&init_data(&env, 5_093_118_502, Some("ref_FRIEND42")), &who)
        .await
        .unwrap();
    auth.miniapp_login(&init_data(&env, 5_093_118_503, None), &who)
        .await
        .unwrap();
    assert_eq!(
        clicks(&env.services, "miniapp").await,
        vec![("tiktok".into(), "cpc".into(), 2)]
    );
    assert!(clicks(&env.services, "bot").await.is_empty());
    env.services.events().dispatch_pending(50).await.unwrap();
    {
        let mut conn = env.services.store().conn().await.unwrap();
        let origin = origin_of(&mut conn, first.principal.client_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(origin.registration_source, RegistrationSource::Miniapp);
    }
    let admin = owner(&env.services).await;
    env.services
        .settings_ops()
        .toggle_module(ModuleId::Utm, false, admin)
        .await
        .unwrap();
    auth.miniapp_login(&init_data(&env, 5_093_118_504, Some("s_vk")), &who)
        .await
        .unwrap();
    assert_eq!(
        clicks(&env.services, "miniapp").await,
        vec![("tiktok".into(), "cpc".into(), 2)]
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn регистрация_на_сайте_по_паролю_и_коду_записывает_метку_сайта() {
    let Some(env) = utm_env().await else { return };
    let auth = env.services.auth();
    let tiktok = tags("tiktok", None, Some("osen"));
    let by_password = session(
        auth.email_register_with(
            "olga@example.com",
            "пароль-ольги-1",
            &meta("203.0.113.41"),
            Some(&tiktok),
        )
        .await
        .unwrap(),
    );
    let who = meta("203.0.113.42");
    let (code, _) = auth
        .issue_email_code("ivan@example.com", &who)
        .await
        .unwrap();
    let by_code = session(
        auth.email_login_verify_with(
            "ivan@example.com",
            &code,
            &who,
            Some(&tags("vk", Some("post"), None)),
        )
        .await
        .unwrap(),
    );
    assert!(by_code.created);
    let (code, _) = auth
        .issue_email_code("ivan@example.com", &who)
        .await
        .unwrap();
    let again = session(
        auth.email_login_verify_with("ivan@example.com", &code, &who, Some(&tiktok))
            .await
            .unwrap(),
    );
    assert!(!again.created);
    let (code, _) = auth
        .issue_email_code("nina@example.com", &who)
        .await
        .unwrap();
    let broken = session(
        auth.email_login_verify_with(
            "nina@example.com",
            &code,
            &who,
            Some(&tags("bad value", None, None)),
        )
        .await
        .unwrap(),
    );
    let plain = session(
        auth.email_register("pavel@example.com", "пароль-павла-1", &meta("203.0.113.43"))
            .await
            .unwrap(),
    );
    env.services.events().dispatch_pending(50).await.unwrap();
    let mut conn = env.services.store().conn().await.unwrap();
    let olga = origin_of(&mut conn, by_password.principal.client_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(olga.start_param, "s_tiktok__k_osen");
    assert_eq!(olga.utm.campaign.as_deref(), Some("osen"));
    assert_eq!(olga.registration_source, RegistrationSource::Web);
    let ivan = origin_of(&mut conn, by_code.principal.client_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ivan.start_param, "s_vk__m_post");
    assert_eq!(ivan.utm.source.as_deref(), Some("vk"));
    assert_eq!(ivan.registration_source, RegistrationSource::Web);
    for client_id in [broken.principal.client_id, plain.principal.client_id] {
        assert!(origin_of(&mut conn, client_id).await.unwrap().is_none());
    }
    drop(conn);
    env.db.cleanup().await;
}
