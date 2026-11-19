use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

const METADATA_V4: Ipv4Addr = Ipv4Addr::new(100, 100, 100, 200);
const METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

pub fn forbidden(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4 == METADATA_V4
        }
        IpAddr::V6(v6) => {
            (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6 == METADATA_V6
        }
    }
}

pub fn refused_text(ip: IpAddr) -> String {
    format!(
        "адрес {ip} — служебный адрес облака или самой сети, к нему панель не подключается. Укажите адрес панели Remnawave"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused(pub IpAddr);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&refused_text(self.0))
    }
}

impl Error for Refused {}

pub fn refused_in(err: &(dyn Error + 'static)) -> Option<IpAddr> {
    let mut source: Option<&(dyn Error + 'static)> = Some(err);
    while let Some(inner) = source {
        if let Some(Refused(ip)) = inner.downcast_ref::<Refused>() {
            return Some(*ip);
        }
        source = inner.source();
    }
    None
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Screened;

impl Resolve for Screened {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if let Some(ip) = found.iter().map(SocketAddr::ip).find(|ip| forbidden(*ip)) {
                return Err(Box::new(Refused(ip)) as Box<dyn Error + Send + Sync>);
            }
            let addrs: Addrs = Box::new(found.into_iter());
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn ip(raw: &str) -> IpAddr {
        raw.parse().unwrap()
    }

    #[test]
    fn служебные_адреса_облака_запрещены_а_частные_сети_нет() {
        for bad in [
            "169.254.169.254",
            "169.254.0.1",
            "100.100.100.200",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "fe80::1",
            "fd00:ec2::254",
            "::",
            "::ffff:169.254.169.254",
        ] {
            assert!(forbidden(ip(bad)), "{bad}");
        }
        for good in [
            "127.0.0.1",
            "10.0.0.5",
            "172.18.0.3",
            "192.168.1.10",
            "100.64.1.2",
            "203.0.113.7",
            "::1",
            "fd12:3456::1",
            "2001:db8::1",
        ] {
            assert!(!forbidden(ip(good)), "{good}");
        }
    }

    #[tokio::test]
    async fn резолвер_не_отдаёт_служебные_адреса() {
        let refused = match Screened
            .resolve(Name::from_str("169.254.169.254").unwrap())
            .await
        {
            Ok(_) => panic!("служебный адрес пропущен"),
            Err(err) => err,
        };
        assert_eq!(refused_in(refused.as_ref()), Some(ip("169.254.169.254")));
        let local: Vec<SocketAddr> = Screened
            .resolve(Name::from_str("127.0.0.1").unwrap())
            .await
            .unwrap()
            .collect();
        assert_eq!(local[0].ip(), ip("127.0.0.1"));
    }
}
