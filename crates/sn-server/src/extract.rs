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
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;
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
    if !credential.from_cookie() && sn_services::api_keys::is_api_token(credential.token()) {
        return Ok(state
            .services
            .api_keys()
            .principal(credential.token())
            .await?);
    }
    Ok(state
        .services
        .auth()
        .admin_session(credential.token())
        .await?)
}

fn full_path(parts: &Parts) -> String {
    parts
        .extensions
        .get::<axum::extract::OriginalUri>()
        .map_or_else(
            || parts.uri.path().to_string(),
            |uri| uri.0.path().to_string(),
        )
}

const PASSWORD_CHANGE_PATHS: [&str; 3] = [
    "/api/client/me",
    "/api/client/security/password",
    "/api/client/auth/logout",
];

async fn client_principal(
    state: &AppState,
    method: &Method,
    headers: &HeaderMap,
    path: &str,
) -> ApiResult<ClientPrincipal> {
    let credential = request_credential(state, method, headers, CLIENT_COOKIE)?
        .ok_or_else(ApiError::unauthorized)?;
    let auth = state.services.auth();
    let principal = if PASSWORD_CHANGE_PATHS.contains(&path) {
        auth.client_session_pending(credential.token()).await?
    } else {
        auth.client_session(credential.token()).await?
    };
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

pub fn docker_gateway() -> Option<IpAddr> {
    static GATEWAY: OnceLock<Option<IpAddr>> = OnceLock::new();
    *GATEWAY.get_or_init(|| {
        let gateway = sn_services::server_load::in_container()
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProxyNetError {
    #[error(
        "«{0}»: ожидается IP-адрес или сеть в записи CIDR, например 10.0.0.2, 172.18.0.0/16 или fd00::/8"
    )]
    Malformed(String),
    #[error("«{value}»: длина префикса сети должна быть от 0 до {max}")]
    Prefix { value: String, max: u8 },
    #[error(
        "«{value}»: это адрес внутри сети, а не сама сеть: укажите {network} или один адрес без длины префикса"
    )]
    HostBits { value: String, network: ProxyNet },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyNet {
    network: IpAddr,
    prefix: u8,
}

const fn v4_mask(prefix: u8) -> u32 {
    match u32::MAX.checked_shl(32 - prefix as u32) {
        Some(mask) => mask,
        None => 0,
    }
}

const fn v6_mask(prefix: u8) -> u128 {
    match u128::MAX.checked_shl(128 - prefix as u32) {
        Some(mask) => mask,
        None => 0,
    }
}

fn has_host_bits(address: IpAddr, prefix: u8) -> bool {
    match address {
        IpAddr::V4(v4) => u32::from(v4) & !v4_mask(prefix) != 0,
        IpAddr::V6(v6) => u128::from(v6) & !v6_mask(prefix) != 0,
    }
}

const fn v4_net(a: u8, b: u8, prefix: u8) -> ProxyNet {
    ProxyNet {
        network: IpAddr::V4(Ipv4Addr::new(a, b, 0, 0)),
        prefix,
    }
}

const fn v6_net(first: u16, prefix: u8) -> ProxyNet {
    ProxyNet {
        network: IpAddr::V6(Ipv6Addr::new(first, 0, 0, 0, 0, 0, 0, 0)),
        prefix,
    }
}

const PRIVATE_NETS: [ProxyNet; 9] = [
    v4_net(10, 0, 8),
    v4_net(172, 16, 12),
    v4_net(192, 168, 16),
    v4_net(100, 64, 10),
    v4_net(127, 0, 8),
    v4_net(169, 254, 16),
    v6_net(0xfc00, 7),
    v6_net(0xfe80, 10),
    ProxyNet {
        network: IpAddr::V6(Ipv6Addr::LOCALHOST),
        prefix: ProxyNet::V6_BITS,
    },
];

impl ProxyNet {
    pub const V4_BITS: u8 = 32;
    pub const V6_BITS: u8 = 128;

    pub fn new(address: IpAddr, prefix: u8) -> Option<Self> {
        match address {
            IpAddr::V4(v4) if prefix <= Self::V4_BITS => Some(ProxyNet {
                network: IpAddr::V4(Ipv4Addr::from(u32::from(v4) & v4_mask(prefix))),
                prefix,
            }),
            IpAddr::V6(v6) if prefix <= Self::V6_BITS => match v6.to_ipv4_mapped() {
                Some(v4) if prefix >= Self::V6_BITS - Self::V4_BITS => {
                    Self::new(IpAddr::V4(v4), prefix - (Self::V6_BITS - Self::V4_BITS))
                }
                _ => Some(ProxyNet {
                    network: IpAddr::V6(Ipv6Addr::from(u128::from(v6) & v6_mask(prefix))),
                    prefix,
                }),
            },
            _ => None,
        }
    }

    pub fn host(address: IpAddr) -> Self {
        let network = address.to_canonical();
        ProxyNet {
            network,
            prefix: Self::bits(network),
        }
    }

    const fn bits(address: IpAddr) -> u8 {
        match address {
            IpAddr::V4(_) => Self::V4_BITS,
            IpAddr::V6(_) => Self::V6_BITS,
        }
    }

    pub const fn network(&self) -> IpAddr {
        self.network
    }

    pub const fn prefix(&self) -> u8 {
        self.prefix
    }

    pub const fn is_host(&self) -> bool {
        self.prefix == Self::bits(self.network)
    }

    pub const fn is_everyone(&self) -> bool {
        self.prefix == 0
    }

    pub fn is_private(&self) -> bool {
        PRIVATE_NETS
            .iter()
            .any(|range| range.prefix <= self.prefix && range.contains(self.network))
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                u32::from(ip) & v4_mask(self.prefix) == u32::from(network)
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                u128::from(ip) & v6_mask(self.prefix) == u128::from(network)
            }
            _ => false,
        }
    }
}

