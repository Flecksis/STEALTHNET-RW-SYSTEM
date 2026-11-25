use super::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use sn_modules::catalog::oauth::OauthModule;
use sn_services::oauth::{Endpoints, OauthEndpoints, protocol};
use std::sync::atomic::{AtomicU32, Ordering};
use wiremock::matchers::{body_string_contains, method, path, path_regex};
use wiremock::{Mock, MockServer};

const IDP_KEY: &str = include_str!("../../../sn-services/src/oauth/testdata/idp_key.pem");
const IDP_N: &str = include_str!("../../../sn-services/src/oauth/testdata/idp_key.n");
const APPLE_KEY: &str = include_str!("../../../sn-services/src/oauth/testdata/apple_key.p8");
const GOOGLE_ID: &str = "1234-test.apps.googleusercontent.com";
const SERVICES_ID: &str = "com.example.signin";
const PANEL: &str = "https://panel.example.com";
const KID: &str = "idp-1";

static CODES: AtomicU32 = AtomicU32::new(0);

struct OauthHttp {
    http: Http,
    google: MockServer,
    apple: MockServer,
}

async fn jwks(server: &MockServer) -> Endpoints {
    Mock::given(method("GET"))
        .and(path_regex("^/keys/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
            "keys": [{ "kty": "RSA", "kid": KID, "use": "sig", "alg": "RS256", "n": IDP_N.trim(), "e": "AQAB" }]
        })))
        .mount(server)
        .await;
    Endpoints {
        jwks: format!("{}/keys/{}", server.uri(), Uuid::new_v4().simple()),
        ..Endpoints::at(&server.uri())
    }
}

