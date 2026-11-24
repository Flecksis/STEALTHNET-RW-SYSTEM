use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const CREDENTIAL_MAX: usize = 255;
pub const HOST_MAX: usize = 253;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    Socks5,
    Socks5h,
    Http,
    Https,
}

impl Scheme {
    pub const ALL: [Scheme; 4] = [Scheme::Socks5, Scheme::Socks5h, Scheme::Http, Scheme::Https];

    pub const fn as_str(self) -> &'static str {
        match self {
            Scheme::Socks5 => "socks5",
            Scheme::Socks5h => "socks5h",
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }

    pub fn parse(raw: &str) -> Option<Scheme> {
        Scheme::ALL
            .into_iter()
            .find(|scheme| scheme.as_str().eq_ignore_ascii_case(raw))
    }

    pub const fn default_port(self) -> u16 {
        match self {
            Scheme::Socks5 | Scheme::Socks5h => 1080,
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }

    pub const fn is_socks(self) -> bool {
        matches!(self, Scheme::Socks5 | Scheme::Socks5h)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyError {
    Empty,
    Spaces,
    Format,
    Scheme(String),
    Host,
    Port,
    Path,
    Credentials,
    Username,
    Password,
    PasswordWithoutUsername,
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProxyError::Empty => f.write_str("адрес прокси пуст"),
            ProxyError::Spaces => f.write_str("в адресе прокси не должно быть пробелов"),
            ProxyError::Format => f.write_str(
                "нужен адрес вида socks5://203.0.113.5:1080 или http://proxy.example.com:3128",
            ),
            ProxyError::Scheme(scheme) => write!(
                f,
                "тип прокси «{scheme}» не поддерживается: подойдут socks5, socks5h, http и https"
            ),
            ProxyError::Host => f.write_str("в адресе прокси нет сервера или он записан неверно"),
            ProxyError::Port => f.write_str("порт прокси — число от 1 до 65535"),
            ProxyError::Path => {
                f.write_str("в адресе прокси не должно быть пути и параметров после / ? #")
            }
            ProxyError::Credentials => {
                f.write_str("логин и пароль укажите в отдельных полях, а не в адресе")
            }
            ProxyError::Username => write!(
                f,
                "логин прокси — до {CREDENTIAL_MAX} байт, без двоеточия и пробелов"
            ),
            ProxyError::Password => write!(f, "пароль прокси — до {CREDENTIAL_MAX} байт"),
            ProxyError::PasswordWithoutUsername => f.write_str("для пароля прокси нужен и логин"),
        }
    }
}

impl std::error::Error for ProxyError {}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProxyServer {
    scheme: Scheme,
    host: String,
    port: u16,
}

impl ProxyServer {
    pub fn new(scheme: Scheme, host: &str, port: u16) -> Result<Self, ProxyError> {
        if port == 0 {
            return Err(ProxyError::Port);
        }
        Ok(ProxyServer {
            scheme,
            host: clean_host(host)?,
            port,
        })
    }

    pub fn parse(raw: &str) -> Result<Self, ProxyError> {
        let parts = split(raw)?;
        if parts.userinfo.is_some() {
            return Err(ProxyError::Credentials);
        }
        Ok(parts.server)
    }

    pub const fn scheme(&self) -> Scheme {
        self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

impl fmt::Display for ProxyServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme.as_str(), self.authority())
    }
}

impl fmt::Debug for ProxyServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Proxy {
    server: ProxyServer,
    username: Option<String>,
    password: Option<String>,
    url: reqwest::Url,
}