impl From<IpAddr> for ProxyNet {
    fn from(address: IpAddr) -> Self {
        ProxyNet::host(address)
    }
}

impl fmt::Display for ProxyNet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_host() {
            write!(f, "{}", self.network)
        } else {
            write!(f, "{}/{}", self.network, self.prefix)
        }
    }
}

impl FromStr for ProxyNet {
    type Err = ProxyNetError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let value = raw.trim();
        let malformed = || ProxyNetError::Malformed(value.to_string());
        let Some((address, prefix)) = value.split_once('/') else {
            return value
                .parse::<IpAddr>()
                .map(ProxyNet::host)
                .map_err(|_| malformed());
        };
        let address = address.parse::<IpAddr>().map_err(|_| malformed())?;
        if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed());
        }
        let out_of_range = || ProxyNetError::Prefix {
            value: value.to_string(),
            max: ProxyNet::bits(address),
        };
        let prefix = prefix.parse::<u8>().map_err(|_| out_of_range())?;
        let network = ProxyNet::new(address, prefix).ok_or_else(out_of_range)?;
        if has_host_bits(address, prefix) {
            return Err(ProxyNetError::HostBits {
                value: value.to_string(),
                network,
            });
        }
        Ok(network)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyList {
    pub networks: Vec<ProxyNet>,
    pub widened: Vec<(String, ProxyNet)>,
}

impl ProxyList {
    pub fn warnings(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .widened
            .iter()
            .map(|(value, network)| {
                let address = value
                    .split_once('/')
                    .map_or(value.as_str(), |(address, _)| address);
                format!(
                    "SN_TRUSTED_PROXIES: «{value}» — это адрес внутри сети, поэтому доверяем всей частной сети {network}. \
                     Если доверять нужно одному адресу, запишите {address} без длины префикса"
                )
            })
            .collect();
        let everyone = trusts_everyone(&self.networks);
        if !everyone.is_empty() {
            let listed = everyone
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" и ");
            out.push(format!(
                "SN_TRUSTED_PROXIES: {listed} — доверие всем адресам. X-Forwarded-For принимается от любого источника, \
                 поэтому любой клиент может выдать себя за другой адрес: обойти ограничения попыток по IP и оставить в журнале чужой адрес. \
                 Оставьте в списке только адреса своих прокси"
            ));
        }
        out
    }
}

pub fn trusts_everyone(networks: &[ProxyNet]) -> Vec<ProxyNet> {
    networks
        .iter()
        .copied()
        .filter(ProxyNet::is_everyone)
        .collect()
}

pub fn parse_proxy_list(raw: &str) -> Result<ProxyList, ProxyNetError> {
    let mut list = ProxyList::default();
    for item in raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|item| !item.is_empty())
    {
        let network = match item.parse::<ProxyNet>() {
            Ok(network) => network,
            Err(ProxyNetError::HostBits { value, network }) if network.is_private() => {
                list.widened.push((value, network));
                network
            }
            Err(err) => return Err(err),
        };
        list.networks.push(network);
    }
    Ok(list)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProxies {
    listed: Vec<ProxyNet>,
    gateway: Option<IpAddr>,
}