async fn oauth_http() -> Option<OauthHttp> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(
            ModuleRegistry::new(vec![
                Arc::new(OauthModule),
                Arc::new(sn_modules::catalog::antibot::AntibotModule),
            ])
            .unwrap(),
        ),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks {
            public_url: Some(PANEL.into()),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    let google = MockServer::start().await;
    let apple = MockServer::start().await;
    services.set_oauth_endpoints(OauthEndpoints {
        google: jwks(&google).await,
        apple: jwks(&apple).await,
    });
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    let http = Http {
        db,
        state: AppState::new(
            ServerConfig {
                public_url: Some(PANEL.into()),
                ..ServerConfig::default()
            },
            services,
        ),
        clock,
        remna,
    };
    let owner = http.owner().await;
    let (status, body) = http
        .write(
            Method::PUT,
            "/api/admin/modules/oauth/config",
            &owner,
            json!({ "config": {
                "google_client_id": GOOGLE_ID,
                "google_client_secret": "GOCSPX-test",
                "apple_services_id": SERVICES_ID,
                "apple_team_id": "TEAM123456",
                "apple_key_id": "KEY1234567",
                "apple_private_key": APPLE_KEY,
            }}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["config"]["apple_team_id"], "TEAM123456");
    assert!(
        body["config"]
            .get("apple_private_key")
            .is_none_or(|v| v.as_str() != Some(APPLE_KEY))
    );
    let (status, body) = http
        .write(
            Method::PATCH,
            "/api/admin/modules/oauth",
            &owner,
            json!({ "enabled": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["active"], true);
    assert_eq!(body["warning"], Value::Null);
    Some(OauthHttp {
        http,
        google,
        apple,
    })
}

struct Raw {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Raw {
    fn cookies(&self) -> Vec<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect()
    }

    fn cookie(&self, name: &str) -> Option<String> {
        self.cookies()
            .into_iter()
            .find(|c| c.starts_with(&format!("{name}=")))
    }

    fn location(&self) -> Option<&str> {
        self.headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
    }
}

fn value_of(cookie: &str) -> String {
    cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}

fn query(url: &str, key: &str) -> Option<String> {
    url.split_once('?')?
        .1
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
}

impl OauthHttp {
    async fn raw(
        &self,
        method: Method,
        uri: &str,
        headers: &[(&str, String)],
        body: Option<(&str, String)>,
    ) -> Raw {
        let mut builder = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, value.as_str());
        }
        let request = match body {
            Some((kind, body)) => builder
                .header(header::CONTENT_TYPE, kind)
                .body(Body::from(body))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let response = router(self.http.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        Raw {
            status,
            headers,
            body: body_json(response).await,
        }
    }

    async fn reply(
        &self,
        server: &MockServer,
        audience: &str,
        browser: &str,
        code: &str,
        sub: &str,
        extra: Value,
    ) {
        let now = Utc::now().timestamp();
        let mut claims = json!({
            "iss": server.uri(), "aud": audience, "sub": sub, "nonce": protocol::nonce(browser),
            "iat": now, "exp": now + 600,
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            claims[key] = value;
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(KID.into());
        let token = jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(IDP_KEY.as_bytes()).unwrap(),
        )
        .unwrap();
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains(format!("code={code}")))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(json!({ "id_token": token })),
            )
            .mount(server)
            .await;
    }
}

fn code() -> String {
    format!("srv-{:08}-end", CODES.fetch_add(1, Ordering::Relaxed))
}

#[tokio::test]
async fn вход_через_google_и_apple_по_http_с_куками_и_переадресацией() {
    let Some(h) = oauth_http().await else { return };
    let (_, _, site) = h
        .http
        .send(Method::GET, "/api/public/site", None, None)
        .await;
    assert_eq!(site["oauth"], json!({ "google": true, "apple": true }));
    let started = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/google",
            &[("origin", PANEL.into())],
            Some(("application/json", json!({ "ref": "FRIEND42" }).to_string())),
        )
        .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let url = started.body["url"].as_str().unwrap().to_string();
    assert!(url.starts_with(&format!("{}/authorize?", h.google.uri())));
    let cookie = started.cookie("sn_oauth").unwrap();
    assert!(cookie.contains("Path=/api/client/auth/oauth"), "{cookie}");
    assert!(
        cookie.contains("HttpOnly")
            && cookie.contains("SameSite=Lax")
            && cookie.contains("Max-Age=600"),
        "{cookie}"
    );
    assert!(cookie.contains("Secure"), "{cookie}");
    assert!(started.body.get("browser").is_none());
    let browser = value_of(&cookie);
    let state = query(&url, "state").unwrap();
    let google_code = code();
    h.reply(
        &h.google,
        GOOGLE_ID,
        &browser,
        &google_code,
        "9001",
        json!({ "email": "neo@gmail.com", "email_verified": true }),
    )
    .await;
    let back = h
        .raw(
            Method::GET,
            &format!("/api/client/auth/oauth/google/callback?state={state}&code={google_code}&scope=openid"),
            &[("cookie", format!("sn_oauth={browser}"))],
            None,
        )
        .await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(back.location(), Some("/cabinet/login?oauth=google"));
    let evil = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/finish",
            &[
                ("cookie", format!("sn_oauth={browser}")),
                ("origin", "https://evil.example".into()),
                ("sec-fetch-site", "cross-site".into()),
            ],
            None,
        )
        .await;
    assert_eq!(evil.status, StatusCode::FORBIDDEN);
    let done = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/finish",
            &[
                ("cookie", format!("sn_oauth={browser}")),
                ("origin", PANEL.into()),
            ],
            None,
        )
        .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(done.body["status"], "confirmed");
    assert_eq!(done.body["login"]["created"], true);
    let session = done.cookie("sn_client").unwrap();
    assert!(session.contains("SameSite=Lax"));
    let cleared = done.cookie("sn_oauth").unwrap();
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    let token = value_of(&session);
    let (status, me) = h.http.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], "neo@gmail.com");
    let again = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/finish",
            &[("cookie", format!("sn_oauth={browser}"))],
            None,
        )
        .await;
    assert_eq!(again.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(again.body["error"]["code"], "oauth.no_request");

    let (status, logins) = h.http.get("/api/client/security/logins", &token).await;
    assert_eq!(status, StatusCode::OK, "{logins}");
    assert_eq!(logins["can_link"], json!(["apple"]));
    let link = h
        .raw(
            Method::POST,
            "/api/client/security/logins/apple",
            &[("authorization", format!("Bearer {token}"))],
            None,
        )
        .await;
    assert_eq!(link.status, StatusCode::OK, "{}", link.body);
    let cookie = link.cookie("sn_oauth").unwrap();
    assert!(
        cookie.contains("SameSite=None") && cookie.contains("Secure"),
        "{cookie}"
    );
    let browser = value_of(&cookie);
    let url = link.body["url"].as_str().unwrap().to_string();
    assert_eq!(query(&url, "response_mode").as_deref(), Some("form_post"));
    let state = query(&url, "state").unwrap();
    let apple_code = code();
    h.reply(
        &h.apple,
        SERVICES_ID,
        &browser,
        &apple_code,
        "000111.apple",
        json!({ "email": "relay1@privaterelay.appleid.com", "email_verified": "true", "is_private_email": "true" }),
    )
    .await;
    let back = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/apple/callback",
            &[
                ("cookie", format!("sn_oauth={browser}")),
                ("origin", "https://appleid.apple.com".into()),
                ("sec-fetch-site", "cross-site".into()),
            ],
            Some((
                "application/x-www-form-urlencoded",
                format!("state={state}&code={apple_code}&user=%7B%22name%22%3A%7B%22firstName%22%3A%22Neo%22%7D%7D"),
            )),
        )
        .await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(back.location(), Some("/cabinet/security?oauth=apple"));
    let linked = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/finish",
            &[("cookie", format!("sn_oauth={browser}"))],
            None,
        )
        .await;
    assert_eq!(linked.status, StatusCode::OK, "{}", linked.body);
    assert_eq!(
        linked.body,
        json!({ "status": "linked", "provider": "apple" })
    );
    let (status, logins) = h.http.get("/api/client/security/logins", &token).await;
    assert_eq!(status, StatusCode::OK);
    let kinds: Vec<&str> = logins["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["google", "email", "apple"]);
    let apple = logins["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "apple")
        .unwrap();
    assert_eq!(apple["account"], "почта скрыта");
    assert_eq!(apple["removable"], true);
    let (status, _, after) = h
        .http
        .send(
            Method::DELETE,
            "/api/client/security/logins/google",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert!(
        after["methods"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["kind"] != "google")
    );
    let (status, _, refused) = h
        .http
        .send(
            Method::DELETE,
            "/api/client/security/logins/apple",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "oauth.last_method");
    let (status, _, _) = h
        .http
        .send(
            Method::DELETE,
            "/api/client/security/logins/github",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ответ_без_куки_и_выключенный_модуль_не_пускают() {
    let Some(h) = oauth_http().await else { return };
    let started = h
        .raw(Method::POST, "/api/client/auth/oauth/apple", &[], None)
        .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let url = started.body["url"].as_str().unwrap().to_string();
    let state = query(&url, "state").unwrap();
    let back = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/apple/callback",
            &[],
            Some((
                "application/x-www-form-urlencoded",
                format!("state={state}&code=stolen"),
            )),
        )
        .await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(back.location(), Some("/cabinet/login?oauth=apple"));
    let finish = h
        .raw(Method::POST, "/api/client/auth/oauth/finish", &[], None)
        .await;
    assert_eq!(finish.body["error"]["code"], "oauth.no_request");
    let junk = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/apple/callback",
            &[],
            Some(("application/json", "{}".into())),
        )
        .await;
    assert_eq!(junk.status, StatusCode::SEE_OTHER);
    let owner = h.http.owner().await;
    let (status, _) = h
        .http
        .write(
            Method::PATCH,
            "/api/admin/modules/oauth",
            &owner,
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, site) = h
        .http
        .send(Method::GET, "/api/public/site", None, None)
        .await;
    assert_eq!(site["oauth"], json!({ "google": false, "apple": false }));
    let refused = h
        .raw(Method::POST, "/api/client/auth/oauth/google", &[], None)
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.body["error"]["code"], "oauth.not_configured");
}

#[tokio::test]
async fn старт_входа_через_google_запоминает_метку_сайта() {
    let Some(h) = oauth_http().await else { return };
    let started = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/google",
            &[("origin", PANEL.into())],
            Some((
                "application/json",
                json!({ "ref": "FRIEND42", "utm": { "source": "tiktok", "campaign": "osen", "term": "bad value" } })
                    .to_string(),
            )),
        )
        .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let broken = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/google",
            &[("origin", PANEL.into())],
            Some(("application/json", json!({ "utm": ["s_vk"] }).to_string())),
        )
        .await;
    assert_eq!(broken.status, StatusCode::OK, "{}", broken.body);
    let mut conn = h.http.services().store().conn().await.unwrap();
    let saved: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT referral, utm FROM oauth_requests ORDER BY id")
            .fetch_all(conn.conn())
            .await
            .unwrap();
    assert_eq!(
        saved,
        vec![
            (Some("FRIEND42".into()), Some("s_tiktok__k_osen".into())),
            (None, None)
        ]
    );
}

