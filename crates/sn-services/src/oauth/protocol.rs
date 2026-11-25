use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use sn_modules::catalog::oauth::{AppleSettings, GoogleSettings, Provider};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

pub const APPLE_AUDIENCE: &str = "https://appleid.apple.com";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const KEYS_TTL: Duration = Duration::from_secs(3600);
const CLOCK_LEEWAY_SECONDS: u64 = 120;
const APPLE_SECRET_SECONDS: i64 = 300;
const GOOGLE_HOSTED: [&str; 2] = ["gmail.com", "googlemail.com"];
const APPLE_HOSTED: [&str; 3] = ["icloud.com", "me.com", "mac.com"];
const APPLE_RELAY: &str = "privaterelay.appleid.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    pub jwks: String,
    pub issuers: Vec<String>,
}

impl Endpoints {
    pub fn google() -> Self {
        Endpoints {
            authorize: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            jwks: "https://www.googleapis.com/oauth2/v3/certs".into(),
            issuers: vec![
                "https://accounts.google.com".into(),
                "accounts.google.com".into(),
            ],
        }
    }

    pub fn apple() -> Self {
        Endpoints {
            authorize: "https://appleid.apple.com/auth/authorize".into(),
            token: "https://appleid.apple.com/auth/token".into(),
            jwks: "https://appleid.apple.com/auth/keys".into(),
            issuers: vec![APPLE_AUDIENCE.into()],
        }
    }

    pub fn at(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Endpoints {
            authorize: format!("{base}/authorize"),
            token: format!("{base}/token"),
            jwks: format!("{base}/keys"),
            issuers: vec![base.to_string()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthEndpoints {
    pub google: Endpoints,
    pub apple: Endpoints,
}

impl Default for OauthEndpoints {
    fn default() -> Self {
        OauthEndpoints {
            google: Endpoints::google(),
            apple: Endpoints::apple(),
        }
    }
}

impl OauthEndpoints {
    pub fn of(&self, provider: Provider) -> &Endpoints {
        match provider {
            Provider::Google => &self.google,
            Provider::Apple => &self.apple,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    Unavailable(String),
    Refused(String),
    BadToken(String),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Unavailable(reason) => write!(f, "нет связи: {reason}"),
            ProtocolError::Refused(reason) => write!(f, "отказ провайдера: {reason}"),
            ProtocolError::BadToken(reason) => write!(f, "ответ не прошёл проверку: {reason}"),
        }
    }
}

fn derive(secret: &str, label: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(format!("{label}:{secret}").as_bytes()))
}

pub fn code_verifier(browser: &str) -> String {
    derive(browser, "pkce")
}

pub fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn nonce(browser: &str) -> String {
    derive(browser, "nonce")
}

pub struct AuthorizeRequest<'a> {
    pub provider: Provider,
    pub endpoints: &'a Endpoints,
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub browser: &'a str,
}

pub fn authorize_url(request: &AuthorizeRequest<'_>) -> Result<String, ProtocolError> {
    let nonce = nonce(request.browser);
    let challenge = code_challenge(&code_verifier(request.browser));
    let mut params: Vec<(&str, &str)> = vec![
        ("response_type", "code"),
        ("client_id", request.client_id),
        ("redirect_uri", request.redirect_uri),
        ("state", request.state),
        ("nonce", &nonce),
    ];
    match request.provider {
        Provider::Google => params.extend([
            ("scope", "openid email profile"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ]),
        Provider::Apple => params.extend([("scope", "name email"), ("response_mode", "form_post")]),
    }
    reqwest::Url::parse_with_params(&request.endpoints.authorize, &params)
        .map(String::from)
        .map_err(|err| ProtocolError::Unavailable(format!("адрес авторизации: {err}")))
}

#[derive(Debug, Serialize)]
struct AppleSecretClaims<'a> {
    iss: &'a str,
    iat: i64,
    exp: i64,
    aud: &'a str,
    sub: &'a str,
}

pub fn apple_client_secret(
    settings: &AppleSettings,
    now: DateTime<Utc>,
) -> Result<String, ProtocolError> {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(settings.key_id.clone());
    let claims = AppleSecretClaims {
        iss: &settings.team_id,
        iat: now.timestamp(),
        exp: now.timestamp() + APPLE_SECRET_SECONDS,
        aud: APPLE_AUDIENCE,
        sub: &settings.services_id,
    };
    let key = EncodingKey::from_ec_pem(settings.private_key.as_bytes())
        .map_err(|err| ProtocolError::Refused(format!("ключ .p8: {err}")))?;
    jsonwebtoken::encode(&header, &claims, &key)
        .map_err(|err| ProtocolError::Refused(format!("ключ .p8: {err}")))
}

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
});