impl TrustedProxies {
    pub fn new(listed: &[IpAddr], gateway: Option<IpAddr>) -> Self {
        let listed: Vec<ProxyNet> = listed.iter().copied().map(ProxyNet::host).collect();
        Self::networks(&listed, gateway)
    }

    pub fn networks(listed: &[ProxyNet], gateway: Option<IpAddr>) -> Self {
        TrustedProxies {
            listed: listed.to_vec(),
            gateway: gateway
                .map(|ip| ip.to_canonical())
                .filter(|ip| !is_public(*ip)),
        }
    }

    pub fn from_config<T>(listed: &[T]) -> Self
    where
        T: Copy + Into<ProxyNet>,
    {
        let listed: Vec<ProxyNet> = listed.iter().copied().map(Into::into).collect();
        let gateway = if listed.is_empty() {
            docker_gateway()
        } else {
            None
        };
        Self::networks(&listed, gateway)
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        if self.listed.is_empty() {
            ip.is_loopback() || self.gateway == Some(ip)
        } else {
            self.listed.iter().any(|net| net.contains(ip))
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

pub fn client_ip<T>(parts_ip: Option<IpAddr>, headers: &HeaderMap, trusted: &[T]) -> Option<IpAddr>
where
    T: Copy + Into<ProxyNet>,
{
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
    SilentProxy(IpAddr),
}

impl ProxyAdvice {
    pub const fn peer(&self) -> IpAddr {
        match self {
            ProxyAdvice::UntrustedProxy(peer)
            | ProxyAdvice::SingleSource(peer)
            | ProxyAdvice::SilentProxy(peer) => *peer,
        }
    }

    pub fn text(&self) -> String {
        match self {
            ProxyAdvice::UntrustedProxy(peer) => format!(
                "запрос пришёл через прокси {peer} с X-Forwarded-For, но этого адреса нет в SN_TRUSTED_PROXIES: \
                 настоящие адреса клиентов не видны, лимиты по IP общие для всех. Добавьте {peer} в SN_TRUSTED_PROXIES"
            ),
            ProxyAdvice::SingleSource(peer) => format!(
                "первые {PEER_SAMPLE} запросов пришли с одного внутреннего адреса {peer}, он не в SN_TRUSTED_PROXIES: \
                 если это обратный прокси, пусть он передаёт X-Forwarded-For, а адрес {peer} добавьте в SN_TRUSTED_PROXIES"
            ),
            ProxyAdvice::SilentProxy(peer) => format!(
                "первые {PEER_SAMPLE} запросов пришли с доверенного адреса {peer} без заголовка X-Forwarded-For. \
                 Если это обратный прокси, он не передаёт адрес клиента: все клиенты видны как {peer}, лимиты по адресу у них общие. \
                 Настройте прокси, чтобы он дописывал заголовок (nginx: proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;)"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Internal,
    Silent,
}

#[derive(Debug, Default)]
struct WatchState {
    forwarded_reported: bool,
    sampling_done: bool,
    seen: u32,
    source: Option<(Pattern, IpAddr)>,
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
        let trusted = proxies.contains(peer);
        let suspicious = !trusted && !is_public(peer);
        if suspicious && forwarded && !state.forwarded_reported {
            state.forwarded_reported = true;
            state.sampling_done = true;
            return Some(ProxyAdvice::UntrustedProxy(peer));
        }
        if state.sampling_done {
            return None;
        }
        let pattern = if forwarded {
            None
        } else if trusted {
            Some(Pattern::Silent)
        } else if suspicious {
            Some(Pattern::Internal)
        } else {
            None
        };
        let sample = pattern.and_then(|pattern| match state.source {
            None => Some((pattern, peer)),
            Some((seen, first))
                if seen == pattern && (pattern == Pattern::Silent || first == peer) =>
            {
                Some((pattern, first))
            }
            Some(_) => None,
        });
        let Some((pattern, first)) = sample else {
            state.sampling_done = true;
            return None;
        };
        state.source = Some((pattern, first));
        state.seen += 1;
        if state.seen < PEER_SAMPLE {
            return None;
        }
        state.sampling_done = true;
        Some(match pattern {
            Pattern::Internal => ProxyAdvice::SingleSource(first),
            Pattern::Silent => ProxyAdvice::SilentProxy(first),
        })
    }
}

static PEER_WATCH: PeerWatch = PeerWatch::new();

fn report(advice: ProxyAdvice) {
    tracing::warn!(peer = %advice.peer(), "{}", advice.text());
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

    pub fn require_session(&self) -> ApiResult<()> {
        if self.0.api_key.is_some() {
            Err(ApiError::forbidden())
        } else {
            Ok(())
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
            client_principal(state, &parts.method, &parts.headers, &full_path(parts)).await?,
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
    let principal = client_principal(
        &state,
        request.method(),
        request.headers(),
        request.uri().path(),
    )
    .await;
    match principal {
        Ok(principal) => {
            let address = module_address(&state, &request);
            request.extensions_mut().insert(principal);
            request.extensions_mut().insert(address);
            next.run(request).await
        }
        Err(err) => axum::response::IntoResponse::into_response(err),
    }
}

fn module_address(state: &AppState, request: &Request) -> sn_modules::catalog::web::ClientAddr {
    sn_modules::catalog::web::ClientAddr(client_ip(
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|peer| peer.0.ip()),
        request.headers(),
        &state.config.trusted_proxies,
    ))
}

pub async fn with_client_address(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let address = module_address(&state, &request);
    request.extensions_mut().insert(address);
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const NO_PROXIES: &[ProxyNet] = &[];

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
            client_ip(Some(loopback), &headers, NO_PROXIES),
            Some("203.0.113.7".parse().unwrap())
        );
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert_eq!(
            client_ip(Some(mapped), &headers, NO_PROXIES),
            Some("203.0.113.7".parse().unwrap())
        );
        let stranger: IpAddr = "198.51.100.9".parse().unwrap();
        assert_eq!(
            client_ip(Some(stranger), &headers, NO_PROXIES),
            Some(stranger)
        );
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
    fn доверенный_прокси_без_x_forwarded_for_даёт_одно_предупреждение() {
        let gateway = ip("172.18.0.1");
        let proxies = TrustedProxies::new(&[], Some(gateway));
        let watch = PeerWatch::new();
        let mut advice = Vec::new();
        for _ in 0..(PEER_SAMPLE * 3) {
            advice.extend(watch.observe(gateway, &proxies, false));
        }
        assert_eq!(
            forwarded_client(gateway, &HeaderMap::new(), &proxies),
            gateway
        );
        assert_eq!(advice, [ProxyAdvice::SilentProxy(gateway)]);
        let loopback = PeerWatch::new();
        for _ in 1..PEER_SAMPLE {
            assert_eq!(loopback.observe(ip("127.0.0.1"), &proxies, false), None);
        }
        assert_eq!(
            loopback.observe(ip("127.0.0.1"), &proxies, false),
            Some(ProxyAdvice::SilentProxy(ip("127.0.0.1")))
        );
        assert_eq!(loopback.observe(ip("127.0.0.1"), &proxies, false), None);
        let listed = TrustedProxies::networks(&[net("10.0.0.0/24")], None);
        let pool = PeerWatch::new();
        let mut advice = Vec::new();
        for n in 0..(PEER_SAMPLE * 2) {
            let peer = ip(if n % 2 == 0 { "10.0.0.2" } else { "10.0.0.3" });
            advice.extend(pool.observe(peer, &listed, false));
        }
        assert_eq!(advice, [ProxyAdvice::SilentProxy(ip("10.0.0.2"))]);
    }

    #[test]
    fn прокси_с_x_forwarded_for_предупреждений_не_даёт() {
        let gateway = ip("172.18.0.1");
        let proxies = TrustedProxies::new(&[], Some(gateway));
        let forwarding = PeerWatch::new();
        for _ in 0..(PEER_SAMPLE * 2) {
            assert_eq!(forwarding.observe(gateway, &proxies, true), None);
        }
        let late = PeerWatch::new();
        for _ in 1..PEER_SAMPLE {
            assert_eq!(late.observe(gateway, &proxies, false), None);
        }
        assert_eq!(late.observe(gateway, &proxies, true), None);
        for _ in 0..PEER_SAMPLE {
            assert_eq!(late.observe(gateway, &proxies, false), None);
        }
        let direct = PeerWatch::new();
        assert_eq!(direct.observe(gateway, &proxies, false), None);
        assert_eq!(direct.observe(ip("203.0.113.7"), &proxies, false), None);
        for _ in 0..PEER_SAMPLE {
            assert_eq!(direct.observe(gateway, &proxies, false), None);
        }
        let switched = PeerWatch::new();
        assert_eq!(switched.observe(ip("172.20.0.4"), &proxies, false), None);
        for _ in 0..PEER_SAMPLE {
            assert_eq!(switched.observe(gateway, &proxies, false), None);
        }
    }

    #[test]
    fn тексты_предупреждений_говорят_что_настроить() {
        let peer = ip("172.18.0.1");
        let silent = ProxyAdvice::SilentProxy(peer);
        assert_eq!(silent.peer(), peer);
        assert_eq!(
            silent.text(),
            format!(
                "первые {PEER_SAMPLE} запросов пришли с доверенного адреса 172.18.0.1 без заголовка X-Forwarded-For. \
                 Если это обратный прокси, он не передаёт адрес клиента: все клиенты видны как 172.18.0.1, лимиты по адресу у них общие. \
                 Настройте прокси, чтобы он дописывал заголовок (nginx: proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;)"
            )
        );
        assert!(
            ProxyAdvice::UntrustedProxy(peer)
                .text()
                .ends_with("Добавьте 172.18.0.1 в SN_TRUSTED_PROXIES")
        );
        assert!(ProxyAdvice::SingleSource(peer).text().starts_with(&format!(
            "первые {PEER_SAMPLE} запросов пришли с одного внутреннего адреса 172.18.0.1"
        )));
    }

    fn net(value: &str) -> ProxyNet {
        value.parse().unwrap()
    }

    #[test]
    fn сети_прокси_разбираются_вперемешку_с_адресами() {
        let listed =
            parse_proxy_list("10.0.0.2, 172.18.0.0/16,fd00:1::/32 ::1\n2001:db8::7,").unwrap();
        assert_eq!(
            listed
                .networks
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            [
                "10.0.0.2",
                "172.18.0.0/16",
                "fd00:1::/32",
                "::1",
                "2001:db8::7"
            ]
        );
        assert!(listed.warnings().is_empty());
        assert_eq!(parse_proxy_list("  ").unwrap(), ProxyList::default());
        assert_eq!(net(" 172.18.0.0/16 ").to_string(), "172.18.0.0/16");
        assert_eq!(
            ProxyNet::new(ip("172.18.0.1"), 16),
            Some(net("172.18.0.0/16"))
        );
        assert_eq!(net("172.18.0.0/16").network(), ip("172.18.0.0"));
        assert_eq!(net("172.18.0.0/16").prefix(), 16);
        assert_eq!(net("10.0.0.2/32"), ProxyNet::host(ip("10.0.0.2")));
        assert!(net("10.0.0.2/32").is_host());
        assert!(net("2001:db8::7").is_host());
        assert!(!net("fd00::/8").is_host());
        assert_eq!(net("::ffff:10.0.0.0/104"), net("10.0.0.0/8"));
        assert_eq!(net("::ffff:10.0.0.2"), net("10.0.0.2"));
        assert_eq!(ProxyNet::from(ip("10.0.0.2")), net("10.0.0.2"));
        assert_eq!(ProxyNet::new(ip("10.0.0.0"), 33), None);
        assert_eq!(ProxyNet::new(ip("fd00::"), 129), None);
    }

    #[test]
    fn сеть_прокси_покрывает_только_свои_адреса() {
        let docker = net("172.18.0.0/16");
        assert!(docker.contains(ip("172.18.0.1")));
        assert!(docker.contains(ip("172.18.255.254")));
        assert!(docker.contains(ip("::ffff:172.18.0.9")));
        assert!(!docker.contains(ip("172.19.0.1")));
        assert!(!docker.contains(ip("2001:db8::1")));
        let ula = net("fd00::/8");
        assert!(ula.contains(ip("fd12:3456::1")));
        assert!(!ula.contains(ip("fe80::1")));
        assert!(!ula.contains(ip("10.0.0.1")));
        let subnet = net("2001:db8:1:2::/64");
        assert!(subnet.contains(ip("2001:db8:1:2:ffff::1")));
        assert!(!subnet.contains(ip("2001:db8:1:3::1")));
        assert!(net("10.0.0.2").contains(ip("10.0.0.2")));
        assert!(!net("10.0.0.2").contains(ip("10.0.0.3")));
        assert!(net("0.0.0.0/0").contains(ip("203.0.113.7")));
        assert!(!net("0.0.0.0/0").contains(ip("2001:db8::1")));
        assert!(net("::/0").contains(ip("2001:db8::1")));
        assert!(!net("::/0").contains(ip("203.0.113.7")));
    }

    #[test]
    fn неверная_запись_прокси_даёт_понятную_ошибку() {
        for bad in [
            "proxy",
            "10.0.0.0/",
            "/8",
            "10.0.0.0/8/9",
            "10.0.0.0/+8",
            "10.0.0.0/a",
            "10.0.0.0 /8",
            "10.0.0.2:8080",
            "[::1]",
            "172.18.0.0-172.18.0.255",
            "",
        ] {
            assert_eq!(
                bad.parse::<ProxyNet>(),
                Err(ProxyNetError::Malformed(bad.to_string())),
                "{bad}"
            );
        }
        assert_eq!(
            "10.0.0.0/33".parse::<ProxyNet>().unwrap_err().to_string(),
            "«10.0.0.0/33»: длина префикса сети должна быть от 0 до 32"
        );
        assert_eq!(
            "10.0.0.0/999".parse::<ProxyNet>(),
            Err(ProxyNetError::Prefix {
                value: "10.0.0.0/999".into(),
                max: 32
            })
        );
        assert_eq!(
            "fd00::/129".parse::<ProxyNet>().unwrap_err().to_string(),
            "«fd00::/129»: длина префикса сети должна быть от 0 до 128"
        );
        assert_eq!(
            "172.18.0.1/16".parse::<ProxyNet>(),
            Err(ProxyNetError::HostBits {
                value: "172.18.0.1/16".into(),
                network: net("172.18.0.0/16")
            })
        );
        assert_eq!(
            "172.18.0.1/16".parse::<ProxyNet>().unwrap_err().to_string(),
            "«172.18.0.1/16»: это адрес внутри сети, а не сама сеть: укажите 172.18.0.0/16 или один адрес без длины префикса"
        );
        assert_eq!(
            "fd00::1/8".parse::<ProxyNet>(),
            Err(ProxyNetError::HostBits {
                value: "fd00::1/8".into(),
                network: net("fd00::/8")
            })
        );
        assert_eq!(
            "::ffff:10.0.0.1/104".parse::<ProxyNet>(),
            Err(ProxyNetError::HostBits {
                value: "::ffff:10.0.0.1/104".into(),
                network: net("10.0.0.0/8")
            })
        );
        assert!("10.0.0.1/0".parse::<ProxyNet>().is_err());
        assert_eq!(
            parse_proxy_list("10.0.0.2, proxy, 10.0.0.0/8")
                .unwrap_err()
                .to_string(),
            "«proxy»: ожидается IP-адрес или сеть в записи CIDR, например 10.0.0.2, 172.18.0.0/16 или fd00::/8"
        );
    }

    #[test]
    fn частными_считаются_только_сети_целиком_внутри_частных_диапазонов() {
        for private in [
            "10.0.0.0/8",
            "10.1.0.0/16",
            "172.16.0.0/12",
            "172.31.0.0/16",
            "192.168.1.0/24",
            "100.64.0.0/10",
            "127.0.0.1",
            "169.254.0.0/16",
            "fc00::/7",
            "fd00::/8",
            "fe80::/10",
            "::1",
        ] {
            assert!(net(private).is_private(), "{private}");
        }
        for public in [
            "0.0.0.0/0",
            "8.0.0.0/7",
            "172.0.0.0/11",
            "192.168.0.0/15",
            "203.0.113.7",
            "::/0",
            "fc00::/6",
            "2001:db8::/32",
        ] {
            assert!(!net(public).is_private(), "{public}");
        }
    }

    #[test]
    fn адрес_внутри_частной_сети_принимается_как_вся_сеть_с_предупреждением() {
        let list =
            parse_proxy_list("172.18.0.1/16, 10.0.0.2, fd00::1/8 ::ffff:10.0.0.1/104").unwrap();
        assert_eq!(
            list.networks,
            [
                net("172.18.0.0/16"),
                net("10.0.0.2"),
                net("fd00::/8"),
                net("10.0.0.0/8")
            ]
        );
        assert_eq!(
            list.widened,
            [
                ("172.18.0.1/16".to_string(), net("172.18.0.0/16")),
                ("fd00::1/8".to_string(), net("fd00::/8")),
                ("::ffff:10.0.0.1/104".to_string(), net("10.0.0.0/8")),
            ]
        );
        let warnings = list.warnings();
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert_eq!(
            warnings[0],
            "SN_TRUSTED_PROXIES: «172.18.0.1/16» — это адрес внутри сети, поэтому доверяем всей частной сети 172.18.0.0/16. \
             Если доверять нужно одному адресу, запишите 172.18.0.1 без длины префикса"
        );
        assert!(warnings[1].contains("запишите fd00::1 без длины префикса"));
        for public in [
            "203.0.113.7/24",
            "10.0.0.1/0",
            "172.16.0.1/11",
            "2001:db8::1/32",
        ] {
            assert!(
                matches!(
                    parse_proxy_list(public),
                    Err(ProxyNetError::HostBits { .. })
                ),
                "{public}"
            );
        }
    }

    #[test]
    fn доверие_всем_адресам_разрешено_но_с_предупреждением() {
        let list = parse_proxy_list("0.0.0.0/0, 10.0.0.2, ::/0").unwrap();
        assert_eq!(list.networks.len(), 3);
        assert_eq!(
            trusts_everyone(&list.networks),
            [net("0.0.0.0/0"), net("::/0")]
        );
        let warnings = list.warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("SN_TRUSTED_PROXIES: 0.0.0.0/0 и ::/0 — доверие всем адресам"),
            "{}",
            warnings[0]
        );
        assert_eq!(
            trusts_everyone(&parse_proxy_list("::ffff:0.0.0.0/96").unwrap().networks),
            [net("0.0.0.0/0")]
        );
        assert!(trusts_everyone(&[net("10.0.0.0/8"), net("0.0.0.0/1")]).is_empty());
        let proxies = TrustedProxies::from_config(&list.networks);
        assert!(proxies.contains(ip("203.0.113.7")));
        assert!(proxies.contains(ip("2001:db8::1")));
    }

    #[test]
    fn доверенные_сети_пропускают_x_forwarded_for_своих_прокси() {
        let proxies = TrustedProxies::networks(
            &[net("172.18.0.0/16"), net("10.0.0.2"), net("fd00:1::/64")],
            Some(ip("172.30.0.1")),
        );
        for trusted in ["172.18.0.1", "172.18.44.3", "10.0.0.2", "fd00:1::5"] {
            assert!(proxies.contains(ip(trusted)), "{trusted}");
        }
        for stranger in ["10.0.0.3", "127.0.0.1", "172.30.0.1", "fd00:2::5"] {
            assert!(!proxies.contains(ip(stranger)), "{stranger}");
        }
        assert_eq!(
            forwarded_client(
                ip("172.18.0.9"),
                &forwarded(&["198.51.100.66, 203.0.113.7, 172.18.0.4", "fd00:1::5"]),
                &proxies
            ),
            ip("203.0.113.7")
        );
        assert_eq!(
            forwarded_client(ip("172.19.0.9"), &forwarded(&["203.0.113.7"]), &proxies),
            ip("172.19.0.9")
        );
        assert_eq!(
            client_ip(
                Some(ip("172.18.0.9")),
                &forwarded(&["203.0.113.7"]),
                &[net("172.18.0.0/16")]
            ),
            Some(ip("203.0.113.7"))
        );
        assert_eq!(
            client_ip(
                Some(ip("172.19.0.9")),
                &forwarded(&["203.0.113.7"]),
                &[net("172.18.0.0/16")]
            ),
            Some(ip("172.19.0.9"))
        );
        assert_eq!(
            TrustedProxies::from_config(&[net("172.18.0.0/16"), net("10.0.0.2")]),
            TrustedProxies::networks(&[net("172.18.0.0/16"), net("10.0.0.2")], None)
        );
        assert_eq!(
            TrustedProxies::from_config(&[ip("10.0.0.2")]),
            TrustedProxies::new(&[ip("10.0.0.2")], None)
        );
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
        for _ in 1..PEER_SAMPLE {
            assert_eq!(
                behind_trusted.observe(ip("127.0.0.1"), &proxies, false),
                None
            );
        }
        assert_eq!(
            behind_trusted.observe(ip("127.0.0.1"), &proxies, false),
            Some(ProxyAdvice::SilentProxy(ip("127.0.0.1")))
        );
    }
}