#[tokio::test]
async fn отказ_с_капчей_не_снимает_куку_входа_а_с_ответом_аккаунт_создаётся() {
    let Some(h) = oauth_http().await else { return };
    let services = h.http.services();
    super::captcha::antibot_on(
        &h.http,
        json!({
            "site_minute": 1,
            "captcha": "turnstile",
            "captcha_site_key": "0x4AAAAAAAsite",
            "captcha_secret": super::captcha::SECRET,
            "captcha_signup": false,
        }),
    )
    .await;
    let _verify = super::captcha::siteverify(&h.http).await;
    let (_, _, site) = h
        .http
        .send(Method::GET, "/api/public/site", None, None)
        .await;
    assert_eq!(site["captcha"], Value::Null);
    assert_eq!(
        site["captcha_spare"],
        json!({ "provider": "turnstile", "site_key": "0x4AAAAAAAsite" })
    );
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO site_signups (via, created_at) VALUES ('code', now()), ('code', now() + interval '1 minute')",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let started = h
        .raw(
            Method::POST,
            "/api/client/auth/oauth/google",
            &[("origin", PANEL.into())],
            Some(("application/json", "{}".into())),
        )
        .await;
    let browser = value_of(&started.cookie("sn_oauth").unwrap());
    let state = query(started.body["url"].as_str().unwrap(), "state").unwrap();
    let google_code = code();
    h.reply(
        &h.google,
        GOOGLE_ID,
        &browser,
        &google_code,
        "9101",
        json!({ "email": "wave@gmail.com", "email_verified": true }),
    )
    .await;
    h.raw(
        Method::GET,
        &format!("/api/client/auth/oauth/google/callback?state={state}&code={google_code}"),
        &[("cookie", format!("sn_oauth={browser}"))],
        None,
    )
    .await;
    let finish = |body: Value| {
        let h = &h;
        let browser = browser.clone();
        async move {
            h.raw(
                Method::POST,
                "/api/client/auth/oauth/finish",
                &[
                    ("cookie", format!("sn_oauth={browser}")),
                    ("origin", PANEL.into()),
                ],
                Some(("application/json", body.to_string())),
            )
            .await
        }
    };
    for (body, code) in [
        (json!({}), "antibot.captcha"),
        (json!({ "captcha": "bad" }), "antibot.failed"),
    ] {
        let asked = finish(body).await;
        assert_eq!(asked.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(asked.body["error"]["code"], code);
        assert!(asked.cookie("sn_oauth").is_none(), "кука входа остаётся");
        assert!(asked.cookie("sn_client").is_none());
    }
    let done = finish(json!({ "captcha": "ok:signup" })).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(done.body["login"]["created"], true);
    assert!(done.cookie("sn_client").is_some());
    assert!(done.cookie("sn_oauth").unwrap().contains("Max-Age=0"));
}
