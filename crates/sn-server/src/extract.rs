use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::Json;
use axum::extract::{
    ConnectInfo, FromRequest, FromRequestParts, OptionalFromRequest, Query, Request, State,
};
use axum::http::header::{AUTHORIZATION, COOKIE, HOST, ORIGIN, USER_AGENT};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde::de::DeserializeOwned;
use sn_domain::access::{AdminPrincipal, ClientChannel, ClientPrincipal, Permission};
use sn_services::Actor;
use sn_services::auth::RequestMeta;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Mutex, OnceLock};

pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match <Json<T> as FromRequest<S>>::from_request(req, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(ApiError::new(
                rejection.status(),
                "validation.invalid_body",
                rejection.body_text(),
            )),
        }
    }
}

impl<T, S> OptionalFromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        match <Json<T> as OptionalFromRequest<S>>::from_request(req, state).await {
            Ok(value) => Ok(value.map(|Json(value)| ApiJson(value))),
            Err(rejection) => Err(ApiError::new(
                rejection.status(),
                "validation.invalid_body",
                rejection.body_text(),
            )),
        }
    }
}

pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(ApiQuery(value)),
            Err(rejection) => Err(ApiError::new(
                rejection.status(),
                "validation.invalid_query",
                rejection.body_text(),
            )),
        }
    }
}

pub const ADMIN_COOKIE: &str = "sn_admin";
pub const CLIENT_COOKIE: &str = "sn_client";
pub const CROSS_SITE: &str = "auth.cross_site";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    Bearer(String),
    Cookie(String),
}

impl Credential {
    pub fn token(&self) -> &str {
        match self {
            Credential::Bearer(token) | Credential::Cookie(token) => token,
        }
    }

    pub fn into_token(self) -> String {
        match self {
            Credential::Bearer(token) | Credential::Cookie(token) => token,
        }
    }

    pub const fn from_cookie(&self) -> bool {
        matches!(self, Credential::Cookie(_))
    }
}

pub fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
        .filter(|v| !v.is_empty())
}

pub fn credential(headers: &HeaderMap, cookie_name: &str) -> Option<Credential> {
    bearer(headers)
        .map(Credential::Bearer)
        .or_else(|| cookie(headers, cookie_name).map(Credential::Cookie))
}

pub fn token(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    credential(headers, cookie_name).map(Credential::into_token)
}

pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    (!scheme.is_empty() && !authority.is_empty()).then(|| {
        format!(
            "{}://{}",
            scheme.to_ascii_lowercase(),
            authority.to_ascii_lowercase()
        )
    })
}

fn header_str(headers: &HeaderMap, name: impl axum::http::header::AsHeaderName) -> Option<&str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
}

fn same_host(origin: &str, headers: &HeaderMap) -> bool {
    let Some(host) = header_str(headers, HOST) else {
        return false;
    };
    origin_of(origin)
        .and_then(|o| {
            o.split_once("://")
                .map(|(_, authority)| authority.to_string())
        })
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

pub fn cross_site_request(method: &Method, headers: &HeaderMap, allowed: &[String]) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return false;
    }
    let origin = header_str(headers, ORIGIN);
    if origin
        .and_then(origin_of)
        .is_some_and(|origin| allowed.contains(&origin))
    {
        return false;
    }
    match header_str(headers, "sec-fetch-site")
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("same-origin" | "none") => false,
        Some(_) => true,
        None => origin.is_some_and(|origin| !same_host(origin, headers)),
    }
}

pub fn allowed_origins(state: &AppState) -> Vec<String> {
    let settings = state.services.settings();
    state
        .config
        .cors_origins
        .iter()
        .map(String::as_str)
        .chain(state.config.public_url.as_deref())
        .chain(settings.public_url.as_deref())
        .filter_map(origin_of)
        .collect()
}

pub fn cross_site_rejected() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        CROSS_SITE,
        "Запрос пришёл с другого сайта и отклонён. Откройте панель по её адресу и повторите",
    )
}

pub fn request_credential(
    state: &AppState,
    method: &Method,
    headers: &HeaderMap,
    cookie_name: &str,
) -> ApiResult<Option<Credential>> {
    let Some(credential) = credential(headers, cookie_name) else {
        return Ok(None);
    };
    if credential.from_cookie() && cross_site_request(method, headers, &allowed_origins(state)) {
        return Err(cross_site_rejected());
    }
    Ok(Some(credential))
}

