use super::*;

async fn send_in(
    h: &Http,
    method: Method,
    path: &str,
    token: Option<&str>,
    lang: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(crate::lang::HEADER, lang);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[tokio::test]
async fn настройка_языков_и_языки_сайта_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, languages) = h.get("/api/admin/settings/languages", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(languages, json!({ "english": false, "default": "ru" }));
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(
        site["languages"],
        json!({ "list": ["ru"], "default": "ru" })
    );

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/languages",
            &owner,
            json!({ "english": false, "default": "en" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "settings.languages");

    let (status, saved) = h
        .write(
            Method::PUT,
            "/api/admin/settings/languages",
            &owner,
            json!({ "english": true, "default": "en" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved, json!({ "english": true, "default": "en" }));
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(
        site["languages"],
        json!({ "list": ["ru", "en"], "default": "en" })
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn клиент_меняет_язык_только_на_включённый() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let client = h.client(7101).await;
    let token = h.client_token(client).await;
    let (_, me) = h.get("/api/client/me", &token).await;
    assert_eq!(me["locale"], "ru");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/client/locale",
            &token,
            json!({ "locale": "en" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "client.locale");

    h.write(
        Method::PUT,
        "/api/admin/settings/languages",
        &owner,
        json!({ "english": true, "default": "ru" }),
    )
    .await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/client/locale",
            &token,
            json!({ "locale": "en" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["locale"], "en");
    let (_, me) = h.get("/api/client/me", &token).await;
    assert_eq!(me["locale"], "en");
    assert_eq!(me["client"]["locale"], "en");

    h.write(
        Method::PUT,
        "/api/admin/settings/languages",
        &owner,
        json!({ "english": false, "default": "ru" }),
    )
    .await;
    let (_, me) = h.get("/api/client/me", &token).await;
    assert_eq!(me["locale"], "ru");
    assert_eq!(me["client"]["locale"], "en");
    h.db.cleanup().await;
}

#[tokio::test]
async fn новый_клиент_с_сайта_получает_язык_страницы_и_ошибки_на_нём() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let register = |email: &str| json!({ "email": email, "password": "очень-длинный-пароль-1" });
    let (status, body) = send_in(
        &h,
        Method::POST,
        "/api/client/auth/email/register",
        None,
        "en",
        Some(register("ann@example.com")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ann = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(sn_domain::client::IdentityKind::Email, "ann@example.com")
            .await
            .unwrap()
            .unwrap()
    };
    assert_eq!(ann.locale, sn_domain::client::Locale::Ru);

    h.write(
        Method::PUT,
        "/api/admin/settings/languages",
        &owner,
        json!({ "english": true, "default": "ru" }),
    )
    .await;
    let (status, body) = send_in(
        &h,
        Method::POST,
        "/api/client/auth/email/register",
        None,
        "en",
        Some(register("bob@example.com")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let bob = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.clients()
            .find_by_identity(sn_domain::client::IdentityKind::Email, "bob@example.com")
            .await
            .unwrap()
            .unwrap()
    };
    assert_eq!(bob.locale, sn_domain::client::Locale::En);

    let (status, body) = send_in(
        &h,
        Method::POST,
        "/api/client/auth/email/register",
        None,
        "en",
        Some(register("bob@example.com")),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["message"],
        "This email is already registered. Sign in with your password."
    );
    let (_, body) = send_in(
        &h,
        Method::POST,
        "/api/client/auth/email/register",
        None,
        "ru",
        Some(register("bob@example.com")),
    )
    .await;
    assert_eq!(
        body["error"]["message"],
        "Эта почта уже зарегистрирована: войдите с паролем"
    );
    h.db.cleanup().await;
}
