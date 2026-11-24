use super::signing::{self, PageClaims};
use super::{ExtModulesOps, runtime, secret_key, store};
use crate::error::ServiceResult;
use crate::settings::read_secret;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Duration;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sn_domain::access::{AdminPrincipal, AdminRole, Permission};
use sn_domain::ids::AdminId;
use sn_store::Db;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

type HmacSha256 = Hmac<Sha256>;

pub const PREFIX: &str = "/m/";
pub const CAP_HOURS: i64 = 12;
const LABEL: &[u8] = b"stealthnet module page\0";
const TAG_LEN: usize = 16;
const CACHE_SECONDS: u64 = 15;
const CACHE_MAX: usize = 1000;

fn mac(key: &[u8], module: &str, payload: &[u8]) -> Option<HmacSha256> {
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(LABEL);
    mac.update(module.as_bytes());
    mac.update(b"\0");
    mac.update(payload);
    Some(mac)
}

pub fn cap(key: &[u8], module: &str, admin: i64, session: i64, exp: i64) -> String {
    let mut payload = Vec::with_capacity(24);
    payload.extend_from_slice(&admin.to_be_bytes());
    payload.extend_from_slice(&session.to_be_bytes());
    payload.extend_from_slice(&exp.to_be_bytes());
    let tag = mac(key, module, &payload)
        .map(|m| m.finalize().into_bytes()[..TAG_LEN].to_vec())
        .unwrap_or_default();
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&payload),
        URL_SAFE_NO_PAD.encode(tag)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapClaims {
    pub admin: i64,
    pub session: i64,
    pub exp: i64,
}

pub fn read_cap(key: &[u8], module: &str, cap: &str, now: i64) -> Option<CapClaims> {
    if cap.len() > 80 {
        return None;
    }
    let (payload, tag) = cap.split_once('.')?;
    let payload = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
    if payload.len() != 24 || tag.len() != TAG_LEN {
        return None;
    }
    mac(key, module, &payload)?
        .verify_truncated_left(&tag)
        .ok()?;
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        Some(i64::from_be_bytes(payload.get(range)?.try_into().ok()?))
    };
    let claims = CapClaims {
        admin: number(0..8)?,
        session: number(8..16)?,
        exp: number(16..24)?,
    };
    (claims.exp > now).then_some(claims)
}

#[derive(Debug, Clone)]
pub struct PageEntry {
    pub module: String,
    pub base: String,
    pub prefix: String,
    key: Vec<u8>,
    admin_id: i64,
    email: String,
    role: AdminRole,
    permissions: Vec<Permission>,
}

impl PageEntry {
    pub fn identity(&self, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
        let claims = PageClaims {
            iss: signing::ISSUER.into(),
            aud: signing::audience(&self.module),
            sub: self.admin_id.to_string(),
            iat: now.timestamp(),
            exp: (now + Duration::seconds(signing::PAGE_TOKEN_SECONDS)).timestamp(),
            jti: signing::nonce(),
            module: self.module.clone(),
            admin_id: self.admin_id,
            email: self.email.clone(),
            role: self.role,
            permissions: self.permissions.clone(),
        };
        signing::page_token(&self.key, &claims)
    }
}

type Cache = Mutex<HashMap<String, (Instant, Arc<PageEntry>)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached(key: &str) -> Option<Arc<PageEntry>> {
    let map = cache().lock().ok()?;
    map.get(key)
        .filter(|(at, _)| at.elapsed().as_secs() < CACHE_SECONDS)
        .map(|(_, entry)| entry.clone())
}

fn remember(key: String, entry: Arc<PageEntry>) {
    if let Ok(mut map) = cache().lock() {
        if map.len() >= CACHE_MAX {
            map.retain(|_, (at, _)| at.elapsed().as_secs() < CACHE_SECONDS);
            if map.len() >= CACHE_MAX {
                map.clear();
            }
        }
        map.insert(key, (Instant::now(), entry));
    }
}

pub fn forget(module: &str) {
    if let Ok(mut map) = cache().lock() {
        let prefix = format!("{module}/");
        map.retain(|key, _| !key.starts_with(&prefix));
    }
}

