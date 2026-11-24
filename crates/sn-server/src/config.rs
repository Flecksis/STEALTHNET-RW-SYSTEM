use crate::extract::ProxyNet;
use crate::security::{CspMode, HSTS_MAX_AGE};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub public_url: Option<String>,
    pub trusted_proxies: Vec<ProxyNet>,
    pub cors_origins: Vec<String>,
    pub body_limit_bytes: usize,
    pub web_dir: Option<PathBuf>,
    pub csp: CspMode,
    pub hsts_max_age: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080),
            public_url: None,
            trusted_proxies: Vec::new(),
            cors_origins: Vec::new(),
            body_limit_bytes: 1024 * 1024,
            web_dir: None,
            csp: CspMode::default(),
            hsts_max_age: HSTS_MAX_AGE,
        }
    }
}

impl ServerConfig {
    pub fn secure_cookies(&self) -> bool {
        self.public_url
            .as_deref()
            .is_some_and(|url| url.starts_with("https://"))
    }
}