#[derive(Debug, Deserialize)]
struct TokenReply {
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenFailure {
    error: Option<String>,
    error_description: Option<String>,
}

pub enum Credentials<'a> {
    Google(&'a GoogleSettings),
    Apple(&'a AppleSettings),
}

pub async fn exchange_code(
    endpoints: &Endpoints,
    credentials: Credentials<'_>,
    code: &str,
    redirect_uri: &str,
    browser: &str,
    now: DateTime<Utc>,
) -> Result<String, ProtocolError> {
    let verifier = code_verifier(browser);
    let form: Vec<(&str, String)> = match credentials {
        Credentials::Google(google) => vec![
            ("grant_type", "authorization_code".into()),
            ("code", code.into()),
            ("redirect_uri", redirect_uri.into()),
            ("client_id", google.client_id.clone()),
            ("client_secret", google.client_secret.clone()),
            ("code_verifier", verifier),
        ],
        Credentials::Apple(apple) => vec![
            ("grant_type", "authorization_code".into()),
            ("code", code.into()),
            ("redirect_uri", redirect_uri.into()),
            ("client_id", apple.services_id.clone()),
            ("client_secret", apple_client_secret(apple, now)?),
        ],
    };
    let response = HTTP
        .post(&endpoints.token)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|err| ProtocolError::Unavailable(err.without_url().to_string()))?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|err| ProtocolError::Unavailable(err.without_url().to_string()))?;
    if !status.is_success() {
        let reason = serde_json::from_slice::<TokenFailure>(&body)
            .ok()
            .and_then(|f| match (f.error, f.error_description) {
                (Some(error), Some(text)) => Some(format!("{error}: {text}")),
                (Some(error), None) => Some(error),
                (None, text) => text,
            })
            .unwrap_or_else(|| format!("ответ {status}"));
        return Err(if status.is_server_error() {
            ProtocolError::Unavailable(reason)
        } else {
            ProtocolError::Refused(reason)
        });
    }
    serde_json::from_slice::<TokenReply>(&body)
        .ok()
        .and_then(|reply| reply.id_token)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| ProtocolError::BadToken("в ответе нет id_token".into()))
}

struct CachedKeys {
    set: JwkSet,
    fetched: Instant,
}

static KEYS: LazyLock<Mutex<HashMap<String, CachedKeys>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cached(url: &str, kid: &str) -> (Option<DecodingKey>, bool) {
    let guard = match KEYS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(entry) = guard.get(url) else {
        return (None, false);
    };
    let key = entry
        .set
        .find(kid)
        .and_then(|jwk| DecodingKey::from_jwk(jwk).ok());
    (key, entry.fetched.elapsed() < KEYS_TTL)
}

async fn fetch_keys(url: &str) -> Result<(), ProtocolError> {
    let response = HTTP
        .get(url)
        .send()
        .await
        .map_err(|err| ProtocolError::Unavailable(err.without_url().to_string()))?;
    if !response.status().is_success() {
        return Err(ProtocolError::Unavailable(format!(
            "ключи провайдера: ответ {}",
            response.status()
        )));
    }
    let set: JwkSet = response
        .json()
        .await
        .map_err(|err| ProtocolError::Unavailable(format!("ключи провайдера: {err}")))?;
    let mut guard = match KEYS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.insert(
        url.to_string(),
        CachedKeys {
            set,
            fetched: Instant::now(),
        },
    );
    Ok(())
}

async fn decoding_key(url: &str, kid: &str) -> Result<DecodingKey, ProtocolError> {
    match cached(url, kid) {
        (Some(key), true) => return Ok(key),
        (stale, _) => {
            if let Err(err) = fetch_keys(url).await {
                return stale.ok_or(err);
            }
        }
    }
    cached(url, kid)
        .0
        .ok_or_else(|| ProtocolError::BadToken(format!("неизвестный ключ подписи {kid}")))
}

