mod exchange;

use super::*;
use crate::settings::ModuleConfigUpdate;
use crate::{EnvFallbacks, FixedClock, ServiceDeps};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use sn_domain::access::ClientChannel;
use sn_domain::client::{Locale, TelegramProfile};
use sn_domain::ids::AdminId;
use sn_domain::settings::keys;
use sn_modules::ModuleRegistry;
use sn_modules::catalog::oauth::OauthModule;
use sn_modules::catalog::utm::{UtmModule, for_client as origin_of};
use sn_payments::ProviderRegistry;
use sn_store::testing::{TestDb, test_store};
use sn_store::{SecretBox, SessionChannel};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration as Delay;
use wiremock::matchers::{body_string_contains, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const IDP_KEY: &str = include_str!("testdata/idp_key.pem");
const IDP_N: &str = include_str!("testdata/idp_key.n");
const STRANGER_KEY: &str = include_str!("testdata/stranger_key.pem");
const APPLE_KEY: &str = include_str!("testdata/apple_key.p8");
const APPLE_PUBLIC: &str = include_str!("testdata/apple_key.pub.pem");
const GOOGLE_ID: &str = "1234-test.apps.googleusercontent.com";
const GOOGLE_SECRET: &str = "GOCSPX-test-secret";
const SERVICES_ID: &str = "com.example.signin";
const TEAM_ID: &str = "TEAM123456";
const KEY_ID: &str = "KEY1234567";
const PANEL: &str = "https://panel.example.com";
const KID: &str = "idp-1";
const PASSWORD: &str = "очень-надёжный-пароль";

static CODES_ISSUED: AtomicU32 = AtomicU32::new(0);

type Forge<'a> = Box<dyn Fn(&Flow) -> String + 'a>;

struct Harness {
    db: TestDb,
    services: Services,
    clock: Arc<FixedClock>,
    google: MockServer,
    apple: MockServer,
    admin: AdminId,
}

fn meta() -> RequestMeta {
    RequestMeta {
        ip: Some("203.0.113.7".into()),
        user_agent: Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) Safari/605.1.15".into()),
    }
}

fn full_config() -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({
        "google_client_id": GOOGLE_ID,
        "google_client_secret": GOOGLE_SECRET,
        "apple_services_id": SERVICES_ID,
        "apple_team_id": TEAM_ID,
        "apple_key_id": KEY_ID,
        "apple_private_key": APPLE_KEY,
    })
    .as_object()
    .cloned()
    .unwrap()
}

async fn jwks(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path_regex("^/keys/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "keys": [{ "kty": "RSA", "kid": KID, "use": "sig", "alg": "RS256", "n": IDP_N.trim(), "e": "AQAB" }]
        })))
        .mount(server)
        .await;
}

async fn harness() -> Option<Harness> {
    harness_with(Vec::new()).await
}

async fn harness_with(extra: Vec<Arc<dyn sn_modules::Module>>) -> Option<Harness> {
    let mut modules: Vec<Arc<dyn sn_modules::Module>> =
        vec![Arc::new(OauthModule), Arc::new(UtmModule)];
    modules.extend(extra);
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(modules).unwrap()),
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
    jwks(&google).await;
    jwks(&apple).await;
    let unique = |server: &MockServer| Endpoints {
        jwks: format!("{}/keys/{}", server.uri(), uuid::Uuid::new_v4().simple()),
        ..Endpoints::at(&server.uri())
    };
    services.set_oauth_endpoints(OauthEndpoints {
        google: unique(&google),
        apple: unique(&apple),
    });
    let admin = services
        .auth()
        .upsert_owner("owner@example.com", PASSWORD)
        .await
        .unwrap()
        .0;
    let ops = services.settings_ops();
    ops.update_module_config(
        ModuleId::Oauth,
        &ModuleConfigUpdate {
            config: full_config(),
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Oauth, true, admin)
        .await
        .unwrap();
    Some(Harness {
        db,
        services,
        clock,
        google,
        apple,
        admin,
    })
}