async fn admin_principal(
    state: &AppState,
    method: &Method,
    headers: &HeaderMap,
) -> ApiResult<AdminPrincipal> {
    let credential = request_credential(state, method, headers, ADMIN_COOKIE)?
        .ok_or_else(ApiError::unauthorized)?;
    Ok(state
        .services
        .auth()
        .admin_session(credential.token())
        .await?)
}

async fn client_principal(
    state: &AppState,
    method: &Method,
    headers: &HeaderMap,
) -> ApiResult<ClientPrincipal> {
    let credential = request_credential(state, method, headers, CLIENT_COOKIE)?
        .ok_or_else(ApiError::unauthorized)?;
    let principal = state
        .services
        .auth()
        .client_session(credential.token())
        .await?;
    if credential.from_cookie() && principal.channel == ClientChannel::Miniapp {
        return Err(ApiError::unauthorized());
    }
    Ok(principal)
}

pub const FORWARDED_FOR: &str = "x-forwarded-for";
pub const PEER_SAMPLE: u32 = 20;

pub fn is_public(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

pub fn default_gateway(route_table: &str) -> Option<IpAddr> {
    route_table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let destination = *fields.get(1)?;
        let gateway = *fields.get(2)?;
        let mask = *fields.get(7)?;
        if destination != "00000000" || mask != "00000000" {
            return None;
        }
        let raw = u32::from_str_radix(gateway, 16)
            .ok()
            .filter(|raw| *raw != 0)?;
        Some(IpAddr::V4(Ipv4Addr::from(raw.to_ne_bytes())))
    })
}

fn in_container() -> bool {
    ["/.dockerenv", "/run/.containerenv"]
        .iter()
        .any(|path| std::path::Path::new(path).exists())
}

