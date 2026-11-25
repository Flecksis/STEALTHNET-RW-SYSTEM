use super::*;

pub(super) async fn panel() -> Option<Http> {
    panel_with(EnvFallbacks::default()).await
}

pub(super) async fn panel_with(env: EnvFallbacks) -> Option<Http> {
    let http = http().await?;
    let services = Services::new(ServiceDeps {
        store: http.db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(
            ModuleRegistry::new(vec![Arc::new(sn_modules::catalog::antibot::AntibotModule)])
                .unwrap(),
        ),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: http.clock.clone(),
        env,
    });
    services.reload().await.unwrap();
    Some(Http {
        state: AppState::new(ServerConfig::default(), services),
        ..http
    })
}

async fn register(h: &Http, email: &str, ip: &str, lang: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/client/auth/email/register")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-sn-lang", lang)
        .header("x-forwarded-for", ip)
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40_000,
        ))))
        .body(Body::from(
            json!({ "email": email, "password": "длинный пароль 1" }).to_string(),
        ))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn summary(h: &Http, token: &str, ip: &str) -> Value {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/admin/m/antibot/summary")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header("x-forwarded-for", ip)
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40_000,
        ))))
        .body(Body::empty())
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

#[tokio::test]
async fn отказ_сверх_предела_понятен_на_двух_языках_и_виден_в_админке() {
    let Some(h) = panel().await else { return };
    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::LANGUAGES,
                &json!({ "english": true, "default": "ru" }),
                None,
            )
            .await
            .unwrap();
    }
    h.services().reload().await.unwrap();
    super::captcha::antibot_on(&h, json!({ "site_address_day": 1 })).await;
    let owner = h.owner().await;
    let (status, body) = register(&h, "first@example.com", "203.0.113.30", "ru").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = register(&h, "second@example.com", "203.0.113.30", "ru").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "signup.limit");
    let text = body["error"]["message"].as_str().unwrap();
    assert!(text.contains("Зайдите через бота"), "{text}");
    assert!(text.chars().all(|c| !c.is_ascii_digit()), "{text}");
    let (_, body) = register(&h, "third@example.com", "203.0.113.30", "en").await;
    let text = body["error"]["message"].as_str().unwrap();
    assert!(text.contains("from your address"), "{text}");
    assert!(
        text.chars()
            .all(|c| !('а'..='я').contains(&c.to_ascii_lowercase())),
        "{text}"
    );
    let view = summary(&h, &owner, "198.51.100.20").await;
    assert_eq!(view["settings"]["site_address_day"], 1);
    assert_eq!(view["settings"]["site_network_day"], 0);
    assert_eq!(view["settings"]["site_minute"], 0);
    assert_eq!(
        view["site"]["day"],
        json!({ "created": 1, "address": 2, "network": 0, "minute": 0, "asked": 0 })
    );
    assert_eq!(view["site"]["hidden"], Value::Null);
}

#[tokio::test]
async fn админка_предупреждает_когда_панель_видит_адреса_cloudflare() {
    let Some(h) = panel().await else { return };
    super::captcha::antibot_on(&h, json!({ "site_address_day": 1 })).await;
    let owner = h.owner().await;
    let view = summary(&h, &owner, "172.70.3.4").await;
    assert_eq!(view["site"]["hidden"]["kind"], "cloudflare");
    assert_eq!(view["site"]["hidden"]["address"], "172.70.3.4");
    let view = summary(&h, &owner, "10.0.0.7").await;
    assert_eq!(
        view["site"]["hidden"],
        Value::Null,
        "администратор из своей сети — не повод предупреждать"
    );
    for n in 0..3 {
        let (status, body) =
            register(&h, &format!("edge{n}@example.com"), "162.158.40.1", "ru").await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let view = summary(&h, &owner, "198.51.100.20").await;
    assert_eq!(view["site"]["hidden"]["kind"], "cloudflare");
    assert_eq!(view["site"]["hidden"]["address"], "162.158.40.1");
    assert_eq!(view["site"]["day"]["created"], 3);
}

#[tokio::test]
async fn пороги_с_адреса_сохраняются_в_настройках_и_действуют_на_странице_проверки() {
    let Some(h) = panel().await else { return };
    let owner = h.owner().await;
    super::captcha::antibot_on(&h, json!({})).await;
    let view = summary(&h, &owner, "198.51.100.20").await;
    assert_eq!(view["settings"]["email_ip"], 5);
    assert_eq!(view["settings"]["telegram_ip"], 10);
    assert_eq!(view["settings"]["verify_ip"], 60);
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/modules/antibot/config",
            &owner,
            json!({ "config": { "verify_ip": 0 }, "clear": [] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("от 1 до 1000"),
        "{body}"
    );
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/modules/antibot/config",
            &owner,
            json!({ "config": { "email_ip": 7, "verify_ip": 2 }, "clear": [] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = summary(&h, &owner, "198.51.100.20").await;
    assert_eq!(view["settings"]["email_ip"], 7);
    assert_eq!(view["settings"]["telegram_ip"], 10);
    assert_eq!(view["settings"]["verify_ip"], 2);
    let mut seen = Vec::new();
    for _ in 0..3 {
        let (_, _, body) = h
            .send(
                Method::POST,
                "/api/public/verify",
                None,
                Some(json!({ "token": "0123456789abcdef0123456789abcdef" })),
            )
            .await;
        seen.push(body["error"]["code"].as_str().unwrap().to_string());
    }
    assert_eq!(
        seen,
        ["antibot.expired", "antibot.expired", "auth.rate_limited"]
    );
}