fn sign(key: &str, kid: &str, claims: &serde_json::Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.into());
    jsonwebtoken::encode(
        &header,
        claims,
        &EncodingKey::from_rsa_pem(key.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn claims(
    server: &MockServer,
    audience: &str,
    browser: &str,
    sub: &str,
    extra: serde_json::Value,
) -> serde_json::Value {
    let now = Utc::now().timestamp();
    let mut claims = serde_json::json!({
        "iss": server.uri(),
        "aud": audience,
        "sub": sub,
        "nonce": protocol::nonce(browser),
        "iat": now,
        "exp": now + 600,
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        claims[key] = value;
    }
    claims
}

fn query(url: &str, key: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

struct Flow {
    start: OauthStart,
    state: String,
    code: String,
}

impl Flow {
    fn params(&self, user: Option<&str>) -> CallbackParams {
        CallbackParams {
            code: Some(self.code.clone()),
            state: Some(self.state.clone()),
            error: None,
            user: user.map(str::to_string),
        }
    }
}

impl Harness {
    fn server(&self, provider: Provider) -> &MockServer {
        match provider {
            Provider::Google => &self.google,
            Provider::Apple => &self.apple,
        }
    }

    fn ops(&self) -> OauthOps<'_> {
        self.services.oauth()
    }

    fn flow(start: OauthStart) -> Flow {
        let state = query(&start.url, "state").unwrap();
        let code = format!(
            "code-{:08}-end",
            CODES_ISSUED.fetch_add(1, Ordering::Relaxed)
        );
        Flow { start, state, code }
    }

    async fn login_flow(&self, provider: Provider) -> Flow {
        Self::flow(
            self.ops()
                .start_login(provider, None, &meta())
                .await
                .unwrap(),
        )
    }

    async fn reply_with(&self, provider: Provider, flow: &Flow, token: String) {
        self.reply_after(provider, flow, token, Delay::ZERO).await;
    }

    async fn reply_after(&self, provider: Provider, flow: &Flow, token: String, delay: Delay) {
        let mut mock = Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains(format!("code={}", flow.code)));
        if provider == Provider::Google {
            mock = mock
                .and(body_string_contains(format!(
                    "code_verifier={}",
                    protocol::code_verifier(&flow.start.browser)
                )))
                .and(body_string_contains(format!(
                    "client_secret={GOOGLE_SECRET}"
                )));
        }
        mock.respond_with(ResponseTemplate::new(200).set_delay(delay).set_body_json(
            serde_json::json!({
                "access_token": "at", "token_type": "Bearer", "expires_in": 3599, "id_token": token
            }),
        ))
        .mount(self.server(provider))
        .await;
    }

    fn token(
        &self,
        provider: Provider,
        flow: &Flow,
        sub: &str,
        extra: serde_json::Value,
    ) -> String {
        let audience = match provider {
            Provider::Google => GOOGLE_ID,
            Provider::Apple => SERVICES_ID,
        };
        sign(
            IDP_KEY,
            KID,
            &claims(
                self.server(provider),
                audience,
                &flow.start.browser,
                sub,
                extra,
            ),
        )
    }

    async fn reply(&self, provider: Provider, flow: &Flow, sub: &str, extra: serde_json::Value) {
        let token = self.token(provider, flow, sub, extra);
        self.reply_with(provider, flow, token).await;
    }

    async fn back(&self, provider: Provider, flow: &Flow, user: Option<&str>) -> CallbackOutcome {
        self.ops()
            .callback(provider, &flow.params(user), Some(&flow.start.browser))
            .await
            .unwrap()
    }

    async fn finish(&self, flow: &Flow) -> ServiceResult<OauthFinish> {
        self.ops().finish(Some(&flow.start.browser), &meta()).await
    }

    async fn sign_in(
        &self,
        provider: Provider,
        sub: &str,
        extra: serde_json::Value,
        user: Option<&str>,
    ) -> ServiceResult<OauthFinish> {
        let flow = self.login_flow(provider).await;
        self.reply(provider, &flow, sub, extra).await;
        self.back(provider, &flow, user).await;
        self.finish(&flow).await
    }

    async fn identities(&self, client: ClientId) -> Vec<Identity> {
        let mut conn = self.services.store().conn().await.unwrap();
        conn.clients().identities(client).await.unwrap()
    }

    async fn clients_count(&self) -> i64 {
        let mut conn = self.services.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT count(*) FROM clients")
            .fetch_one(conn.conn())
            .await
            .unwrap()
    }

    async fn email_client(&self, email: &str, verified: bool, password: bool) -> ClientId {
        let mut tx = self.services.store().begin().await.unwrap();
        let client = tx
            .clients()
            .create(&NewClient {
                locale: Locale::Ru,
                registration_source: RegistrationSource::Web,
                note: None,
            })
            .await
            .unwrap();
        tx.clients()
            .add_identity(
                client.id,
                &NewIdentity::new(IdentityKind::Email, email, verified).unwrap(),
            )
            .await
            .unwrap();
        if password {
            sqlx::query("INSERT INTO client_passwords (client_id, password_hash) VALUES ($1, 'x')")
                .bind(client.id.0)
                .execute(tx.conn())
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        client.id
    }

    async fn telegram_client(&self, id: i64) -> ClientId {
        self.services
            .clients()
            .ensure_telegram(
                &TelegramProfile {
                    id,
                    username: Some("morozov_a".into()),
                    first_name: Some("Алексей".into()),
                    last_name: None,
                    language_code: Some("ru".into()),
                    is_premium: false,
                },
                RegistrationSource::Bot,
                None,
            )
            .await
            .unwrap()
            .0
            .id
    }

    async fn cabinet(&self, client: ClientId) -> ClientPrincipal {
        self.services
            .auth()
            .client_session_for(client, SessionChannel::Cabinet, &meta(), false)
            .await
            .unwrap()
            .principal
    }

    async fn link(
        &self,
        principal: &ClientPrincipal,
        provider: Provider,
        sub: &str,
        extra: serde_json::Value,
    ) -> ServiceResult<OauthFinish> {
        let flow = Self::flow(self.ops().start_link(principal, provider, &meta()).await?);
        self.reply(provider, &flow, sub, extra).await;
        let outcome = self.back(provider, &flow, None).await;
        assert_eq!(outcome.purpose, Purpose::Link);
        self.finish(&flow).await
    }
}

fn confirmed(result: ServiceResult<OauthFinish>) -> ClientLogin {
    match result.unwrap() {
        OauthFinish::Confirmed { login } => login,
        other => panic!("ожидался вход, пришло {other:?}"),
    }
}

fn refused(result: ServiceResult<OauthFinish>) -> &'static str {
    result.unwrap_err().code()
}

#[tokio::test]
async fn вход_через_google_создаёт_клиента_и_открывает_кабинет() {
    let Some(h) = harness().await else { return };
    let start = h
        .ops()
        .start_login(Provider::Google, Some("friend42"), &meta())
        .await
        .unwrap();
    let url = reqwest::Url::parse(&start.url).unwrap();
    assert!(
        start
            .url
            .starts_with(&format!("{}/authorize?", h.google.uri()))
    );
    assert_eq!(query(&start.url, "client_id").as_deref(), Some(GOOGLE_ID));
    assert_eq!(
        query(&start.url, "redirect_uri").as_deref(),
        Some("https://panel.example.com/api/client/auth/oauth/google/callback")
    );
    assert_eq!(
        query(&start.url, "scope").as_deref(),
        Some("openid email profile")
    );
    assert_eq!(
        query(&start.url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    assert_eq!(
        query(&start.url, "code_challenge"),
        Some(protocol::code_challenge(&protocol::code_verifier(
            &start.browser
        )))
    );
    assert_eq!(
        query(&start.url, "nonce"),
        Some(protocol::nonce(&start.browser))
    );
    assert_eq!(query(&start.url, "response_type").as_deref(), Some("code"));
    assert!(url.query_pairs().all(|(k, _)| k != "response_mode"));
    let flow = Harness::flow(start);
    let profile = serde_json::json!({
        "email": "Anna.Petrova@gmail.com",
        "email_verified": true,
        "name": "Анна Петрова",
        "given_name": "Анна",
        "family_name": "Петрова",
        "azp": GOOGLE_ID,
    });
    h.reply(Provider::Google, &flow, "1084", profile.clone())
        .await;
    let outcome = h.back(Provider::Google, &flow, None).await;
    assert_eq!(outcome.purpose, Purpose::Login);
    let login = confirmed(h.finish(&flow).await);
    assert!(login.created);
    assert_eq!(login.principal.channel, ClientChannel::Cabinet);
    let client = login.principal.client_id;
    let identities = h.identities(client).await;
    let google = identities
        .iter()
        .find(|i| i.kind == IdentityKind::Google)
        .unwrap();
    assert_eq!(google.value, "1084");
    assert_eq!(google.meta["email"], "anna.petrova@gmail.com");
    assert_eq!(google.meta["first_name"], "Анна");
    let email = identities
        .iter()
        .find(|i| i.kind == IdentityKind::Email)
        .unwrap();
    assert_eq!(email.value, "anna.petrova@gmail.com");
    assert!(email.verified);
    {
        let mut conn = h.services.store().conn().await.unwrap();
        let param: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'start_param' FROM domain_events WHERE client_id = $1 AND payload->>'type' = 'client_registered'",
        )
        .bind(client.0)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        assert_eq!(param.as_deref(), Some("ref_FRIEND42"));
        let source: String =
            sqlx::query_scalar("SELECT registration_source FROM clients WHERE id = $1")
                .bind(client.0)
                .fetch_one(conn.conn())
                .await
                .unwrap();
        assert_eq!(source, "web");
    }
    assert_eq!(refused(h.finish(&flow).await), "oauth.no_request");
    let again = confirmed(h.sign_in(Provider::Google, "1084", profile, None).await);
    assert_eq!(again.principal.client_id, client);
    assert!(!again.created);
    assert_eq!(h.clients_count().await, 1);
    let keys_fetched = h
        .google
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().starts_with("/keys/"))
        .count();
    assert_eq!(keys_fetched, 1);
}

#[tokio::test]
async fn вход_через_apple_принимает_скрытую_почту_и_имя_первого_входа() {
    let Some(h) = harness().await else { return };
    let start = h
        .ops()
        .start_login(Provider::Apple, None, &meta())
        .await
        .unwrap();
    assert!(
        start
            .url
            .starts_with(&format!("{}/authorize?", h.apple.uri()))
    );
    assert_eq!(
        query(&start.url, "response_mode").as_deref(),
        Some("form_post")
    );
    assert_eq!(query(&start.url, "scope").as_deref(), Some("name email"));
    assert_eq!(query(&start.url, "client_id").as_deref(), Some(SERVICES_ID));
    assert_eq!(
        query(&start.url, "redirect_uri").as_deref(),
        Some("https://panel.example.com/api/client/auth/oauth/apple/callback")
    );
    assert_eq!(query(&start.url, "code_challenge"), None);
    let flow = Harness::flow(start);
    let token = sign(
        IDP_KEY,
        KID,
        &claims(
            &h.apple,
            SERVICES_ID,
            &flow.start.browser,
            "001234.abcdef.0987",
            serde_json::json!({
                "email": "x7k2q9@privaterelay.appleid.com",
                "email_verified": "true",
                "is_private_email": "true",
            }),
        ),
    );
    let expected_code = flow.code.clone();
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!("code={}", flow.code)))
        .respond_with(move |request: &Request| {
            let form: std::collections::HashMap<String, String> = reqwest::Url::parse(&format!(
                "http://form/?{}",
                String::from_utf8_lossy(&request.body)
            ))
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
            let secret = form.get("client_secret").cloned().unwrap_or_default();
            let header = jsonwebtoken::decode_header(&secret).unwrap();
            let mut validation = Validation::new(Algorithm::ES256);
            validation.set_audience(&["https://appleid.apple.com"]);
            validation.set_issuer(&[TEAM_ID]);
            let decoded = jsonwebtoken::decode::<serde_json::Value>(
                &secret,
                &DecodingKey::from_ec_pem(APPLE_PUBLIC.as_bytes()).unwrap(),
                &validation,
            );
            let valid = decoded.is_ok_and(|d| d.claims["sub"] == SERVICES_ID)
                && header.kid.as_deref() == Some(KEY_ID)
                && form.get("client_id").map(String::as_str) == Some(SERVICES_ID)
                && form.get("code") == Some(&expected_code)
                && form.get("grant_type").map(String::as_str) == Some("authorization_code")
                && !form.contains_key("code_verifier");
            if valid {
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id_token": token.clone() }))
            } else {
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({ "error": "invalid_client" }))
            }
        })
        .mount(&h.apple)
        .await;
    h.back(
        Provider::Apple,
        &flow,
        Some(r#"{"name":{"firstName":"Мария","lastName":"Ким"},"email":"x7k2q9@privaterelay.appleid.com"}"#),
    )
    .await;
    let login = confirmed(h.finish(&flow).await);
    assert!(login.created);
    let client = login.principal.client_id;
    let identities = h.identities(client).await;
    let apple = identities
        .iter()
        .find(|i| i.kind == IdentityKind::Apple)
        .unwrap();
    assert_eq!(apple.value, "001234.abcdef.0987");
    assert_eq!(apple.meta["first_name"], "Мария");
    assert_eq!(apple.meta["last_name"], "Ким");
    assert_eq!(apple.meta["private_email"], true);
    let email = identities
        .iter()
        .find(|i| i.kind == IdentityKind::Email)
        .unwrap();
    assert_eq!(email.value, "x7k2q9@privaterelay.appleid.com");
    assert!(email.verified);
    let again = confirmed(
        h.sign_in(
            Provider::Apple,
            "001234.abcdef.0987",
            serde_json::json!({ "email": "x7k2q9@privaterelay.appleid.com", "email_verified": true, "is_private_email": true }),
            None,
        )
        .await,
    );
    assert_eq!(again.principal.client_id, client);
    let apple = h
        .identities(client)
        .await
        .into_iter()
        .find(|i| i.kind == IdentityKind::Apple)
        .unwrap();
    assert_eq!(apple.meta["first_name"], "Мария");
    let methods = h.ops().logins(client).await.unwrap();
    let shown = methods
        .methods
        .iter()
        .find(|m| m.kind == IdentityKind::Apple)
        .unwrap();
    assert_eq!(shown.account, "почта скрыта");
}

