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

pub fn blocked_message(ip: IpAddr) -> String {
    format!("адрес {ip} — из внутренней сети, такие адреса модулям запрещены")
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
        f.write_str(&blocked_message(self.0))
    }
}

impl Error for Blocked {}

pub fn screen(addrs: &[SocketAddr]) -> Result<(), Blocked> {
    match addrs.iter().map(SocketAddr::ip).find(|ip| !allowed(*ip)) {
        Some(ip) => Err(Blocked(ip)),
        None => Ok(()),
    }
}

struct Guard;

impl Resolve for Guard {
    fn resolve(&self, name: Name) -> Resolving {
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
            screen(&addrs).map_err(|b| Box::new(b) as Box<dyn Error + Send + Sync>)?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

pub fn client() -> &'static reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .dns_resolver(Arc::new(Guard))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
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
        assert!(screen(&[public, local]).is_ok());
        assert_eq!(
            screen(&[public, inside]),
            Err(Blocked("10.1.2.3".parse().unwrap()))
        );
    }
}