pub const BODY_MAX: usize = 10 * 1024 * 1024;
pub const IDENTITY_HEADER: &str = "x-stealthnet-admin";
const REQUEST_HEADERS: [&str; 9] = [
    "accept",
    "accept-language",
    "accept-encoding",
    "content-type",
    "authorization",
    "if-none-match",
    "if-modified-since",
    "range",
    "cache-control",
];
const RESPONSE_HEADERS: [&str; 12] = [
    "content-type",
    "content-length",
    "content-encoding",
    "content-disposition",
    "content-language",
    "content-range",
    "accept-ranges",
    "etag",
    "last-modified",
    "retry-after",
    "content-security-policy",
    "vary",
];

pub type BodyStream = std::pin::Pin<
    Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send>,
>;

pub struct Forwarded {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: BodyStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardError {
    Unreachable,
    Timeout,
    Redirect,
}

impl ForwardError {
    pub fn text(&self) -> &'static str {
        match self {
            ForwardError::Unreachable => {
                "Модуль не отвечает. Проверьте его состояние в карточке модуля"
            }
            ForwardError::Timeout => "Модуль не ответил вовремя. Попробуйте ещё раз",
            ForwardError::Redirect => {
                "Модуль перенаправил на внешний адрес: такие переходы закрыты"
            }
        }
    }
}

pub fn request_header(name: &str) -> bool {
    REQUEST_HEADERS.contains(&name)
}

fn location(prefix: &str, value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with("//") || value.contains("://") || value.contains('\\') {
        return None;
    }
    if value.starts_with('/') {
        return Some(format!("{prefix}{value}"));
    }
    Some(value.to_string())
}

pub struct Outgoing<'a> {
    pub method: &'a str,
    pub rest: &'a str,
    pub query: Option<&'a str>,
    pub headers: &'a [(String, String)],
    pub body: bytes::Bytes,
}

pub async fn forward(
    entry: &PageEntry,
    outgoing: Outgoing<'_>,
    now: chrono::DateTime<chrono::Utc>,
    public_url: Option<&str>,
) -> Result<Forwarded, ForwardError> {
    let Outgoing {
        method,
        rest,
        query,
        headers,
        body,
    } = outgoing;
    let method =
        reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| ForwardError::Unreachable)?;
    let path = if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    };
    let url = match query.filter(|q| !q.is_empty()) {
        Some(query) => format!("{}{path}?{query}", entry.base),
        None => format!("{}{path}", entry.base),
    };
    let mut request = super::address::page_client().request(method, &url);
    for (name, value) in headers {
        if request_header(name) {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    if let Some(identity) = entry.identity(now) {
        request = request.header(IDENTITY_HEADER, identity);
    }
    request = request.header("x-forwarded-prefix", entry.prefix.as_str());
    if let Some(public) = public_url.and_then(|u| reqwest::Url::parse(u).ok()) {
        request = request.header("x-forwarded-proto", public.scheme());
        if let Some(host) = public.host_str() {
            let host = match public.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            };
            request = request.header("x-forwarded-host", host);
        }
    }
    if !body.is_empty() {
        request = request.body(body);
    }
    let response = request.send().await.map_err(|err| {
        if err.is_timeout() {
            ForwardError::Timeout
        } else {
            ForwardError::Unreachable
        }
    })?;
    let status = response.status().as_u16();
    let mut out = Vec::new();
    for (name, value) in response.headers() {
        let name = name.as_str().to_ascii_lowercase();
        let Ok(value) = value.to_str() else {
            continue;
        };
        if name == "location" {
            match location(&entry.prefix, value) {
                Some(value) => out.push((name, value)),
                None => return Err(ForwardError::Redirect),
            }
        } else if RESPONSE_HEADERS.contains(&name.as_str()) {
            out.push((name, value.to_string()));
        }
    }
    let body = futures_util::stream::unfold(Some(response), |state| async move {
        let mut response = state?;
        match response.chunk().await {
            Ok(Some(chunk)) => Some((Ok(chunk), Some(response))),
            Ok(None) => None,
            Err(err) => Some((Err(std::io::Error::other(err)), None)),
        }
    });
    Ok(Forwarded {
        status,
        headers: out,
        body: Box::pin(body),
    })
}

