use super::*;
use axum::extract::ConnectInfo;
use sn_domain::client::IdentityKind;
use sn_modules::catalog::utm::visit::VISITS_PER_NETWORK;
use sn_modules::catalog::utm::{UtmModule, for_client as origin_of};
use std::net::SocketAddr;

const VISIT: &str = "/api/public/m/utm/visit";

async fn utm_http() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(vec![Arc::new(UtmModule)]).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna: Arc::new(FakeRemnawave::new()),
    })
}

async fn visit(h: &Http, peer: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(VISIT)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    if let Some(peer) = peer {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(peer.parse().unwrap(), 50_000)));
    }
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

type Row = (String, String, String, String, String, i32);

async fn site_clicks(h: &Http) -> Vec<Row> {
    let mut conn = h.services().store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT source, medium, campaign, content, term, clicks FROM utm_clicks \
         WHERE channel = 'site' ORDER BY source, medium, campaign",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

fn row(source: &str, medium: &str, campaign: &str, clicks: i32) -> Row {
    (
        source.into(),
        medium.into(),
        campaign.into(),
        String::new(),
        String::new(),
        clicks,
    )
}

#[tokio::test]
async fn переход_с_сайта_проверяет_метки_и_считается() {
    let Some(h) = utm_http().await else { return };
    let (status, body) = visit(
        &h,
        Some("203.0.113.7"),
        json!({ "source": "tiktok", "medium": "cpc", "campaign": "osen", "content": "bad value", "term": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, _) = visit(
        &h,
        None,
        json!({ "source": " tiktok ", "medium": "cpc", "campaign": "osen" }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(site_clicks(&h).await, vec![row("tiktok", "cpc", "osen", 2)]);
    for bad in [
        json!({}),
        json!({ "source": "осень" }),
        json!({ "source": "a__b", "medium": "" }),
        json!({ "source": "_x", "term": "x".repeat(65) }),
    ] {
        let (status, body) = visit(&h, Some("203.0.113.7"), bad).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "utm.invalid");
    }
    let (status, _) = visit(&h, Some("203.0.113.7"), json!({ "source": 5 })).await;
    assert!(status.is_client_error(), "{status}");
    assert_eq!(site_clicks(&h).await, vec![row("tiktok", "cpc", "osen", 2)]);
    h.db.cleanup().await;
}

#[tokio::test]
async fn переходы_с_одного_адреса_ограничены_а_выключенный_модуль_не_считает() {
    let Some(h) = utm_http().await else { return };
    let tags = json!({ "source": "vk", "campaign": "zima" });
    for _ in 0..VISITS_PER_NETWORK.limit {
        let (status, body) = visit(&h, Some("198.51.100.20"), tags.clone()).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
    let (status, body) = visit(&h, Some("198.51.100.20"), tags.clone()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "utm.rate_limited");
    let (status, _) = visit(&h, Some("198.51.100.21"), tags.clone()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let counted = i32::try_from(VISITS_PER_NETWORK.limit).unwrap() + 1;
    assert_eq!(site_clicks(&h).await, vec![row("vk", "", "zima", counted)]);
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PATCH,
            "/api/admin/modules/utm",
            &owner,
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = visit(&h, Some("198.51.100.22"), tags).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "module.disabled");
    assert_eq!(site_clicks(&h).await, vec![row("vk", "", "zima", counted)]);
    h.db.cleanup().await;
}

async fn origin_by_email(h: &Http, email: &str) -> Option<sn_modules::catalog::utm::ClientUtm> {
    let mut conn = h.services().store().conn().await.unwrap();
    let client = conn
        .clients()
        .find_by_identity(IdentityKind::Email, email)
        .await
        .unwrap()
        .expect("клиента с такой почтой нет");
    origin_of(&mut conn, client.id).await.unwrap()
}

#[tokio::test]
async fn регистрация_на_сайте_принимает_метку_в_запросе() {
    let Some(h) = utm_http().await else { return };
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register",
            None,
            Some(json!({
                "email": "olga@example.com",
                "password": "пароль-ольги-1",
                "utm": { "source": "tiktok", "medium": 5, "campaign": "osen", "at": 1_800_000_000_000_i64 },
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (code, _) = h
        .services()
        .auth()
        .issue_email_code("ivan@example.com", &Default::default())
        .await
        .unwrap();
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/verify",
            None,
            Some(json!({ "email": "ivan@example.com", "code": code, "utm": { "source": "vk" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register",
            None,
            Some(
                json!({ "email": "nina@example.com", "password": "пароль-нины-1", "utm": "s_vk" }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.services().events().dispatch_pending(50).await.unwrap();
    let olga = origin_by_email(&h, "olga@example.com").await.unwrap();
    assert_eq!(olga.start_param, "s_tiktok__k_osen");
    assert_eq!(olga.registration_source, RegistrationSource::Web);
    let ivan = origin_by_email(&h, "ivan@example.com").await.unwrap();
    assert_eq!(ivan.start_param, "s_vk");
    assert_eq!(ivan.registration_source, RegistrationSource::Web);
    assert!(origin_by_email(&h, "nina@example.com").await.is_none());
    h.db.cleanup().await;
}
