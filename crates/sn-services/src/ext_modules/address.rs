use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub const URL_MAX: usize = 500;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const TIMEOUT: Duration = Duration::from_secs(10);
pub const LOOPBACK_ENV: &str = "SN_EXT_MODULES_LOOPBACK";

fn v4_public(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (18..20).contains(&b)))
}

fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        return Some(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
    }
    if s[0] == 0x2002 {
        return Some(Ipv4Addr::new(
            (s[1] >> 8) as u8,
            s[1] as u8,
            (s[2] >> 8) as u8,
            s[2] as u8,
        ));
    }
    None
}

fn v6_public(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    if let Some(v4) = embedded_v4(ip) {
        return v4_public(v4);
    }
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first & 0xffc0) == 0xfec0
        || (first == 0x2001 && ip.segments()[1] == 0x0db8)
        || (first == 0x0100 && ip.segments()[1..4] == [0, 0, 0]))
}

pub fn is_loopback(ip: IpAddr) -> bool {
    ip.to_canonical().is_loopback()
}

pub fn allowed(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_loopback() || v4_public(v4),
        IpAddr::V6(v6) => v6.is_loopback() || v6_public(v6),
    }
}

fn host_ip(url: &Url) -> Option<IpAddr> {
    url.host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .ok()
}

pub fn this_server(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("localhost"))
        || host_ip(url).is_some_and(is_loopback)
}

fn local_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

pub fn on_this_server(url: &Url) -> bool {
    url.host_str().is_some_and(local_name) || host_ip(url).is_some_and(is_loopback)
}

pub fn local(raw: &str) -> bool {
    Url::parse(raw.trim()).is_ok_and(|url| on_this_server(&url))
}

pub fn blocked_message(ip: IpAddr) -> String {
    format!("адрес {ip} — из внутренней сети, такие адреса модулям запрещены")
}

pub fn loopback_closed() -> String {
    format!(
        "адрес на этом же сервере закрыт для модулей: если модуль работает здесь, добавьте {LOOPBACK_ENV}=1 в deploy/.env и перезапустите панель"
    )
}

pub fn check(raw: &str) -> Result<Url, String> {
    let raw = raw.trim();
    if raw.chars().count() > URL_MAX {
        return Err(format!("адрес не длиннее {URL_MAX} символов"));
    }
    let url = Url::parse(raw)
        .map_err(|_| "нужен полный адрес вида https://example.com/путь".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if this_server(&url) => {}
        "http" => {
            return Err(
                "http:// можно только для этого же сервера: localhost или 127.0.0.1. Для других адресов нужен https://"
                    .into(),
            );
        }
        _ => return Err("адрес должен начинаться с https://".into()),
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("в адресе нет домена".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("в адресе не должно быть логина и пароля".into());
    }
    if url.fragment().is_some() {
        return Err("в адресе не должно быть части после #".into());
    }
    if let Some(ip) = host_ip(&url)
        && !allowed(ip)
    {
        return Err(blocked_message(ip));
    }
    Ok(url)
}

pub fn check_with(raw: &str, loopback: bool) -> Result<Url, String> {
    let url = check(raw)?;
    if !loopback && on_this_server(&url) {
        return Err(loopback_closed());
    }
    Ok(url)
}

pub fn setting_url(raw: &str) -> bool {
    let raw = raw.trim();
    raw.chars().count() <= URL_MAX
        && !raw.contains(char::is_whitespace)
        && Url::parse(raw).is_ok_and(|u| matches!(u.scheme(), "https" | "http") && u.has_host())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blocked(pub IpAddr);

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if is_loopback(self.0) {
            f.write_str(&loopback_closed())
        } else {
            f.write_str(&blocked_message(self.0))
        }
    }
}

impl Error for Blocked {}

pub fn screen(addrs: &[SocketAddr], loopback: bool) -> Result<(), Blocked> {
    match addrs
        .iter()
        .map(SocketAddr::ip)
        .find(|ip| !allowed(*ip) || (!loopback && is_loopback(*ip)))
    {
        Some(ip) => Err(Blocked(ip)),
        None => Ok(()),
    }
}

struct Guard {
    loopback: bool,
}

impl Resolve for Guard {
    fn resolve(&self, name: Name) -> Resolving {
        let loopback = self.loopback;
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addrs.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("домен {host} не найден"),
                )) as Box<dyn Error + Send + Sync>);
            }
            screen(&addrs, loopback).map_err(|b| Box::new(b) as Box<dyn Error + Send + Sync>)?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

pub fn client(loopback: bool) -> &'static reqwest::Client {
    static LOCAL: OnceLock<reqwest::Client> = OnceLock::new();
    static REMOTE: OnceLock<reqwest::Client> = OnceLock::new();
    let cell = if loopback { &LOCAL } else { &REMOTE };
    cell.get_or_init(|| {
        reqwest::Client::builder()
            .dns_resolver(Arc::new(Guard { loopback }))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

pub fn inside(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            v4.is_loopback() || v4.is_private() || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outside(pub IpAddr);

impl fmt::Display for Outside {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "адрес {} — не из сети модулей на этом сервере", self.0)
    }
}

impl Error for Outside {}

struct Inside;

impl Resolve for Inside {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addrs.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("модуль {host} не найден в сети модулей"),
                )) as Box<dyn Error + Send + Sync>);
            }
            if let Some(ip) = addrs.iter().map(SocketAddr::ip).find(|ip| !inside(*ip)) {
                return Err(Box::new(Outside(ip)) as Box<dyn Error + Send + Sync>);
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn inside_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(Inside))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION")))
}