fn flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bool(bool),
        Text(String),
    }
    Ok(match Option::<Raw>::deserialize(deserializer)? {
        Some(Raw::Bool(value)) => value,
        Some(Raw::Text(text)) => text.trim().eq_ignore_ascii_case("true"),
        None => false,
    })
}

#[derive(Debug, Deserialize)]
struct RawClaims {
    sub: String,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, deserialize_with = "flag")]
    email_verified: bool,
    #[serde(default, deserialize_with = "flag")]
    is_private_email: bool,
    #[serde(default)]
    hd: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    given_name: Option<String>,
    #[serde(default)]
    family_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdClaims {
    pub provider: Provider,
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub private_email: bool,
    pub hosted_domain: Option<String>,
    pub name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

fn clean(value: Option<String>, max: usize) -> Option<String> {
    value
        .map(|v| {
            v.chars()
                .filter(|c| !c.is_control())
                .take(max)
                .collect::<String>()
        })
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub async fn verify_id_token(
    provider: Provider,
    endpoints: &Endpoints,
    token: &str,
    audience: &str,
    browser: &str,
) -> Result<IdClaims, ProtocolError> {
    let header = jsonwebtoken::decode_header(token)
        .map_err(|err| ProtocolError::BadToken(format!("заголовок: {err}")))?;
    if header.alg != Algorithm::RS256 {
        return Err(ProtocolError::BadToken(format!(
            "подпись {:?} вместо RS256",
            header.alg
        )));
    }
    let kid = header
        .kid
        .ok_or_else(|| ProtocolError::BadToken("в заголовке нет kid".into()))?;
    let key = decoding_key(&endpoints.jwks, &kid).await?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[audience]);
    validation.set_issuer(&endpoints.issuers);
    validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);
    validation.leeway = CLOCK_LEEWAY_SECONDS;
    let data = jsonwebtoken::decode::<RawClaims>(token, &key, &validation)
        .map_err(|err| ProtocolError::BadToken(err.to_string()))?;
    let claims = data.claims;
    let expected = nonce(browser);
    if !claims.nonce.as_deref().is_some_and(|n| same(n, &expected)) {
        return Err(ProtocolError::BadToken("nonce не совпал".into()));
    }
    if claims.azp.as_deref().is_some_and(|azp| azp != audience) {
        return Err(ProtocolError::BadToken("azp не совпал".into()));
    }
    let sub = claims.sub.trim().to_string();
    if sub.is_empty() || sub.len() > 255 {
        return Err(ProtocolError::BadToken("пустой sub".into()));
    }
    Ok(IdClaims {
        provider,
        sub,
        email: clean(claims.email, 254).map(|e| e.to_lowercase()),
        email_verified: claims.email_verified,
        private_email: claims.is_private_email,
        hosted_domain: clean(claims.hd, 253).map(|d| d.to_lowercase()),
        name: clean(claims.name, 120),
        first_name: clean(claims.given_name, 80),
        last_name: clean(claims.family_name, 80),
    })
}

impl IdClaims {
    pub fn verified_email(&self) -> Option<&str> {
        self.email.as_deref().filter(|_| self.email_verified)
    }

    pub fn email_domain(&self) -> Option<&str> {
        self.email.as_deref()?.rsplit_once('@').map(|(_, d)| d)
    }

    pub fn relay(&self) -> bool {
        self.provider == Provider::Apple
            && (self.private_email || self.email_domain() == Some(APPLE_RELAY))
    }

    pub fn authoritative(&self) -> bool {
        if self.verified_email().is_none() {
            return false;
        }
        let Some(domain) = self.email_domain() else {
            return false;
        };
        match self.provider {
            Provider::Google => {
                GOOGLE_HOSTED.contains(&domain) || self.hosted_domain.as_deref() == Some(domain)
            }
            Provider::Apple => self.relay() || APPLE_HOSTED.contains(&domain),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppleName {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

pub fn apple_name(user: Option<&str>) -> AppleName {
    #[derive(Deserialize)]
    struct User {
        name: Option<Name>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Name {
        first_name: Option<String>,
        last_name: Option<String>,
    }
    let Some(name) = user
        .filter(|raw| raw.len() <= 2000)
        .and_then(|raw| serde_json::from_str::<User>(raw).ok())
        .and_then(|user| user.name)
    else {
        return AppleName::default();
    };
    AppleName {
        first_name: clean(name.first_name, 80),
        last_name: clean(name.last_name, 80),
    }
}