#[tokio::test]
async fn подтверждённая_почта_привязывает_вход_к_существующему_клиенту() {
    let Some(h) = harness().await else { return };
    let owner = h.email_client("anna@gmail.com", true, false).await;
    let login = confirmed(
        h.sign_in(
            Provider::Google,
            "2001",
            serde_json::json!({ "email": "anna@gmail.com", "email_verified": true }),
            None,
        )
        .await,
    );
    assert_eq!(login.principal.client_id, owner);
    assert!(!login.created);
    assert!(
        h.identities(owner)
            .await
            .iter()
            .any(|i| i.kind == IdentityKind::Google && i.value == "2001")
    );
    let workspace = h.email_client("ceo@corp.example", true, false).await;
    let login = confirmed(
        h.sign_in(
            Provider::Google,
            "2002",
            serde_json::json!({ "email": "ceo@corp.example", "email_verified": true, "hd": "corp.example" }),
            None,
        )
        .await,
    );
    assert_eq!(login.principal.client_id, workspace);
    let icloud = h.email_client("maria@icloud.com", true, false).await;
    let login = confirmed(
        h.sign_in(
            Provider::Apple,
            "000777.apple",
            serde_json::json!({ "email": "maria@icloud.com", "email_verified": "true" }),
            None,
        )
        .await,
    );
    assert_eq!(login.principal.client_id, icloud);
    assert_eq!(h.clients_count().await, 3);
    let mut conn = h.services.store().conn().await.unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'client.oauth_linked' AND details->>'via' = 'email'",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(audits, 3);
    let notices: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE payload->>'kind' = 'notify' AND payload->>'template' = 'oauth_linked' ORDER BY id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(notices.len(), 3);
    assert_eq!(notices[0]["client_id"], owner.0);
    assert_eq!(notices[0]["params"]["provider"], "Google");
    assert_eq!(notices[0]["params"]["account"], "anna@gmail.com");
    assert_eq!(notices[2]["params"]["provider"], "Apple");
}

