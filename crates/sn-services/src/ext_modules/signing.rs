use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sn_domain::access::{AdminRole, Permission};

pub const SECRET_PREFIX: &str = "whsec_";
pub const ISSUER: &str = "stealthnet";
pub const PAGE_TOKEN_SECONDS: i64 = 300;
pub const TOKEN_PARAM: &str = "sn_token";

pub fn new_secret() -> String {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    format!("{SECRET_PREFIX}{}", STANDARD.encode(raw))
}

pub fn key(secret: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(secret.trim().strip_prefix(SECRET_PREFIX)?)
        .ok()
        .filter(|k| !k.is_empty())
}

pub fn signature(key: &[u8], id: &str, timestamp: i64, body: &[u8]) -> Option<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    Some(format!(
        "v1,{}",
        STANDARD.encode(mac.finalize().into_bytes())
    ))
}

pub fn audience(module: &str) -> String {
    format!("module:{module}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
    pub module: String,
    pub admin_id: i64,
    pub email: String,
    pub role: AdminRole,
    pub permissions: Vec<Permission>,
}

pub fn page_token(key: &[u8], claims: &PageClaims) -> Option<String> {
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(key),
    )
    .ok()
}

pub fn read_page_token(key: &[u8], token: &str, module: &str, now: i64) -> Option<PageClaims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_audience(&[audience(module)]);
    validation.set_issuer(&[ISSUER]);
    validation.validate_exp = false;
    let claims =
        jsonwebtoken::decode::<PageClaims>(token, &DecodingKey::from_secret(key), &validation)
            .ok()?
            .claims;
    (claims.exp > now && claims.iat <= now + 60).then_some(claims)
}

pub fn nonce() -> String {
    let mut raw = [0u8; 12];
    OsRng.fill_bytes(&mut raw);
    hex::encode(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn подпись_совпадает_с_открытой_спецификацией_standard_webhooks() {
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
        let body = br#"{"test": 2432232314}"#;
        assert_eq!(
            signature(
                &key(secret).unwrap(),
                "msg_p5jXN8AQM9LWM0D4loKWxJek",
                1614265330,
                body
            )
            .unwrap(),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
    }

    #[test]
    fn секрет_подписи_новый_каждый_раз_и_читается_обратно() {
        let a = new_secret();
        let b = new_secret();
        assert_ne!(a, b);
        assert!(a.starts_with(SECRET_PREFIX));
        assert_eq!(key(&a).unwrap().len(), 32);
        assert!(key("не секрет").is_none());
        assert!(key("whsec_").is_none());
    }

    #[test]
    fn токен_страницы_проверяется_только_своим_модулем_и_до_срока() {
        let key = key(&new_secret()).unwrap();
        let claims = PageClaims {
            iss: ISSUER.into(),
            aud: audience("hello"),
            sub: "7".into(),
            iat: 1_000,
            exp: 1_000 + PAGE_TOKEN_SECONDS,
            jti: nonce(),
            module: "hello".into(),
            admin_id: 7,
            email: "owner@example.com".into(),
            role: AdminRole::Owner,
            permissions: vec![Permission::Modules],
        };
        let token = page_token(&key, &claims).unwrap();
        assert_eq!(read_page_token(&key, &token, "hello", 1_100), Some(claims));
        assert!(read_page_token(&key, &token, "other", 1_100).is_none());
        assert!(read_page_token(&key, &token, "hello", 1_000 + PAGE_TOKEN_SECONDS).is_none());
        let stranger = super::key(&new_secret()).unwrap();
        assert!(read_page_token(&stranger, &token, "hello", 1_100).is_none());
    }
}
