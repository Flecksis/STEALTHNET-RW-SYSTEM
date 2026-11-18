use super::*;
use crate::services::RemnaConnection;
use crate::settings::{
    GuardUpdate, PanelGuard, RemnaCheckRequest, RemnaUpdate, remna_connection, tinyauth_key,
};
use base64::Engine as _;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn stored(services: &Services) -> RemnaConnection {
    let mut conn = services.store().conn().await.unwrap();
    remna_connection(&mut conn, services.secrets(), services.env())
        .await
        .unwrap()
}

fn save(guard: GuardUpdate) -> RemnaUpdate {
    RemnaUpdate {
        guard: Some(guard),
        ..Default::default()
    }
}

fn message<T: std::fmt::Debug>(result: ServiceResult<T>) -> String {
    result.unwrap_err().to_string()
}

fn check_with(url: &str, guard: Option<GuardUpdate>) -> RemnaCheckRequest {
    RemnaCheckRequest {
        url: Some(url.to_string()),
        token: Some("token-123".into()),
        cookie: None,
        guard,
    }
}

fn tinyauth(login: &str, password: &str) -> GuardUpdate {
    GuardUpdate::Tinyauth {
        login: Some(login.into()),
        password: Some(password.into()),
    }
}

async fn closing_server() -> String {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut seen = Vec::new();
                let mut chunk = [0u8; 1024];
                while let Ok(read) = socket.read(&mut chunk).await {
                    if read == 0 {
                        break;
                    }
                    seen.extend_from_slice(&chunk[..read]);
                    if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
            });
        }
    });
    format!("http://{address}")
}

#[test]
fn ключ_tinyauth_собирается_в_utf8_без_искажений() {
    assert_eq!(
        tinyauth_key("username", "password").unwrap(),
        "Basic dXNlcm5hbWU6cGFzc3dvcmQ="
    );
    let password = "p%s%d%%x:ä!@#^&*()_+";
    let expected =
        base64::engine::general_purpose::STANDARD.encode(format!("логин:{password}").as_bytes());
    assert_eq!(
        tinyauth_key(" логин ", &format!("{password}\n")).unwrap(),
        format!("Basic {expected}")
    );
    assert!(tinyauth_key("ad:min", "x").is_err());
    assert!(tinyauth_key("admin", "  ").is_err());
    assert!(tinyauth_key("admin", "pa\u{7}ss").is_err());
}