#[tokio::test]
async fn чужая_почта_без_подтверждения_не_даёт_захватить_аккаунт() {
    let Some(h) = harness().await else { return };
    let squatted = h.email_client("boris@gmail.com", false, true).await;
    let result = h
        .sign_in(
            Provider::Google,
            "3001",
            serde_json::json!({ "email": "boris@gmail.com", "email_verified": true }),
            None,
        )
        .await;
    let error = result.unwrap_err();
    assert_eq!(error.code(), "oauth.email_taken");
    assert!(error.to_string().contains("boris@gmail.com"));
    assert!(error.to_string().contains("«Безопасность»"));
    assert!(
        !h.identities(squatted)
            .await
            .iter()
            .any(|i| i.kind == IdentityKind::Google)
    );
    let verified_elsewhere = h.email_client("boss@company.example", true, false).await;
    assert_eq!(
        refused(
            h.sign_in(
                Provider::Google,
                "3002",
                serde_json::json!({ "email": "boss@company.example", "email_verified": true }),
                None,
            )
            .await
        ),
        "oauth.email_taken"
    );
    let gmail_on_apple = h.email_client("gleb@gmail.com", true, false).await;
    assert_eq!(
        refused(
            h.sign_in(
                Provider::Apple,
                "000555.apple",
                serde_json::json!({ "email": "gleb@gmail.com", "email_verified": "true" }),
                None,
            )
            .await
        ),
        "oauth.email_taken"
    );
    let unverified = confirmed(
        h.sign_in(
            Provider::Google,
            "3003",
            serde_json::json!({ "email": "gleb@gmail.com", "email_verified": false }),
            None,
        )
        .await,
    );
    assert!(unverified.created);
    assert_ne!(unverified.principal.client_id, gmail_on_apple);
    assert!(
        !h.identities(unverified.principal.client_id)
            .await
            .iter()
            .any(|i| i.kind == IdentityKind::Email)
    );
    h.ops()
        .unlink(&h.cabinet(gmail_on_apple).await, Provider::Google)
        .await
        .unwrap_err();
    let linked_twice = h.email_client("dina@gmail.com", true, false).await;
    confirmed(
        h.sign_in(
            Provider::Google,
            "3004",
            serde_json::json!({ "email": "dina@gmail.com", "email_verified": true }),
            None,
        )
        .await,
    );
    assert_eq!(
        refused(
            h.sign_in(
                Provider::Google,
                "3005",
                serde_json::json!({ "email": "dina@gmail.com", "email_verified": true }),
                None,
            )
            .await
        ),
        "oauth.other_account"
    );
    for client in [squatted, verified_elsewhere, gmail_on_apple, linked_twice] {
        assert!(
            h.identities(client)
                .await
                .iter()
                .filter(|i| i.kind == IdentityKind::Google)
                .count()
                <= 1
        );
    }
    h.services
        .clients()
        .block(linked_twice, Some("спам"), crate::Actor::Admin(h.admin))
        .await
        .unwrap();
    assert_eq!(
        refused(
            h.sign_in(
                Provider::Google,
                "3004",
                serde_json::json!({ "email": "dina@gmail.com", "email_verified": true }),
                None,
            )
            .await
        ),
        "oauth.blocked"
    );
}