pub fn internal_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        inside_builder()
            .timeout(TIMEOUT)
            .build()
            .unwrap_or_default()
    })
}

pub fn page_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        inside_builder()
            .read_timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default()
    })
}

pub fn outside_in(err: &(dyn Error + 'static)) -> Option<Outside> {
    let mut current: Option<&(dyn Error + 'static)> = Some(err);
    while let Some(e) = current {
        if let Some(outside) = e.downcast_ref::<Outside>() {
            return Some(*outside);
        }
        current = e.source();
    }
    None
}

pub fn blocked_in(err: &(dyn Error + 'static)) -> Option<Blocked> {
    let mut current: Option<&(dyn Error + 'static)> = Some(err);
    while let Some(e) = current {
        if let Some(blocked) = e.downcast_ref::<Blocked>() {
            return Some(*blocked);
        }
        current = e.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn частные_адреса_запрещены_а_свой_сервер_и_интернет_нет() {
        for ip in [
            "10.0.0.5",
            "172.16.3.4",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "0.1.2.3",
            "255.255.255.255",
            "224.0.0.1",
            "198.18.0.1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "::",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:c0a8:101::1",
        ] {
            assert!(!allowed(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "::ffff:127.0.0.1",
            "8.8.8.8",
            "185.112.59.123",
            "2a00:1450:4001::1",
        ] {
            assert!(allowed(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn адрес_модуля_https_или_этот_же_сервер() {
        assert!(check("https://hello.example.com/events").is_ok());
        assert!(check("https://hello.example.com/events?token=1").is_ok());
        assert!(check("http://127.0.0.1:8085/events").is_ok());
        assert!(check("http://localhost:8085/events").is_ok());
        assert!(check("http://[::1]:8085/events").is_ok());
        assert!(check("https://127.0.0.1:8443/").is_ok());
        for (bad, part) in [
            ("http://hello.example.com/events", "http://"),
            ("ftp://hello.example.com/", "https://"),
            ("hello.example.com/events", "полный адрес"),
            ("https://user:pass@hello.example.com/", "логина"),
            ("https://hello.example.com/#x", "#"),
            ("https://10.0.0.5/events", "внутренней сети"),
            ("https://169.254.169.254/latest", "внутренней сети"),
            ("http://2130706433/", ""),
            ("https://0x0a000001/", "внутренней сети"),
            ("https://[fd00::1]/", "внутренней сети"),
            ("http://0.0.0.0:8080/", "http://"),
        ] {
            let result = check(bad);
            if part.is_empty() {
                assert!(result.is_ok(), "{bad}");
                continue;
            }
            let message = result.expect_err(bad);
            assert!(message.contains(part), "{bad}: {message}");
        }
    }

    #[test]
    fn резолвер_отказывает_если_домен_ведёт_во_внутреннюю_сеть() {
        let public: SocketAddr = "8.8.8.8:443".parse().unwrap();
        let inside: SocketAddr = "10.1.2.3:443".parse().unwrap();
        let local: SocketAddr = "127.0.0.1:443".parse().unwrap();
        assert!(screen(&[public, local], true).is_ok());
        assert_eq!(
            screen(&[public, inside], true),
            Err(Blocked("10.1.2.3".parse().unwrap()))
        );
    }

    #[test]
    fn этот_же_сервер_только_с_разрешением_в_переменной() {
        for raw in [
            "http://127.0.0.1:8085/events",
            "http://localhost:8085/events",
            "http://[::1]:8085/events",
            "http://[::ffff:127.0.0.1]:8085/events",
            "https://127.0.0.1:8443/",
            "https://localhost./events",
            "https://module.localhost/events",
        ] {
            assert!(check_with(raw, true).is_ok(), "{raw}");
            let message = check_with(raw, false).expect_err(raw);
            assert!(message.contains(LOOPBACK_ENV), "{raw}: {message}");
            assert!(local(raw), "{raw}");
        }
        assert!(check_with("https://hello.example.com/events", false).is_ok());
        assert!(!local("https://hello.example.com/events"));
        let public: SocketAddr = "8.8.8.8:443".parse().unwrap();
        let local_v4: SocketAddr = "127.0.0.1:443".parse().unwrap();
        let local_v6: SocketAddr = "[::1]:443".parse().unwrap();
        assert!(screen(&[public, local_v4, local_v6], true).is_ok());
        assert!(screen(&[public], false).is_ok());
        let blocked = screen(&[public, local_v4], false).unwrap_err();
        assert_eq!(blocked, Blocked("127.0.0.1".parse().unwrap()));
        assert!(blocked.to_string().contains(LOOPBACK_ENV));
        assert_eq!(
            screen(&[local_v6], false),
            Err(Blocked("::1".parse().unwrap()))
        );
        assert!(
            !Blocked("10.1.2.3".parse().unwrap())
                .to_string()
                .contains(LOOPBACK_ENV)
        );
    }
}