#[tokio::test]
async fn защита_панели_хранится_секретом_и_переключается() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let ops = services.settings_ops();
    let view = ops
        .update_remnawave(
            &RemnaUpdate {
                url: Some("https://panel.example.com".into()),
                token: Some("eyJhbGciOi.token".into()),
                guard: Some(GuardUpdate::Cookie {
                    cookie: Some("https://panel.example.com/auth/login?zIdejKtN=MWbLrRzx".into()),
                }),
                ..Default::default()
            },
            admin,
        )
        .await
        .unwrap();
    assert_eq!(view.guard, PanelGuard::Cookie);
    assert!(view.cookie_set && !view.api_key_set && !view.guard_from_env);
    assert!(!serde_json::to_string(&view).unwrap().contains("MWbLrRzx"));
    assert_eq!(
        serde_json::to_value(&view).unwrap()["guard"],
        serde_json::json!("cookie")
    );
    assert_eq!(
        stored(services).await.cookie.as_deref(),
        Some("zIdejKtN=MWbLrRzx")
    );
    let wrong = ops
        .update_remnawave(
            &save(GuardUpdate::Cookie {
                cookie: Some("просто текст".into()),
            }),
            admin,
        )
        .await;
    assert!(message(wrong).contains("«Кука»"));
    let same = ops
        .update_remnawave(&save(GuardUpdate::Cookie { cookie: None }), admin)
        .await
        .unwrap();
    assert!(same.cookie_set);

    let missing = ops
        .update_remnawave(
            &save(GuardUpdate::Tinyauth {
                login: None,
                password: None,
            }),
            admin,
        )
        .await;
    assert!(message(missing).contains("«Логин TinyAuth»"));
    let view = ops
        .update_remnawave(&save(tinyauth("admin", "Zx%9!q")), admin)
        .await
        .unwrap();
    assert_eq!(view.guard, PanelGuard::Tinyauth);
    assert!(view.api_key_set && !view.cookie_set);
    let connection = stored(services).await;
    assert_eq!(connection.cookie, None);
    assert_eq!(
        connection.api_key,
        Some(tinyauth_key("admin", "Zx%9!q").unwrap())
    );
    let kept = ops
        .update_remnawave(
            &save(GuardUpdate::Tinyauth {
                login: None,
                password: None,
            }),
            admin,
        )
        .await
        .unwrap();
    assert_eq!(kept.guard, PanelGuard::Tinyauth);
    assert_eq!(
        stored(services).await.api_key,
        Some(tinyauth_key("admin", "Zx%9!q").unwrap())
    );

    let no_key = ops
        .update_remnawave(&save(GuardUpdate::Portal { key: None }), admin)
        .await;
    assert!(message(no_key).contains("«Ключ портала»"));
    let view = ops
        .update_remnawave(
            &save(GuardUpdate::Portal {
                key: Some("X-Api-Key: portal-key-123".into()),
            }),
            admin,
        )
        .await
        .unwrap();
    assert_eq!(view.guard, PanelGuard::Portal);
    assert_eq!(
        stored(services).await.api_key.as_deref(),
        Some("portal-key-123")
    );

    let view = ops
        .update_remnawave(&save(GuardUpdate::Off), admin)
        .await
        .unwrap();
    assert_eq!(view.guard, PanelGuard::Off);
    assert!(!view.cookie_set && !view.api_key_set);
    let connection = stored(services).await;
    assert_eq!((connection.cookie, connection.api_key), (None, None));
    let mut conn = services.store().conn().await.unwrap();
    assert!(
        conn.secrets()
            .get(secret_keys::REMNA_COOKIE)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        conn.secrets()
            .get(secret_keys::REMNA_API_KEY)
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    assert!(services.remna().is_ok());
    env.db.cleanup().await;
}

