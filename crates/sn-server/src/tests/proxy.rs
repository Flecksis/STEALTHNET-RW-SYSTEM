use super::*;
use sn_egress::{EnvProxy, Route};
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const USER: &str = "shop";
const SECRET_PASS: &str = "Pr0xy:p@ss/word";

struct Socks {
    port: u16,
    seen: Arc<StdMutex<Vec<String>>>,
}

impl Socks {
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    fn server(&self) -> String {
        format!("socks5h://127.0.0.1:{}", self.port)
    }
}

async fn session(tcp: &mut TcpStream, log: &StdMutex<Vec<String>>) -> std::io::Result<()> {
    let mut head = [0u8; 2];
    tcp.read_exact(&mut head).await?;
    let mut methods = vec![0u8; usize::from(head[1])];
    tcp.read_exact(&mut methods).await?;
    if !methods.contains(&2) {
        tcp.write_all(&[5, 0xff]).await?;
        log.lock().unwrap().push("auth:none".into());
        return Ok(());
    }
    tcp.write_all(&[5, 2]).await?;
    let mut version = [0u8; 2];
    tcp.read_exact(&mut version).await?;
    let mut name = vec![0u8; usize::from(version[1])];
    tcp.read_exact(&mut name).await?;
    let mut size = [0u8; 1];
    tcp.read_exact(&mut size).await?;
    let mut pass = vec![0u8; usize::from(size[0])];
    tcp.read_exact(&mut pass).await?;
    let ok = name == USER.as_bytes() && pass == SECRET_PASS.as_bytes();
    tcp.write_all(&[1, u8::from(!ok)]).await?;
    if !ok {
        log.lock().unwrap().push("auth:wrong".into());
        return Ok(());
    }
    let mut request = [0u8; 4];
    tcp.read_exact(&mut request).await?;
    let mut size = [0u8; 1];
    tcp.read_exact(&mut size).await?;
    let mut host = vec![0u8; usize::from(size[0])];
    tcp.read_exact(&mut host).await?;
    let mut port = [0u8; 2];
    tcp.read_exact(&mut port).await?;
    log.lock().unwrap().push(format!(
        "{}:{}",
        String::from_utf8_lossy(&host),
        u16::from_be_bytes(port)
    ));
    tcp.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    Ok(())
}

async fn socks() -> Socks {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let _ = session(&mut tcp, &log).await;
            });
        }
    });
    Socks { port, seen }
}