impl Proxy {
    pub fn new(
        server: ProxyServer,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<Self, ProxyError> {
        let username = username.map(str::trim).filter(|u| !u.is_empty());
        let password = password.filter(|p| !p.is_empty());
        if let Some(name) = username
            && (name.len() > CREDENTIAL_MAX
                || name.contains(':')
                || name.chars().any(|c| c.is_whitespace() || c.is_control()))
        {
            return Err(ProxyError::Username);
        }
        if let Some(secret) = password
            && (secret.len() > CREDENTIAL_MAX || secret.chars().any(char::is_control))
        {
            return Err(ProxyError::Password);
        }
        if password.is_some() && username.is_none() {
            return Err(ProxyError::PasswordWithoutUsername);
        }
        let userinfo = match (username, password) {
            (Some(name), Some(secret)) => format!("{}:{}@", encode(name), encode(secret)),
            (Some(name), None) => format!("{}@", encode(name)),
            _ => String::new(),
        };
        let url = reqwest::Url::parse(&format!(
            "{}://{userinfo}{}",
            server.scheme.as_str(),
            server.authority()
        ))
        .map_err(|_| ProxyError::Format)?;
        Ok(Proxy {
            server,
            username: username.map(str::to_string),
            password: password.map(str::to_string),
            url,
        })
    }

    pub fn parse(raw: &str) -> Result<Self, ProxyError> {
        let parts = split(raw)?;
        let (username, password) = match parts.userinfo {
            None => (None, None),
            Some(info) => {
                let (name, secret) = match info.split_once(':') {
                    Some((name, secret)) => (name, Some(secret)),
                    None => (info, None),
                };
                let name = decode(name).ok_or(ProxyError::Username)?;
                let secret = secret
                    .map(|s| decode(s).ok_or(ProxyError::Password))
                    .transpose()?;
                (Some(name), secret)
            }
        };
        Proxy::new(parts.server, username.as_deref(), password.as_deref())
    }

    pub const fn server(&self) -> &ProxyServer {
        &self.server
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub const fn has_password(&self) -> bool {
        self.password.is_some()
    }

    pub const fn has_credentials(&self) -> bool {
        self.username.is_some()
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub(crate) fn url(&self) -> &reqwest::Url {
        &self.url
    }

    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_string();
        if let Some(secret) = self.password.as_deref().filter(|s| s.chars().count() >= 3) {
            out = out.replace(secret, "***");
            out = out.replace(&encode(secret), "***");
        }
        out
    }
}

impl fmt::Display for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.username {
            Some(name) => write!(
                f,
                "{}://{}@{}",
                self.server.scheme.as_str(),
                name,
                self.server.authority()
            ),
            None => fmt::Display::fmt(&self.server, f),
        }
    }
}

impl fmt::Debug for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let password = if self.password.is_some() { ":***" } else { "" };
        match &self.username {
            Some(name) => write!(
                f,
                "Proxy({}://{name}{password}@{})",
                self.server.scheme.as_str(),
                self.server.authority()
            ),
            None => write!(f, "Proxy({})", self.server),
        }
    }
}

struct Parts<'a> {
    server: ProxyServer,
    userinfo: Option<&'a str>,
}

fn split(raw: &str) -> Result<Parts<'_>, ProxyError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ProxyError::Empty);
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ProxyError::Spaces);
    }
    let (scheme, rest) = trimmed.split_once("://").ok_or(ProxyError::Format)?;
    let scheme = Scheme::parse(scheme).ok_or_else(|| {
        ProxyError::Scheme(scheme.chars().take(16).collect::<String>().to_lowercase())
    })?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (userinfo, hostport) = match rest.rsplit_once('@') {
        Some((info, hostport)) => (Some(info), hostport),
        None => (None, rest),
    };
    if hostport.contains(['/', '?', '#']) {
        return Err(ProxyError::Path);
    }
    if userinfo.is_some_and(|info| info.is_empty()) {
        return Err(ProxyError::Username);
    }
    let (host, port) = if let Some(after) = hostport.strip_prefix('[') {
        let (host, tail) = after.split_once(']').ok_or(ProxyError::Host)?;
        let port = match tail {
            "" => None,
            tail => Some(tail.strip_prefix(':').ok_or(ProxyError::Host)?),
        };
        (host, port)
    } else {
        match hostport.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (hostport, None),
        }
    };
    let port = match port {
        None => scheme.default_port(),
        Some(port) => port
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or(ProxyError::Port)?,
    };
    Ok(Parts {
        server: ProxyServer::new(scheme, host, port)?,
        userinfo,
    })
}

fn clean_host(raw: &str) -> Result<String, ProxyError> {
    let host = raw.trim().trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() || host.len() > HOST_MAX {
        return Err(ProxyError::Host);
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip.to_string());
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let labels_ok = !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    if labels_ok {
        Ok(host)
    } else {
        Err(ProxyError::Host)
    }
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn outward(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => public_v4(ip),
        Ok(IpAddr::V6(ip)) => public_v6(ip),
        Err(_) => public_name(bare),
    }
}