#[tokio::test]
async fn ответ_провайдера_проверяется_строго() {
    let Some(h) = harness().await else { return };
    let now = Utc::now().timestamp();
    let google = &h.google;
    let cases: Vec<(&str, Forge<'_>)> = vec![
        (
            "чужой nonce",
            Box::new(|flow: &Flow| {
                sign(
                    IDP_KEY,
                    KID,
                    &claims(
                        google,
                        GOOGLE_ID,
                        &flow.start.browser,
                        "4001",
                        serde_json::json!({ "nonce": "attacker" }),
                    ),
                )
            }),
        ),
        (
            "чужой получатель",
            Box::new(|flow: &Flow| {
                sign(
                    IDP_KEY,
                    KID,
                    &claims(
                        google,
                        "other.apps.googleusercontent.com",
                        &flow.start.browser,
                        "4001",
                        serde_json::json!({}),
                    ),
                )
            }),
        ),
        (
            "чужой издатель",
            Box::new(|flow: &Flow| {
                sign(
                    IDP_KEY,
                    KID,
                    &claims(
                        google,
                        GOOGLE_ID,
                        &flow.start.browser,
                        "4001",
                        serde_json::json!({ "iss": "https://evil.example" }),
                    ),
                )
            }),
        ),
        (
            "просроченный",
            Box::new(move |flow: &Flow| {
                sign(
                    IDP_KEY,
                    KID,
                    &claims(
                        google,
                        GOOGLE_ID,
                        &flow.start.browser,
                        "4001",
                        serde_json::json!({ "iat": now - 7200, "exp": now - 3600 }),
                    ),
                )
            }),
        ),
        (
            "чужой ключ",
            Box::new(|flow: &Flow| {
                sign(
                    STRANGER_KEY,
                    "unknown-kid",
                    &claims(
                        google,
                        GOOGLE_ID,
                        &flow.start.browser,
                        "4001",
                        serde_json::json!({}),
                    ),
                )
            }),
        ),
        (
            "подмена ключа при том же kid",
            Box::new(|flow: &Flow| {
                sign(
                    STRANGER_KEY,
                    KID,
                    &claims(
                        google,
                        GOOGLE_ID,
                        &flow.start.browser,
                        "4001",
                        serde_json::json!({}),
                    ),
                )
            }),
        ),
        (
            "общий секрет вместо подписи",
            Box::new(|flow: &Flow| {
                let mut header = Header::new(Algorithm::HS256);
                header.kid = Some(KID.into());
                jsonwebtoken::encode(
                    &header,
                    &claims(
                        google,
                        GOOGLE_ID,
                        &flow.start.browser,
                        "4001",
                        serde_json::json!({}),
                    ),
                    &EncodingKey::from_secret(GOOGLE_SECRET.as_bytes()),
                )
                .unwrap()
            }),
        ),
        (
            "чужой azp",
            Box::new(|flow: &Flow| {
                sign(
                    IDP_KEY,
                    KID,
                    &claims(
                        google,
                        GOOGLE_ID,
                        &flow.start.browser,
                        "4001",
                        serde_json::json!({ "azp": "other" }),
                    ),
                )
            }),
        ),
    ];
    for (name, token) in cases {
        let flow = h.login_flow(Provider::Google).await;
        h.reply_with(Provider::Google, &flow, token(&flow)).await;
        h.back(Provider::Google, &flow, None).await;
        assert_eq!(refused(h.finish(&flow).await), "oauth.bad_token", "{name}");
    }
    assert_eq!(h.clients_count().await, 0);
    let flow = h.login_flow(Provider::Google).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!("code={}", flow.code)))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            serde_json::json!({ "error": "invalid_grant", "error_description": "Bad Request" }),
        ))
        .mount(&h.google)
        .await;
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(refused(h.finish(&flow).await), "oauth.refused");
    let flow = h.login_flow(Provider::Google).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!("code={}", flow.code)))
        .respond_with(ResponseTemplate::new(503))
        .mount(&h.google)
        .await;
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(refused(h.finish(&flow).await), "oauth.unavailable");
}