async fn with_env(env: Option<EnvProxy>) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks {
            proxy: env,
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

async fn audit_details(h: &Http, action: &str) -> Vec<Value> {
    let mut conn = h.services().store().conn().await.unwrap();
    sqlx::query_scalar("SELECT details FROM audit_log WHERE action = $1 ORDER BY id")
        .bind(action)
        .fetch_all(conn.conn())
        .await
        .unwrap()
}

#[tokio::test]
async fn пароль_прокси_не_отдаётся_и_сбрасывается_при_смене_сервера() {
    let Some(h) = with_env(None).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h.get("/api/admin/settings/proxy", &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "none");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["telegram_api"], "https://api.telegram.org");
    assert!(
        body["payment_options"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["id"] != "stars" && o["id"] != "balance")
    );
    for bad in [
        json!({ "enabled": true, "server": "ftp://127.0.0.1:21", "routes": ["telegram"] }),
        json!({ "enabled": true, "server": "socks5://127.0.0.1:1080", "routes": [] }),
        json!({ "enabled": true, "server": "", "routes": ["telegram"] }),
        json!({ "enabled": true, "server": "socks5://127.0.0.1:1080", "routes": ["payments"], "providers": ["nope"] }),
        json!({ "enabled": true, "server": "socks5://127.0.0.1:1080", "password": "x", "routes": ["telegram"] }),
    ] {
        let (status, body) = h
            .write(
                Method::PUT,
                "/api/admin/settings/proxy",
                &owner,
                bad.clone(),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad} → {body}");
        assert_eq!(body["error"]["code"], "settings.invalid_field", "{body}");
    }
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/proxy",
            &owner,
            json!({
                "enabled": true,
                "server": " SOCKS5H://127.0.0.1:18999/ ",
                "username": USER,
                "password": SECRET_PASS,
                "routes": ["telegram", "payments"],
                "providers": ["cryptomus"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.to_string().contains(SECRET_PASS));
    assert_eq!(body["source"], "panel");
    assert_eq!(body["server"], "socks5h://127.0.0.1:18999");
    assert_eq!(body["password_set"], true);
    assert_eq!(body["providers"], json!(["cryptomus"]));
    let egress = h.services().egress();
    let telegram = egress.proxy_for(Route::Telegram, None).unwrap();
    assert_eq!(telegram.password(), Some(SECRET_PASS));
    assert!(
        egress
            .proxy_for(Route::Payments, Some("cryptomus"))
            .is_some()
    );
    assert!(
        egress
            .proxy_for(Route::Payments, Some("yookassa"))
            .is_none()
    );
    assert!(egress.proxy_for(Route::Ai, None).is_none());
    {
        let mut conn = h.services().store().conn().await.unwrap();
        let stored = conn.settings().get(keys::PROXY).await.unwrap().unwrap();
        assert!(!stored.to_string().contains(SECRET_PASS));
        let sealed: Vec<u8> = sqlx::query_scalar(
            "SELECT ciphertext FROM secrets WHERE key = 'egress.proxy_password'",
        )
        .fetch_one(conn.conn())
        .await
        .unwrap();
        assert!(
            !sealed
                .windows(SECRET_PASS.len())
                .any(|w| w == SECRET_PASS.as_bytes())
        );
    }

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/proxy",
            &owner,
            json!({
                "enabled": true,
                "server": "socks5h://127.0.0.1:18999",
                "username": USER,
                "routes": ["telegram", "ai"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["password_set"], true);
    assert_eq!(body["providers"], json!([]));
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/proxy",
            &owner,
            json!({
                "enabled": true,
                "server": "socks5h://198.51.100.20:1080",
                "username": USER,
                "routes": ["telegram"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["password_set"], false);
    assert_eq!(
        h.services()
            .egress()
            .proxy_for(Route::Telegram, None)
            .unwrap()
            .password(),
        None
    );
    let journal = audit_details(&h, "settings.proxy").await;
    let passwords: Vec<&str> = journal
        .iter()
        .map(|d| d["password"].as_str().unwrap())
        .collect();
    assert_eq!(passwords, vec!["set", "kept", "dropped"]);
    assert!(journal.iter().all(|d| !d.to_string().contains(SECRET_PASS)));

    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h.get("/api/admin/settings/proxy", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .write(
            Method::POST,
            "/api/admin/settings/proxy/check",
            &support,
            json!({ "current": true }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn проверка_прокси_объясняет_ошибку_и_не_ходит_мимо_прокси() {
    let Some(h) = with_env(None).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/proxy/check",
            &owner,
            json!({ "current": true }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "settings.proxy_off");
    let fake = socks().await;
    let form = |password: Option<&str>| {
        json!({
            "enabled": true,
            "server": fake.server(),
            "username": USER,
            "password": password,
            "routes": ["telegram", "mail"],
        })
    };
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/proxy/check",
            &owner,
            form(Some("wrong-one")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false);
    let telegram = &body["items"][0];
    assert_eq!(telegram["route"], "telegram");
    assert_eq!(telegram["host"], "api.telegram.org");
    assert_eq!(telegram["problem"], "proxy_auth_rejected", "{body}");
    let mail = &body["items"][1];
    assert_eq!(mail["route"], "mail");
    assert_eq!(mail["direct"], true);
    assert_eq!(fake.seen(), vec!["auth:wrong".to_string()]);

    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/proxy/check",
            &owner,
            form(Some(SECRET_PASS)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["items"][0]["problem"], "proxy_refused_target",
        "{body}"
    );
    assert!(
        body["items"][0]["text"]
            .as_str()
            .unwrap()
            .contains("api.telegram.org")
    );
    assert!(!body.to_string().contains(SECRET_PASS));
    assert_eq!(fake.seen()[1], "api.telegram.org:443");

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/proxy",
            &owner,
            form(Some(SECRET_PASS)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/proxy/check",
            &owner,
            form(None),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["items"][0]["problem"], "proxy_refused_target",
        "{body}"
    );
    assert_eq!(fake.seen().len(), 3);

    assert!(h.services().alerts().collect().await.unwrap().is_empty());
    let health = h.services().egress_ops().watch().await.unwrap();
    assert_eq!(health.failures, 1);
    assert!(!h.services().egress().is_down());
    let health = h.services().egress_ops().watch().await.unwrap();
    assert_eq!(health.failures, 2);
    assert!(h.services().egress().is_down());
    assert!(h.services().egress().down_for(Route::Telegram));
    assert!(!h.services().egress().down_for(Route::Payments));
    let down = audit_details(&h, "settings.proxy_down").await;
    assert_eq!(down.len(), 1);
    assert_eq!(down[0]["target"], "api.telegram.org");
    assert!(!down[0].to_string().contains(SECRET_PASS));
    let (_, view) = h.get("/api/admin/settings/proxy", &owner).await;
    assert_eq!(view["health"]["state"], "down", "{view}");
    let alerts = h.services().alerts().collect().await.unwrap();
    let proxy_alerts: Vec<_> = alerts
        .iter()
        .filter(|a| a.kind == sn_domain::alerts::AlertKind::Proxy)
        .collect();
    assert_eq!(proxy_alerts.len(), 1, "{alerts:?}");
    assert!(proxy_alerts[0].html.contains("Прокси не отвечает"));
    assert!(proxy_alerts[0].html.contains("Telegram"));
    assert!(!proxy_alerts[0].html.contains(SECRET_PASS));
    assert_eq!(proxy_alerts[0].path, "/admin/settings/general#proxy");
    h.services().egress_ops().watch().await.unwrap();
    assert_eq!(audit_details(&h, "settings.proxy_down").await.len(), 1);

    let dead = closed_port().await;
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/proxy/check",
            &owner,
            json!({
                "enabled": true,
                "server": format!("socks5://127.0.0.1:{dead}"),
                "routes": ["telegram"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][0]["problem"], "proxy_unreachable", "{body}");
}

async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn прокси_из_окружения_уступает_панели_и_возвращается_после_сброса() {
    let env = EnvProxy::from_env(
        Some("socks5://envuser:EnvSecret9@203.0.113.9:1080"),
        None,
        None,
    )
    .unwrap();
    let Some(h) = with_env(env).await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h.get("/api/admin/settings/proxy", &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "env");
    assert_eq!(body["env"]["server"], "socks5://203.0.113.9:1080");
    assert_eq!(body["env"]["password_set"], true);
    assert_eq!(body["routes"], json!(["telegram", "ai"]));
    assert!(!body.to_string().contains("EnvSecret9"));
    let egress = h.services().egress();
    assert_eq!(
        egress.proxy_for(Route::Telegram, None).unwrap().password(),
        Some("EnvSecret9")
    );
    assert!(
        egress
            .proxy_for(Route::Payments, Some("cryptomus"))
            .is_none()
    );

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/proxy",
            &owner,
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "panel");
    assert!(
        h.services()
            .egress()
            .proxy_for(Route::Telegram, None)
            .is_none()
    );

    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/proxy/reset",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "env");
    assert!(
        h.services()
            .egress()
            .proxy_for(Route::Telegram, None)
            .is_some()
    );
    assert_eq!(audit_details(&h, "settings.proxy_reset").await.len(), 1);
}
