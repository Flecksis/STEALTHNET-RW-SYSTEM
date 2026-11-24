use super::*;
use sn_modules::catalog::broadcast::{BroadcastModule, TEST_LETTERS};

const TEST: &str = "/api/admin/m/broadcast/test";

async fn broadcast_http() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(vec![Arc::new(BroadcastModule)]).unwrap()),
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

fn letter(to: &str) -> Value {
    json!({ "email": to, "subject": "Проверка", "text": "Привет" })
}

#[tokio::test]
async fn проверочные_письма_рассылки_на_адрес_ограничены_для_каждого_администратора() {
    let Some(h) = broadcast_http().await else {
        return;
    };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PATCH,
            "/api/admin/modules/broadcast",
            &owner,
            json!({ "enabled": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for n in 0..TEST_LETTERS.limit {
        let (status, body) = h
            .write(
                Method::POST,
                TEST,
                &owner,
                letter(&format!("box{n}@example.com")),
            )
            .await;
        assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{n}: {body}");
    }
    let (status, body) = h
        .write(Method::POST, TEST, &owner, letter("last@example.com"))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "broadcast.rate_limited");
    let (status, body) = h
        .write(
            Method::POST,
            TEST,
            &owner,
            json!({ "client_id": 1, "text": "Привет" }),
        )
        .await;
    assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    let marketer = h.admin("marketer@example.com", AdminRole::Admin).await;
    let (status, body) = h
        .write(Method::POST, TEST, &marketer, letter("other@example.com"))
        .await;
    assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    h.clock
        .set(start() + Duration::seconds(TEST_LETTERS.window_seconds + 1));
    let (status, body) = h
        .write(Method::POST, TEST, &owner, letter("later@example.com"))
        .await;
    assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    h.db.cleanup().await;
}