pub fn docker_gateway() -> Option<IpAddr> {
    static GATEWAY: OnceLock<Option<IpAddr>> = OnceLock::new();
    *GATEWAY.get_or_init(|| {
        let gateway = in_container()
            .then(|| std::fs::read_to_string("/proc/net/route").ok())
            .flatten()
            .and_then(|table| default_gateway(&table))
            .filter(|ip| !is_public(*ip));
        match gateway {
            Some(ip) => tracing::info!(
                gateway = %ip,
                "SN_TRUSTED_PROXIES не задан: X-Forwarded-For принимается от loopback и шлюза Docker {ip}"
            ),
            None => tracing::info!(
                "SN_TRUSTED_PROXIES не задан: X-Forwarded-For принимается только от loopback"
            ),
        }
        gateway
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProxies {
    listed: Vec<IpAddr>,
    gateway: Option<IpAddr>,
}

impl TrustedProxies {
    pub fn new(listed: &[IpAddr], gateway: Option<IpAddr>) -> Self {
        TrustedProxies {
            listed: listed.iter().map(|ip| ip.to_canonical()).collect(),
            gateway: gateway
                .map(|ip| ip.to_canonical())
                .filter(|ip| !is_public(*ip)),
        }
    }

    pub fn from_config(listed: &[IpAddr]) -> Self {
        if listed.is_empty() {
            Self::new(listed, docker_gateway())
        } else {
            Self::new(listed, None)
        }
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        if self.listed.is_empty() {
            ip.is_loopback() || self.gateway == Some(ip)
        } else {
            self.listed.contains(&ip)
        }
    }
}

fn hop_ip(hop: &str) -> Option<IpAddr> {
    let hop = hop.trim();
    hop.parse::<IpAddr>()
        .or_else(|_| hop.parse::<SocketAddr>().map(|addr| addr.ip()))
        .ok()
        .map(|ip| ip.to_canonical())
}

pub fn forwarded_client(peer: IpAddr, headers: &HeaderMap, proxies: &TrustedProxies) -> IpAddr {
    let mut client = peer.to_canonical();
    if !proxies.contains(client) {
        return client;
    }
    let hops: Vec<&str> = headers
        .get_all(FORWARDED_FOR)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .collect();
    for hop in hops.iter().rev() {
        let Some(ip) = hop_ip(hop) else {
            break;
        };
        client = ip;
        if !proxies.contains(client) {
            break;
        }
    }
    client
}

pub fn client_ip(
    parts_ip: Option<IpAddr>,
    headers: &HeaderMap,
    trusted: &[IpAddr],
) -> Option<IpAddr> {
    let peer = parts_ip?;
    Some(forwarded_client(
        peer,
        headers,
        &TrustedProxies::from_config(trusted),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyAdvice {
    UntrustedProxy(IpAddr),
    SingleSource(IpAddr),
}

#[derive(Debug, Default)]
struct WatchState {
    forwarded_reported: bool,
    sampling_done: bool,
    seen: u32,
    source: Option<IpAddr>,
}

#[derive(Debug, Default)]
pub struct PeerWatch {
    state: Mutex<WatchState>,
}

impl PeerWatch {
    pub const fn new() -> Self {
        PeerWatch {
            state: Mutex::new(WatchState {
                forwarded_reported: false,
                sampling_done: false,
                seen: 0,
                source: None,
            }),
        }
    }

    pub fn observe(
        &self,
        peer: IpAddr,
        proxies: &TrustedProxies,
        forwarded: bool,
    ) -> Option<ProxyAdvice> {
        let peer = peer.to_canonical();
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        let suspicious = !is_public(peer) && !proxies.contains(peer);
        if suspicious && forwarded && !state.forwarded_reported {
            state.forwarded_reported = true;
            state.sampling_done = true;
            return Some(ProxyAdvice::UntrustedProxy(peer));
        }
        if state.sampling_done {
            return None;
        }
        if !suspicious || state.source.is_some_and(|source| source != peer) {
            state.sampling_done = true;
            return None;
        }
        state.source = Some(peer);
        state.seen += 1;
        if state.seen >= PEER_SAMPLE {
            state.sampling_done = true;
            return Some(ProxyAdvice::SingleSource(peer));
        }
        None
    }
}

static PEER_WATCH: PeerWatch = PeerWatch::new();

fn report(advice: ProxyAdvice) {
    match advice {
        ProxyAdvice::UntrustedProxy(peer) => tracing::warn!(
            peer = %peer,
            "запрос пришёл через прокси {peer} с X-Forwarded-For, но этого адреса нет в SN_TRUSTED_PROXIES: \
             настоящие адреса клиентов не видны, лимиты по IP общие для всех. Добавьте {peer} в SN_TRUSTED_PROXIES"
        ),
        ProxyAdvice::SingleSource(peer) => tracing::warn!(
            peer = %peer,
            "первые {PEER_SAMPLE} запросов пришли с одного внутреннего адреса {peer}, он не в SN_TRUSTED_PROXIES: \
             если это обратный прокси, пусть он передаёт X-Forwarded-For, а адрес {peer} добавьте в SN_TRUSTED_PROXIES"
        ),
    }
}

pub struct CurrentAdmin(pub AdminPrincipal);

impl CurrentAdmin {
    pub fn require(&self, permission: Permission) -> ApiResult<()> {
        if self.0.can(permission) {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }

    pub fn can(&self, permission: Permission) -> bool {
        self.0.can(permission)
    }

    pub fn actor(&self) -> Actor {
        Actor::Admin(self.0.admin_id)
    }
}

impl FromRequestParts<AppState> for CurrentAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(principal) = parts.extensions.get::<AdminPrincipal>() {
            return Ok(CurrentAdmin(principal.clone()));
        }
        Ok(CurrentAdmin(
            admin_principal(state, &parts.method, &parts.headers).await?,
        ))
    }
}

pub struct CurrentClient(pub ClientPrincipal);

impl CurrentClient {
    pub fn actor(&self) -> Actor {
        Actor::Client(self.0.client_id)
    }

    pub fn require_full(&self) -> ApiResult<()> {
        if self.0.full_access {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }

    pub fn channel(&self) -> ClientChannel {
        self.0.channel
    }
}

impl FromRequestParts<AppState> for CurrentClient {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(principal) = parts.extensions.get::<ClientPrincipal>() {
            return Ok(CurrentClient(principal.clone()));
        }
        Ok(CurrentClient(
            client_principal(state, &parts.method, &parts.headers).await?,
        ))
    }
}

pub struct Meta(pub RequestMeta);

impl FromRequestParts<AppState> for Meta {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let ip = peer.map(|peer| {
            let proxies = TrustedProxies::from_config(&state.config.trusted_proxies);
            let forwarded = parts.headers.contains_key(FORWARDED_FOR);
            if let Some(advice) = PEER_WATCH.observe(peer, &proxies, forwarded) {
                report(advice);
            }
            forwarded_client(peer, &parts.headers, &proxies)
        });
        Ok(Meta(RequestMeta {
            ip: ip.map(|ip| ip.to_string()),
            user_agent: parts
                .headers
                .get(USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.chars().take(300).collect()),
        }))
    }
}

pub async fn require_admin(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let principal = admin_principal(&state, request.method(), request.headers()).await;
    match principal {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(err) => axum::response::IntoResponse::into_response(err),
    }
}

pub async fn require_client(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let principal = client_principal(&state, request.method(), request.headers()).await;
    match principal {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(err) => axum::response::IntoResponse::into_response(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn токен_из_заголовка_или_куки() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("theme=dark; sn_admin=abc123"),
        );
        assert_eq!(token(&headers, ADMIN_COOKIE).as_deref(), Some("abc123"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer xyz"));
        assert_eq!(token(&headers, ADMIN_COOKIE).as_deref(), Some("xyz"));
        assert_eq!(token(&HeaderMap::new(), ADMIN_COOKIE), None);
    }

    #[test]
    fn адрес_клиента_только_через_доверенный_прокси() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.7, 198.51.100.1"),
        );
        let proxy: IpAddr = "10.0.0.2".parse().unwrap();
        assert_eq!(
            client_ip(Some(proxy), &headers, &[proxy]),
            Some("198.51.100.1".parse().unwrap())
        );
        let stranger: IpAddr = "203.0.113.50".parse().unwrap();
        assert_eq!(
            client_ip(Some(stranger), &headers, &[proxy]),
            Some(stranger)
        );
    }

    #[test]
    fn без_списка_прокси_верим_loopback_но_не_внешним_адресам() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.7"));
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(
            client_ip(Some(loopback), &headers, &[]),
            Some("203.0.113.7".parse().unwrap())
        );
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert_eq!(
            client_ip(Some(mapped), &headers, &[]),
            Some("203.0.113.7".parse().unwrap())
        );
        let stranger: IpAddr = "198.51.100.9".parse().unwrap();
        assert_eq!(client_ip(Some(stranger), &headers, &[]), Some(stranger));
    }

    fn browser(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn изменения_по_куке_только_со_своего_сайта() {
        let allowed = vec![
            origin_of("https://Panel.Example.com/").unwrap(),
            origin_of("https://cabinet.example.org").unwrap(),
        ];
        assert_eq!(allowed[0], "https://panel.example.com");
        let post = Method::POST;
        let evil = browser(&[
            ("origin", "https://evil.example"),
            ("sec-fetch-site", "cross-site"),
        ]);
        assert!(cross_site_request(&post, &evil, &allowed));
        assert!(cross_site_request(&Method::DELETE, &evil, &allowed));
        assert!(!cross_site_request(&Method::GET, &evil, &allowed));
        assert!(!cross_site_request(
            &post,
            &browser(&[
                ("origin", "https://cabinet.example.org"),
                ("sec-fetch-site", "cross-site")
            ]),
            &allowed
        ));
        assert!(!cross_site_request(
            &post,
            &browser(&[("sec-fetch-site", "same-origin")]),
            &allowed
        ));
        assert!(cross_site_request(
            &post,
            &browser(&[("sec-fetch-site", "same-site")]),
            &allowed
        ));
        assert!(!cross_site_request(
            &post,
            &browser(&[
                ("origin", "https://vpn.example.net"),
                ("host", "vpn.example.net")
            ]),
            &allowed
        ));
        assert!(cross_site_request(
            &post,
            &browser(&[
                ("origin", "https://evil.example"),
                ("host", "vpn.example.net")
            ]),
            &allowed
        ));
        assert!(cross_site_request(
            &post,
            &browser(&[("origin", "null")]),
            &allowed
        ));
        assert!(!cross_site_request(&post, &HeaderMap::new(), &allowed));
        assert_eq!(origin_of("panel.example.com"), None);
        assert_eq!(
            credential(&browser(&[("cookie", "sn_client=abc")]), CLIENT_COOKIE),
            Some(Credential::Cookie("abc".into()))
        );
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    fn forwarded(values: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(FORWARDED_FOR, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn шлюз_docker_берётся_из_таблицы_маршрутов() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     eth0\t00000000\t010012AC\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
                     eth0\t000012AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n";
        assert_eq!(default_gateway(table), Some(ip("172.18.0.1")));
        let only_local = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\n\
                          eth0\t000012AC\t00000000\t0001\t0\t0\t0\t0000FFFF\n";
        assert_eq!(default_gateway(only_local), None);
        assert_eq!(default_gateway(""), None);
    }

    #[test]
    fn по_умолчанию_доверяем_loopback_и_шлюзу_docker() {
        let proxies = TrustedProxies::new(&[], Some(ip("172.18.0.1")));
        assert!(proxies.contains(ip("127.0.0.1")));
        assert!(proxies.contains(ip("::1")));
        assert!(proxies.contains(ip("::ffff:172.18.0.1")));
        assert!(!proxies.contains(ip("172.18.0.5")));
        assert!(!proxies.contains(ip("203.0.113.7")));
        assert_eq!(
            forwarded_client(ip("172.18.0.1"), &forwarded(&["203.0.113.7"]), &proxies),
            ip("203.0.113.7")
        );
        assert_eq!(
            forwarded_client(ip("172.18.0.5"), &forwarded(&["203.0.113.7"]), &proxies),
            ip("172.18.0.5")
        );
        let public_gateway = TrustedProxies::new(&[], Some(ip("198.51.100.1")));
        assert!(!public_gateway.contains(ip("198.51.100.1")));
        let listed = TrustedProxies::new(&[ip("10.0.0.2")], Some(ip("172.18.0.1")));
        assert!(listed.contains(ip("10.0.0.2")));
        assert!(!listed.contains(ip("127.0.0.1")));
        assert!(!listed.contains(ip("172.18.0.1")));
    }

    #[test]
    fn цепочка_прокси_разбирается_справа_до_первого_чужого() {
        let proxies = TrustedProxies::new(&[ip("10.0.0.2"), ip("10.0.0.3")], None);
        assert_eq!(
            forwarded_client(
                ip("10.0.0.2"),
                &forwarded(&["198.51.100.66, 203.0.113.7", "10.0.0.3"]),
                &proxies
            ),
            ip("203.0.113.7")
        );
        assert_eq!(
            forwarded_client(ip("10.0.0.2"), &forwarded(&["203.0.113.7:51234"]), &proxies),
            ip("203.0.113.7")
        );
        assert_eq!(
            forwarded_client(ip("10.0.0.2"), &forwarded(&["[2001:db8::7]:443"]), &proxies),
            ip("2001:db8::7")
        );
        assert_eq!(
            forwarded_client(ip("10.0.0.2"), &forwarded(&["unknown"]), &proxies),
            ip("10.0.0.2")
        );
        assert_eq!(
            forwarded_client(ip("10.0.0.2"), &HeaderMap::new(), &proxies),
            ip("10.0.0.2")
        );
    }

    #[test]
    fn частные_адреса_отличаются_от_публичных() {
        for private in [
            "10.1.2.3",
            "172.18.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public(ip(private)), "{private}");
        }
        for public in ["203.0.113.7", "8.8.8.8", "100.128.0.1", "2001:db8::1"] {
            assert!(is_public(ip(public)), "{public}");
        }
    }

    #[test]
    fn предупреждение_о_прокси_без_доверия_пишется_один_раз() {
        let proxies = TrustedProxies::new(&[], None);
        let watch = PeerWatch::new();
        assert_eq!(watch.observe(ip("172.20.0.4"), &proxies, false), None);
        assert_eq!(
            watch.observe(ip("172.20.0.4"), &proxies, true),
            Some(ProxyAdvice::UntrustedProxy(ip("172.20.0.4")))
        );
        assert_eq!(watch.observe(ip("172.20.0.4"), &proxies, true), None);
        let trusted = PeerWatch::new();
        assert_eq!(trusted.observe(ip("127.0.0.1"), &proxies, true), None);
        assert_eq!(trusted.observe(ip("203.0.113.7"), &proxies, true), None);
    }

    #[test]
    fn предупреждение_если_все_запросы_с_одного_внутреннего_адреса() {
        let proxies = TrustedProxies::new(&[], None);
        let watch = PeerWatch::new();
        for _ in 1..PEER_SAMPLE {
            assert_eq!(watch.observe(ip("172.20.0.4"), &proxies, false), None);
        }
        assert_eq!(
            watch.observe(ip("172.20.0.4"), &proxies, false),
            Some(ProxyAdvice::SingleSource(ip("172.20.0.4")))
        );
        assert_eq!(watch.observe(ip("172.20.0.4"), &proxies, false), None);
        let mixed = PeerWatch::new();
        assert_eq!(mixed.observe(ip("172.20.0.4"), &proxies, false), None);
        assert_eq!(mixed.observe(ip("203.0.113.7"), &proxies, false), None);
        for _ in 0..PEER_SAMPLE {
            assert_eq!(mixed.observe(ip("172.20.0.4"), &proxies, false), None);
        }
        let behind_trusted = PeerWatch::new();
        for _ in 0..PEER_SAMPLE {
            assert_eq!(
                behind_trusted.observe(ip("127.0.0.1"), &proxies, false),
                None
            );
        }
    }
}