impl ExtModulesOps<'_> {
    pub async fn page_access(
        &self,
        module: &str,
        cap: &str,
    ) -> ServiceResult<Option<Arc<PageEntry>>> {
        if !runtime::valid_module(module) {
            return Ok(None);
        }
        let cache_key = format!("{module}/{cap}");
        if let Some(entry) = cached(&cache_key) {
            return Ok(Some(entry));
        }
        let mut conn = self.services.store().conn().await?;
        let Some(row) = store::get(&mut conn, module).await? else {
            return Ok(None);
        };
        if !row.managed || !row.enabled {
            return Ok(None);
        }
        let (Some(page), Some(run)) = (row.manifest.page.as_ref(), row.manifest.run.as_ref())
        else {
            return Ok(None);
        };
        let Some(key) = read_secret(&mut conn, self.services.secrets(), &secret_key(module))
            .await?
            .as_deref()
            .and_then(signing::key)
        else {
            return Ok(None);
        };
        let now = self.services.now();
        let Some(claims) = read_cap(&key, module, cap, now.timestamp()) else {
            return Ok(None);
        };
        let admin_id = AdminId(claims.admin);
        if conn
            .sessions()
            .admin_by_id(claims.session, admin_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let admin = conn.admins().get(admin_id).await?;
        let principal = AdminPrincipal {
            admin_id,
            role: admin.role,
            permissions: admin.permissions.clone(),
            session_id: claims.session,
            api_key: None,
        };
        if !principal.can(page.permission) {
            return Ok(None);
        }
        let entry = Arc::new(PageEntry {
            module: module.to_string(),
            base: runtime::url(
                self.services.update_config().as_deref(),
                module,
                run.port,
                "",
            ),
            prefix: format!("{PREFIX}{module}/{cap}"),
            key,
            admin_id: claims.admin,
            email: admin.email,
            role: admin.role,
            permissions: Permission::ALL
                .into_iter()
                .filter(|p| principal.can(*p))
                .collect(),
        });
        remember(cache_key, entry.clone());
        Ok(Some(entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn перенаправления_модуля_остаются_под_его_адресом() {
        let prefix = "/m/nodes/abc";
        assert_eq!(
            location(prefix, "/login").as_deref(),
            Some("/m/nodes/abc/login")
        );
        assert_eq!(location(prefix, "next?x=1").as_deref(), Some("next?x=1"));
        assert_eq!(location(prefix, "https://evil.example/"), None);
        assert_eq!(location(prefix, "//evil.example/"), None);
        assert_eq!(location(prefix, "/\\evil.example/"), None);
        assert!(request_header("authorization"));
        assert!(!request_header("cookie"));
        assert!(!request_header("x-forwarded-for"));
    }

    #[test]
    fn пропуск_на_страницу_подписан_и_привязан_к_модулю() {
        let key = [7u8; 32];
        let made = cap(&key, "nodes", 3, 41, 2_000_000_000);
        assert!(made.len() < 80, "{made}");
        assert!(
            made.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        );
        assert_eq!(
            read_cap(&key, "nodes", &made, 1_900_000_000),
            Some(CapClaims {
                admin: 3,
                session: 41,
                exp: 2_000_000_000
            })
        );
        assert_eq!(read_cap(&key, "other", &made, 1_900_000_000), None);
        assert_eq!(read_cap(&[8u8; 32], "nodes", &made, 1_900_000_000), None);
        assert_eq!(read_cap(&key, "nodes", &made, 2_000_000_001), None);
        let (payload, tag) = made.split_once('.').unwrap();
        let mut forged = URL_SAFE_NO_PAD.decode(payload).unwrap();
        forged[7] = 1;
        let forged = format!("{}.{tag}", URL_SAFE_NO_PAD.encode(forged));
        assert_eq!(read_cap(&key, "nodes", &forged, 1_900_000_000), None);
        assert_eq!(read_cap(&key, "nodes", "garbage", 1_900_000_000), None);
    }
}