fn public_name(name: &str) -> bool {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if !name.contains('.') {
        return false;
    }
    ![".localhost", ".local", ".internal", ".lan", ".home.arpa"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (b == 18 || b == 19)))
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn адрес_разбирается_и_нормализуется() {
        let proxy = Proxy::parse("SOCKS5H://User:p%40ss@Proxy.Example.COM:1080/").unwrap();
        assert_eq!(proxy.server().scheme(), Scheme::Socks5h);
        assert_eq!(proxy.server().host(), "proxy.example.com");
        assert_eq!(proxy.server().port(), 1080);
        assert_eq!(proxy.username(), Some("User"));
        assert_eq!(proxy.password(), Some("p@ss"));
        assert_eq!(proxy.to_string(), "socks5h://User@proxy.example.com:1080");
        let v6 = Proxy::parse("http://[2001:db8::1]:3128").unwrap();
        assert_eq!(v6.to_string(), "http://[2001:db8::1]:3128");
        assert_eq!(
            ProxyServer::parse("socks5://203.0.113.5").unwrap().port(),
            1080
        );
        assert_eq!(ProxyServer::parse("http://10.0.0.1").unwrap().port(), 80);
        assert_eq!(
            ProxyServer::parse("https://proxy.example.com")
                .unwrap()
                .port(),
            443
        );
    }

    #[test]
    fn плохие_адреса_отклоняются_без_пароля_в_тексте() {
        for (raw, expected) in [
            ("", ProxyError::Empty),
            ("socks5://host :1080", ProxyError::Spaces),
            ("proxy.example.com:1080", ProxyError::Format),
            (
                "ftp://proxy.example.com:21",
                ProxyError::Scheme("ftp".into()),
            ),
            (
                "socks4://proxy.example.com:1080",
                ProxyError::Scheme("socks4".into()),
            ),
            ("socks5://", ProxyError::Host),
            ("socks5://:1080", ProxyError::Host),
            ("socks5://proxy_example:1080", ProxyError::Host),
            ("socks5://proxy.example.com:0", ProxyError::Port),
            ("socks5://proxy.example.com:70000", ProxyError::Port),
            ("http://proxy.example.com:3128/path", ProxyError::Path),
            ("http://proxy.example.com:3128?x=1", ProxyError::Path),
            ("http://[2001:db8::1:3128", ProxyError::Host),
        ] {
            assert_eq!(Proxy::parse(raw).unwrap_err(), expected, "{raw}");
        }
        let err = Proxy::parse("socks5://user:Sup3rSecret!@proxy.example.com:abc").unwrap_err();
        assert_eq!(err, ProxyError::Port);
        assert!(!err.to_string().contains("Sup3rSecret"));
        assert_eq!(
            ProxyServer::parse("socks5://user:pass@proxy.example.com:1080").unwrap_err(),
            ProxyError::Credentials
        );
    }

    #[test]
    fn логин_и_пароль_проверяются() {
        let server = ProxyServer::parse("http://proxy.example.com:3128").unwrap();
        assert_eq!(
            Proxy::new(server.clone(), Some("us:er"), Some("x")).unwrap_err(),
            ProxyError::Username
        );
        assert_eq!(
            Proxy::new(server.clone(), None, Some("x")).unwrap_err(),
            ProxyError::PasswordWithoutUsername
        );
        assert_eq!(
            Proxy::new(server.clone(), Some("user"), Some(&"x".repeat(256))).unwrap_err(),
            ProxyError::Password
        );
        let tricky = "p@ss:w/rd#?%&=+ пароль";
        let proxy = Proxy::new(server, Some("user"), Some(tricky)).unwrap();
        assert_eq!(proxy.password(), Some(tricky));
        assert!(!format!("{proxy:?}").contains("rd#"));
        assert!(!proxy.to_string().contains("p@ss"));
        let back = Proxy::parse(proxy.url().as_str()).unwrap();
        assert_eq!(back.password(), Some(tricky));
        assert_eq!(proxy.scrub(&format!("ошибка {tricky}")), "ошибка ***");
    }

    #[test]
    fn через_прокси_идут_только_внешние_адреса_https() {
        let outward_url = |raw: &str| outward(&reqwest::Url::parse(raw).unwrap());
        for raw in [
            "https://api.telegram.org/bot1:x/getMe",
            "https://api.openai.com/v1",
            "https://203.0.113.10:8443/",
            "https://[2001:db8::10]/",
        ] {
            assert!(outward_url(raw), "{raw}");
        }
        for raw in [
            "http://api.telegram.org/",
            "https://localhost:8081/",
            "https://bot-api.localhost/",
            "https://remnawave:3000/",
            "https://127.0.0.1/",
            "https://10.1.2.3/",
            "https://172.20.0.5/",
            "https://192.168.1.10/",
            "https://169.254.169.254/",
            "https://100.64.0.1/",
            "https://[::1]/",
            "https://[fd00::1]/",
            "https://[fe80::1]/",
            "https://[::ffff:10.0.0.1]/",
            "https://panel.internal/",
            "https://printer.local/",
        ] {
            assert!(!outward_url(raw), "{raw}");
        }
    }
}