#[tokio::test]
async fn кука_из_переменной_окружения_выключается_в_админке() {
    let Some(db) = test_store().await else { return };
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(FixedClock::new(start())),
        env: EnvFallbacks {
            remna: Some(RemnaConnection {
                url: "https://panel.example.com".into(),
                token: "token".into(),
                cookie: Some("zIdejKtN:MWbLrRzx".into()),
                api_key: None,
                webhook_secret: None,
            }),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    let admin = owner(&services).await;
    let ops = services.settings_ops();
    let view = ops.remnawave().await.unwrap();
    assert_eq!(view.guard, PanelGuard::Cookie);
    assert!(view.cookie_set && view.guard_from_env);
    assert!(services.remna().is_ok());
    let view = ops
        .update_remnawave(&save(GuardUpdate::Off), admin)
        .await
        .unwrap();
    assert_eq!(view.guard, PanelGuard::Off);
    assert!(!view.cookie_set && !view.guard_from_env);
    assert_eq!(stored(&services).await.cookie, None);
    db.cleanup().await;
}

#[tokio::test]
async fn проверка_подсказывает_про_куку_если_соединение_закрыто_без_ответа() {
    let Some(env) = env().await else { return };
    let ops = env.services.settings_ops();
    let url = closing_server().await;
    let check = ops.check_remnawave(&check_with(&url, None)).await.unwrap();
    assert!(!check.ok);
    assert_eq!(
        check.error.as_deref(),
        Some("Панель закрыла соединение, не ответив")
    );
    assert!(check.hint.unwrap().contains("Выберите «Кука»"));
    let check = ops
        .check_remnawave(&check_with(
            &url,
            Some(GuardUpdate::Cookie {
                cookie: Some(r#""~*zIdejKtN=MWbLrRzx" 1;"#.into()),
            }),
        ))
        .await
        .unwrap();
    assert!(check.hint.unwrap().starts_with("Кука не подошла"));
    let wrong = ops
        .check_remnawave(&check_with(
            &url,
            Some(GuardUpdate::Cookie {
                cookie: Some("мусор".into()),
            }),
        ))
        .await;
    assert_eq!(code(wrong), "settings.invalid_field");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let refused = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let check = ops
        .check_remnawave(&check_with(&refused, None))
        .await
        .unwrap();
    assert!(check.error.unwrap().starts_with("Remnawave недоступна"));
    assert_eq!(check.hint, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn проверка_подсказывает_про_страницу_входа_при_редиректе() {
    let Some(env) = env().await else { return };
    let ops = env.services.settings_ops();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/system/metadata"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            "https://auth.example.com/login?redirect_uri=https%3A%2F%2Fpanel.example.com",
        ))
        .mount(&server)
        .await;
    let check = ops
        .check_remnawave(&check_with(&server.uri(), None))
        .await
        .unwrap();
    assert_eq!(
        check.error.as_deref(),
        Some("Панель отправляет на страницу входа auth.example.com")
    );
    assert!(
        check
            .hint
            .unwrap()
            .contains("«Логин и пароль TinyAuth» или «Ключ портала (Caddy)»")
    );
    let check = ops
        .check_remnawave(&check_with(
            &server.uri(),
            Some(tinyauth("admin", "Zx%9!q")),
        ))
        .await
        .unwrap();
    assert!(
        check
            .hint
            .unwrap()
            .starts_with("Логин и пароль TinyAuth не подошли")
    );
    let requests = server.received_requests().await.unwrap();
    let last = requests.last().unwrap();
    assert_eq!(
        last.headers.get("x-api-key").unwrap(),
        tinyauth_key("admin", "Zx%9!q").unwrap().as_str()
    );
    assert_eq!(
        last.headers.get("authorization").unwrap(),
        "Bearer token-123"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.path() != "/api/system/metadata")
            .count(),
        0
    );

    let plain = MockServer::start().await;
    let https = plain.uri().replacen("http://", "https://", 1);
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(301)
                .insert_header("location", format!("{https}/api/system/metadata")),
        )
        .mount(&plain)
        .await;
    let check = ops
        .check_remnawave(&check_with(&plain.uri(), None))
        .await
        .unwrap();
    assert_eq!(
        check.error.as_deref(),
        Some("Панель перенаправляет на https")
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn проверка_отличает_ключ_защиты_от_токена_панели() {
    let Some(env) = env().await else { return };
    let ops = env.services.settings_ops();
    let guarded = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&guarded)
        .await;
    let check = ops
        .check_remnawave(&check_with(
            &guarded.uri(),
            Some(GuardUpdate::Portal {
                key: Some("portal-key-123".into()),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        check.error.as_deref(),
        Some("Защита перед панелью не пропустила запрос (HTTP 401)")
    );
    assert!(check.hint.unwrap().starts_with("Ключ портала не подошёл"));
    let paths: Vec<String> = guarded
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert_eq!(paths, vec!["/api/system/metadata", "/api/auth/status"]);

    let panel = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/system/metadata"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"message": "Unauthorized", "statusCode": 401})),
        )
        .mount(&panel)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/auth/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"response": {}})))
        .mount(&panel)
        .await;
    let check = ops
        .check_remnawave(&check_with(&panel.uri(), None))
        .await
        .unwrap();
    assert_eq!(
        check.error.as_deref(),
        Some("токен Remnawave не принят (HTTP 401)")
    );
    assert!(check.hint.unwrap().contains("«API-токены»"));
    env.db.cleanup().await;
}

#[tokio::test]
async fn проверка_проходит_через_защиту_с_верной_кукой() {
    let Some(env) = env().await else { return };
    let ops = env.services.settings_ops();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/system/metadata"))
        .and(header("cookie", "zIdejKtN=MWbLrRzx"))
        .and(header("authorization", "Bearer token-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"response": {"version": sn_remnawave::SUPPORTED_VERSION}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/system/configuration"))
        .and(header("cookie", "zIdejKtN=MWbLrRzx"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"response": {}})))
        .mount(&server)
        .await;
    let check = ops
        .check_remnawave(&check_with(
            &server.uri(),
            Some(GuardUpdate::Cookie {
                cookie: Some(format!("{}/auth/login?zIdejKtN=MWbLrRzx", server.uri())),
            }),
        ))
        .await
        .unwrap();
    assert!(check.ok, "{check:?}");
    assert!(check.supported);
    assert_eq!(check.hint, None);
    env.db.cleanup().await;
}
