use super::mini_account::mini_http;
use super::*;
use sn_services::auth::sign_init_data;
use sn_services::server_clock::STATE_KEY;

const BOT: &str = "123456:TEST-token";

fn init_data(opened: DateTime<Utc>, bot: &str) -> String {
    let user = r#"{"id":5093118422,"first_name":"Alex"}"#;
    let auth_date = opened.timestamp().to_string();
    let hash = sign_init_data(&[("auth_date", &auth_date), ("user", user)], bot);
    let encoded: String = user.bytes().map(|b| format!("%{b:02X}")).collect();
    format!("auth_date={auth_date}&user={encoded}&hash={hash}")
}

async fn open(h: &Http, data: &str, lang: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/client/auth/miniapp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(crate::lang::HEADER, lang)
        .body(Body::from(json!({ "init_data": data }).to_string()))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    (response.status(), body_json(response).await)
}

async fn english(h: &Http, owner: &str) {
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/languages",
            owner,
            json!({ "english": true, "default": "ru" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn refused(h: &Http, data: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for lang in ["ru", "en"] {
        let (status, body) = open(h, data, lang).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        let text = body["error"]["message"].as_str().unwrap().to_string();
        assert!(!text.contains("initData"), "{text}");
        assert!(!text.to_lowercase().contains("init data"), "{text}");
        out.push((body["error"]["code"].as_str().unwrap().to_string(), text));
    }
    out
}

fn both(code: &str, ru: &str, en: &str) -> Vec<(String, String)> {
    vec![(code.into(), ru.into()), (code.into(), en.into())]
}

#[tokio::test]
async fn отказ_во_входе_в_мини_приложение_понятен_клиенту_на_двух_языках() {
    let Some(h) = mini_http().await else { return };
    let owner = h.owner().await;
    english(&h, &owner).await;
    assert_eq!(
        refused(&h, &init_data(start() - Duration::hours(2), BOT)).await,
        both(
            "auth.init_data_old",
            "Войти не получилось: данные от Telegram устарели. Закройте мини-приложение и откройте его снова из бота. Если не помогло, напишите в поддержку.",
            "Couldn't sign you in: the data from Telegram is out of date. Close the mini app and open it again from the bot. If that doesn't help, please contact support.",
        )
    );
    assert_eq!(
        refused(&h, &init_data(start() + Duration::minutes(2), BOT)).await,
        both(
            "auth.server_clock",
            "Войти сейчас не получается: на сервере неверное время. Это сбой на нашей стороне. Попробуйте позже или напишите в поддержку.",
            "We can't sign you in right now: the time on our server is wrong. The problem is on our side. Please try again later or contact support.",
        )
    );
    assert_eq!(
        refused(&h, &init_data(start(), "654321:OTHER-token")).await,
        both(
            "auth.init_data",
            "Войти не получилось: данные от Telegram не подошли. Закройте мини-приложение и откройте его снова из бота. Если не помогло, напишите в поддержку.",
            "Couldn't sign you in: the data from Telegram didn't match. Close the mini app and open it again from the bot. If that doesn't help, please contact support.",
        )
    );
    let (status, body) = open(&h, &init_data(start(), BOT), "en").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.db.cleanup().await;
}

async fn warnings(h: &Http, token: &str) -> Value {
    let (status, body) = h.get("/api/admin/system/warnings", token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["warnings"].clone()
}

#[tokio::test]
async fn насколько_часы_расходятся_с_телеграмом_видно_на_главной_админки() {
    let Some(h) = mini_http().await else { return };
    let owner = h.owner().await;
    assert_eq!(warnings(&h, &owner).await, json!([]));
    for (ahead, shown) in [
        (
            7_380,
            json!({ "kind": "server_clock", "seconds": 7_380, "span": "2 ч 3 мин", "evidence": "telegram", "miniapp": "closed" }),
        ),
        (
            420,
            json!({ "kind": "server_clock", "seconds": 420, "span": "7 минут", "evidence": "telegram", "miniapp": "open" }),
        ),
        (
            -300,
            json!({ "kind": "server_clock", "seconds": -300, "span": "5 минут", "evidence": "telegram", "miniapp": "closed" }),
        ),
    ] {
        h.clock.set(start() + Duration::seconds(ahead));
        h.services().server_clock().answered(start());
        h.services().server_clock().answered(start());
        h.services().server_clock_ops().report().await.unwrap();
        assert_eq!(warnings(&h, &owner).await, json!([shown]), "{ahead}");
    }
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/client_access",
            &owner,
            json!({ "miniapp": false, "site_login": true, "self_devices": true, "self_reissue": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(warnings(&h, &owner).await[0]["miniapp"], "off");
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = h.get("/api/admin/system/warnings", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    h.clock.set(start());
    h.services().server_clock().answered(start());
    h.services().server_clock_ops().report().await.unwrap();
    assert_eq!(warnings(&h, &owner).await, json!([]));
    let mut conn = h.db.store.conn().await.unwrap();
    assert_eq!(conn.bot_state().get(STATE_KEY).await.unwrap(), None);
    drop(conn);
    h.db.cleanup().await;
}