#[tokio::test]
async fn ответ_в_чужом_браузере_и_повтор_не_проходят() {
    let Some(h) = harness().await else { return };
    let victim = h.login_flow(Provider::Google).await;
    let attacker = h.login_flow(Provider::Google).await;
    h.reply(
        Provider::Google,
        &victim,
        "5001",
        serde_json::json!({ "email": "victim@gmail.com", "email_verified": true }),
    )
    .await;
    h.ops()
        .callback(
            Provider::Google,
            &CallbackParams {
                code: Some(victim.code.clone()),
                state: Some(victim.state.clone()),
                ..CallbackParams::default()
            },
            Some(&attacker.start.browser),
        )
        .await
        .unwrap();
    h.ops()
        .callback(
            Provider::Google,
            &CallbackParams {
                code: Some(victim.code.clone()),
                state: Some(victim.state.clone()),
                ..CallbackParams::default()
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(refused(h.finish(&attacker).await), "oauth.no_request");
    assert_eq!(refused(h.finish(&victim).await), "oauth.no_request");
    assert_eq!(h.clients_count().await, 0);
    h.back(Provider::Google, &victim, None).await;
    let login = confirmed(h.finish(&victim).await);
    assert!(login.created);
    h.back(Provider::Google, &victim, None).await;
    assert_eq!(refused(h.finish(&victim).await), "oauth.no_request");
    assert_eq!(h.clients_count().await, 1);
    let unknown = h
        .ops()
        .callback(
            Provider::Apple,
            &CallbackParams {
                code: Some("x".into()),
                state: Some(victim.state.clone()),
                ..CallbackParams::default()
            },
            Some(&victim.start.browser),
        )
        .await
        .unwrap();
    assert_eq!(unknown.purpose, Purpose::Login);
    assert_eq!(
        refused(h.ops().finish(None, &meta()).await),
        "oauth.no_request"
    );
}

#[tokio::test]
async fn отмена_и_просроченный_запрос_дают_понятный_ответ() {
    let Some(h) = harness().await else { return };
    let flow = h.login_flow(Provider::Apple).await;
    h.ops()
        .callback(
            Provider::Apple,
            &CallbackParams {
                state: Some(flow.state.clone()),
                error: Some("user_cancelled_authorize".into()),
                ..CallbackParams::default()
            },
            Some(&flow.start.browser),
        )
        .await
        .unwrap();
    let error = h.finish(&flow).await.unwrap_err();
    assert_eq!(error.code(), "oauth.canceled");
    assert_eq!(error.to_string(), "Вход через Apple отменён.");
    let flow = h.login_flow(Provider::Google).await;
    h.clock.set(Utc::now() + Duration::minutes(11));
    h.reply(Provider::Google, &flow, "6001", serde_json::json!({}))
        .await;
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(refused(h.finish(&flow).await), "oauth.expired");
    h.clock.set(Utc::now());
    let flow = h.login_flow(Provider::Google).await;
    h.reply(Provider::Google, &flow, "6002", serde_json::json!({}))
        .await;
    h.back(Provider::Google, &flow, None).await;
    h.clock.set(Utc::now() + Duration::minutes(6));
    assert_eq!(refused(h.finish(&flow).await), "oauth.expired");
    assert_eq!(refused(h.finish(&flow).await), "oauth.no_request");
}

#[tokio::test]
async fn после_google_спрашивается_код_двухфакторного_входа() {
    let Some(h) = harness().await else { return };
    let client = h.email_client("totp@gmail.com", true, false).await;
    let setup = h.services.auth().client_totp_setup(client).await.unwrap();
    let key = crate::totp::base32_decode(&setup.secret).unwrap();
    let step = (h.services.now().timestamp() / crate::totp::STEP_SECONDS) as u64;
    let code = crate::totp::code_at(&key, step, crate::totp::DIGITS).unwrap();
    h.services
        .auth()
        .client_totp_enable(client, &setup.secret, &code)
        .await
        .unwrap();
    let result = h
        .sign_in(
            Provider::Google,
            "7001",
            serde_json::json!({ "email": "totp@gmail.com", "email_verified": true }),
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(result, OauthFinish::TotpRequired { .. }),
        "{result:?}"
    );
    let mut conn = h.services.store().conn().await.unwrap();
    let sessions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM client_sessions WHERE client_id = $1")
            .bind(client.0)
            .fetch_one(conn.conn())
            .await
            .unwrap();
    assert_eq!(sessions, 0);
}

#[tokio::test]
async fn привязка_и_отвязка_в_безопасности() {
    let Some(h) = harness().await else { return };
    let client = h.telegram_client(5_093_118_422).await;
    let principal = h.cabinet(client).await;
    let view = h.ops().logins(client).await.unwrap();
    assert_eq!(view.methods.len(), 1);
    assert_eq!(view.methods[0].account, "@morozov_a");
    assert!(!view.methods[0].removable);
    assert_eq!(view.can_link, vec![Provider::Google, Provider::Apple]);
    let linked = h
        .link(
            &principal,
            Provider::Google,
            "8001",
            serde_json::json!({ "email": "alex@gmail.com", "email_verified": true }),
        )
        .await
        .unwrap();
    assert_eq!(
        linked,
        OauthFinish::Linked {
            provider: Provider::Google
        }
    );
    let view = h.ops().logins(client).await.unwrap();
    let google = view
        .methods
        .iter()
        .find(|m| m.kind == IdentityKind::Google)
        .unwrap();
    assert_eq!(google.account, "alex@gmail.com");
    assert!(google.removable);
    assert_eq!(view.can_link, vec![Provider::Apple]);
    assert!(
        !h.identities(client)
            .await
            .iter()
            .any(|i| i.kind == IdentityKind::Email)
    );
    let again = confirmed(
        h.sign_in(Provider::Google, "8001", serde_json::json!({}), None)
            .await,
    );
    assert_eq!(again.principal.client_id, client);
    assert_eq!(
        h.ops()
            .start_link(&principal, Provider::Google, &meta())
            .await
            .unwrap_err()
            .code(),
        "oauth.already_linked"
    );
    let view = h.ops().unlink(&principal, Provider::Google).await.unwrap();
    assert!(view.methods.iter().all(|m| m.kind != IdentityKind::Google));
    assert_eq!(
        h.ops()
            .unlink(&principal, Provider::Google)
            .await
            .unwrap_err()
            .code(),
        "oauth.not_linked"
    );
    let only_google = confirmed(
        h.sign_in(Provider::Google, "8002", serde_json::json!({}), None)
            .await,
    );
    let only = only_google.principal.clone();
    assert_eq!(
        h.ops()
            .unlink(&only, Provider::Google)
            .await
            .unwrap_err()
            .code(),
        "oauth.last_method"
    );
    assert_eq!(
        h.link(&only, Provider::Google, "8001", serde_json::json!({}))
            .await
            .unwrap_err()
            .code(),
        "oauth.already_linked"
    );
    assert_eq!(
        h.link(&principal, Provider::Google, "8002", serde_json::json!({}))
            .await
            .unwrap_err()
            .code(),
        "oauth.taken"
    );
    h.link(
        &only,
        Provider::Apple,
        "000888.apple",
        serde_json::json!({}),
    )
    .await
    .unwrap();
    let view = h.ops().unlink(&only, Provider::Google).await.unwrap();
    assert_eq!(view.methods.len(), 1);
    assert!(!view.methods[0].removable);
    let lapsed = h.cabinet(client).await;
    let flow = Harness::flow(
        h.ops()
            .start_link(&lapsed, Provider::Apple, &meta())
            .await
            .unwrap(),
    );
    {
        let mut conn = h.services.store().conn().await.unwrap();
        sqlx::query("UPDATE client_sessions SET revoked_at = now() WHERE client_id = $1")
            .bind(client.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    h.reply(
        Provider::Apple,
        &flow,
        "000999.apple",
        serde_json::json!({}),
    )
    .await;
    h.back(Provider::Apple, &flow, None).await;
    assert_eq!(refused(h.finish(&flow).await), "oauth.session_gone");
    assert!(
        !h.identities(client)
            .await
            .iter()
            .any(|i| i.kind == IdentityKind::Apple)
    );
    let mut conn = h.services.store().conn().await.unwrap();
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE entity_id = $1 AND action LIKE 'client.oauth_%' ORDER BY id",
    )
    .bind(client.0)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(
        actions,
        [
            "client.oauth_linked",
            "client.oauth_login",
            "client.oauth_unlinked"
        ]
    );
}

#[tokio::test]
async fn кнопки_есть_только_у_включённого_и_настроенного_модуля() {
    let Some(h) = harness().await else { return };
    assert_eq!(
        h.ops().availability(),
        Availability {
            google: true,
            apple: true
        }
    );
    let ops = h.services.settings_ops();
    let mut google_only = full_config();
    for key in ["apple_services_id", "apple_team_id", "apple_key_id"] {
        google_only.insert(key.into(), serde_json::json!(""));
    }
    ops.update_module_config(
        ModuleId::Oauth,
        &ModuleConfigUpdate {
            config: google_only,
            clear: vec!["apple_private_key".into()],
        },
        h.admin,
    )
    .await
    .unwrap();
    assert_eq!(
        h.ops().availability(),
        Availability {
            google: true,
            apple: false
        }
    );
    assert_eq!(
        h.ops()
            .start_login(Provider::Apple, None, &meta())
            .await
            .unwrap_err()
            .code(),
        "oauth.not_configured"
    );
    {
        let mut conn = h.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::PUBLIC_URL,
                &serde_json::json!("http://203.0.113.10:8080"),
                None,
            )
            .await
            .unwrap();
    }
    h.services.reload().await.unwrap();
    assert_eq!(h.ops().availability(), Availability::default());
    {
        let mut conn = h.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::PUBLIC_URL,
                &serde_json::json!("http://localhost:5173/"),
                None,
            )
            .await
            .unwrap();
    }
    h.services.reload().await.unwrap();
    assert_eq!(
        h.ops().availability(),
        Availability {
            google: true,
            apple: false
        }
    );
    let start = h
        .ops()
        .start_login(Provider::Google, None, &meta())
        .await
        .unwrap();
    assert_eq!(
        query(&start.url, "redirect_uri").as_deref(),
        Some("http://localhost:5173/api/client/auth/oauth/google/callback")
    );
    ops.toggle_module(ModuleId::Oauth, false, h.admin)
        .await
        .unwrap();
    assert_eq!(h.ops().availability(), Availability::default());
    assert_eq!(
        h.ops()
            .start_login(Provider::Google, None, &meta())
            .await
            .unwrap_err()
            .code(),
        "oauth.not_configured"
    );
}

#[tokio::test]
async fn google_не_привязать_к_аккаунту_только_с_неподтверждённой_почтой() {
    let Some(h) = harness().await else { return };
    let squatted = h.email_client("vika@gmail.com", false, true).await;
    assert_eq!(
        refused(
            h.link(
                &h.cabinet(squatted).await,
                Provider::Google,
                "6001",
                serde_json::json!({ "email": "evil@gmail.com", "email_verified": true }),
            )
            .await
        ),
        crate::oauth::EMAIL_UNVERIFIED
    );
    let mut conn = h.services.store().conn().await.unwrap();
    assert!(
        conn.clients()
            .find_by_identity(IdentityKind::Google, "6001")
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    let owner = h.email_client("dasha@gmail.com", true, true).await;
    assert_eq!(
        h.link(
            &h.cabinet(owner).await,
            Provider::Google,
            "6002",
            serde_json::json!({ "email": "dasha@gmail.com", "email_verified": true }),
        )
        .await
        .unwrap(),
        OauthFinish::Linked {
            provider: Provider::Google
        }
    );
}

#[tokio::test]
async fn вход_через_google_сохраняет_метку_сайта_вместе_с_приглашением() {
    let Some(h) = harness().await else { return };
    let tags = Utm {
        source: Some("tiktok".into()),
        campaign: Some("osen".into()),
        term: Some("bad value".into()),
        ..Utm::default()
    };
    let flow = Harness::flow(
        h.ops()
            .start_login_with(Provider::Google, Some("friend42"), Some(&tags), &meta())
            .await
            .unwrap(),
    );
    h.reply(
        Provider::Google,
        &flow,
        "2077",
        serde_json::json!({ "email": "lena@gmail.com", "email_verified": true }),
    )
    .await;
    h.back(Provider::Google, &flow, None).await;
    let login = confirmed(h.finish(&flow).await);
    assert!(login.created);
    let plain = confirmed(
        h.sign_in(
            Provider::Google,
            "2078",
            serde_json::json!({ "email": "pavel@gmail.com", "email_verified": true }),
            None,
        )
        .await,
    );
    assert!(plain.created);
    h.services.events().dispatch_pending(50).await.unwrap();
    let mut conn = h.services.store().conn().await.unwrap();
    let origin = origin_of(&mut conn, login.principal.client_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(origin.start_param, "ref_FRIEND42__s_tiktok__k_osen");
    assert_eq!(origin.utm.source.as_deref(), Some("tiktok"));
    assert_eq!(origin.utm.term, None);
    assert_eq!(origin.registration_source, RegistrationSource::Web);
    assert!(
        origin_of(&mut conn, plain.principal.client_id)
            .await
            .unwrap()
            .is_none()
    );
}

async fn limited(config: serde_json::Value) -> Option<Harness> {
    let h = harness_with(vec![Arc::new(sn_modules::catalog::antibot::AntibotModule)]).await?;
    let ops = h.services.settings_ops();
    ops.update_module_config(
        ModuleId::Antibot,
        &ModuleConfigUpdate {
            config: config.as_object().cloned().unwrap(),
            clear: Vec::new(),
        },
        h.admin,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Antibot, true, h.admin)
        .await
        .unwrap();
    Some(h)
}

fn person(n: u32) -> serde_json::Value {
    serde_json::json!({ "email": format!("person{n}@gmail.com"), "email_verified": true })
}

async fn held(h: &Harness) -> (i64, i64) {
    let mut conn = h.services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'human'), count(*) FILTER (WHERE claims IS NOT NULL) FROM oauth_requests",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn signups_made(h: &Harness) {
    let mut conn = h.services.store().conn().await.unwrap();
    sqlx::query(
        "INSERT INTO site_signups (via, address, created_at) \
         VALUES ('password', '198.51.100.1', now()), ('password', '198.51.100.1', now() + interval '1 minute')",
    )
    .execute(conn.conn())
    .await
    .unwrap();
}

#[tokio::test]
async fn предел_аккаунтов_с_адреса_действует_на_google_и_apple_а_вход_в_свои_аккаунты_нет() {
    let Some(h) = limited(serde_json::json!({ "site_address_day": 2 })).await else {
        return;
    };
    confirmed(h.sign_in(Provider::Google, "3001", person(1), None).await);
    confirmed(h.sign_in(Provider::Apple, "3002", person(2), None).await);
    for provider in [Provider::Google, Provider::Apple] {
        let refused = h
            .sign_in(provider, "3003", person(3), None)
            .await
            .unwrap_err();
        assert_eq!(refused.code(), signups::LIMIT_CODE);
        assert!(refused.to_string().contains("Зайдите через бота"));
    }
    assert_eq!(h.clients_count().await, 2);
    let again = confirmed(h.sign_in(Provider::Google, "3001", person(1), None).await);
    assert!(!again.created);
    let again = confirmed(h.sign_in(Provider::Apple, "3002", person(2), None).await);
    assert!(!again.created);
    let mut conn = h.services.store().conn().await.unwrap();
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT via, address FROM site_signups ORDER BY id")
            .fetch_all(conn.conn())
            .await
            .unwrap();
    assert_eq!(
        rows,
        ["google", "apple"].map(|via| (via.to_string(), "203.0.113.7".to_string()))
    );
    let day = signups::day(&mut conn).await.unwrap();
    assert_eq!((day.created, day.address), (2, 2));
}

#[tokio::test]
async fn сверх_минутного_предела_google_просит_капчу_и_не_теряет_вход() {
    let Some(h) = limited(serde_json::json!({
        "site_minute": 1,
        "captcha": "turnstile",
        "captcha_site_key": "0x4AAAAAAAsite",
        "captcha_secret": "0x4AAAAAAAsecret-key",
        "captcha_signup": false,
    }))
    .await
    else {
        return;
    };
    let verify = crate::scenario_tests::siteverify::siteverify(&h.services).await;
    signups_made(&h).await;
    let flow = h.login_flow(Provider::Google).await;
    h.reply(Provider::Google, &flow, "3101", person(11)).await;
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(held(&h).await, (1, 1));
    for _ in 0..2 {
        let asked = h.finish(&flow).await.unwrap_err();
        assert_eq!(asked.code(), antibot::CAPTCHA_REQUIRED);
    }
    assert_eq!(verify.calls(), 0);
    let wrong = h
        .ops()
        .finish_with(Some(&flow.start.browser), &meta(), Some("nope"))
        .await
        .unwrap_err();
    assert_eq!(wrong.code(), antibot::FAILED);
    assert_eq!(h.clients_count().await, 0);
    let finish = h
        .ops()
        .finish_with(Some(&flow.start.browser), &meta(), Some("ok:signup"))
        .await;
    let login = confirmed(finish);
    assert!(login.created);
    assert_eq!(held(&h).await, (0, 0));
    assert_eq!(
        h.finish(&flow).await.unwrap_err().code(),
        "oauth.no_request"
    );
    let again = confirmed(h.sign_in(Provider::Google, "3101", person(11), None).await);
    assert!(!again.created, "вход в созданный аккаунт без капчи");
    let mut conn = h.services.store().conn().await.unwrap();
    let day = signups::day(&mut conn).await.unwrap();
    assert_eq!((day.asked, day.minute), (2, 0));
}

#[tokio::test]
async fn отложенный_вход_через_google_гаснет_через_пять_минут_и_не_хранит_данные() {
    let Some(h) = limited(serde_json::json!({
        "site_minute": 1,
        "captcha": "turnstile",
        "captcha_site_key": "0x4AAAAAAAsite",
        "captcha_secret": "0x4AAAAAAAsecret-key",
    }))
    .await
    else {
        return;
    };
    let _verify = crate::scenario_tests::siteverify::siteverify(&h.services).await;
    signups_made(&h).await;
    let flow = h.login_flow(Provider::Google).await;
    h.reply(Provider::Google, &flow, "3201", person(21)).await;
    h.back(Provider::Google, &flow, None).await;
    assert_eq!(held(&h).await, (1, 1));
    h.clock
        .set(Utc::now() + Duration::seconds(FINISH_SECONDS + 1));
    assert_eq!(
        h.ops()
            .finish_with(Some(&flow.start.browser), &meta(), Some("ok:signup"))
            .await
            .unwrap_err()
            .code(),
        "oauth.expired"
    );
    assert_eq!(held(&h).await, (0, 0));
    h.clock.set(Utc::now());
    let left = h.login_flow(Provider::Google).await;
    h.reply(Provider::Google, &left, "3202", person(22)).await;
    h.back(Provider::Google, &left, None).await;
    assert_eq!(held(&h).await, (1, 1));
    h.clock
        .set(Utc::now() + Duration::seconds(FINISH_SECONDS + 1));
    h.login_flow(Provider::Google).await;
    assert_eq!(held(&h).await, (0, 0), "брошенный вход не хранит данные");
    assert_eq!(h.clients_count().await, 0);
}

#[tokio::test]
async fn без_капчи_сверх_минутного_предела_google_отвечает_отказом() {
    let Some(h) = limited(serde_json::json!({ "site_minute": 1 })).await else {
        return;
    };
    signups_made(&h).await;
    let flow = h.login_flow(Provider::Apple).await;
    h.reply(Provider::Apple, &flow, "3301", person(31)).await;
    h.back(Provider::Apple, &flow, None).await;
    let refused = h.finish(&flow).await.unwrap_err();
    assert_eq!(refused.code(), signups::BUSY_CODE);
    assert_eq!(held(&h).await, (0, 0));
    assert_eq!(h.clients_count().await, 0);
    let mut conn = h.services.store().conn().await.unwrap();
    assert_eq!(signups::day(&mut conn).await.unwrap().minute, 1);
}
