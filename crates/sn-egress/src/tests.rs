use crate::*;
use base64::Engine;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

const CA: &str = include_str!("testdata/ca.pem");
const CERT: &str = include_str!("testdata/server.crt");
const KEY: &str = include_str!("testdata/server.key");
const USER: &str = "shop";
const SECRET: &str = "p@ss:w/rd#?%&=+ пароль";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Tunnel,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    target: String,
    credentials: Option<(String, String)>,
}

#[derive(Clone)]
struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn url(&self, scheme: &str, credentials: Option<(&str, &str)>) -> Proxy {
        let server = ProxyServer::parse(&format!("{scheme}://127.0.0.1:{}", self.port)).unwrap();
        match credentials {
            Some((user, secret)) => Proxy::new(server, Some(user), Some(secret)).unwrap(),
            None => Proxy::new(server, None, None).unwrap(),
        }
    }
}

fn record(log: &Mutex<Vec<Seen>>, target: String, credentials: Option<(String, String)>) {
    log.lock().unwrap().push(Seen {
        target,
        credentials,
    });
}

async fn upstream() -> u16 {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(CERT.as_bytes()).unwrap()],
            PrivateKeyDer::from_pem_slice(KEY.as_bytes()).unwrap(),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match tls.read(&mut byte).await {
                        Ok(1) => head.push(byte[0]),
                        _ => return,
                    }
                }
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });
    port
}

async fn serve(
    kind: &'static str,
    auth: Option<(&'static str, &'static str)>,
    answer: Answer,
    upstream: u16,
) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let _ = if kind == "socks" {
                    socks(&mut tcp, auth, answer, upstream, &log).await
                } else {
                    http(&mut tcp, auth, answer, upstream, &log).await
                };
            });
        }
    });
    Fake { port, seen }
}

async fn socks(
    tcp: &mut TcpStream,
    auth: Option<(&str, &str)>,
    answer: Answer,
    upstream: u16,
    log: &Mutex<Vec<Seen>>,
) -> std::io::Result<()> {
    let mut head = [0u8; 2];
    tcp.read_exact(&mut head).await?;
    let mut methods = vec![0u8; usize::from(head[1])];
    tcp.read_exact(&mut methods).await?;
    let mut credentials = None;
    if let Some((user, secret)) = auth {
        if !methods.contains(&2) {
            tcp.write_all(&[5, 0xff]).await?;
            return Ok(());
        }
        tcp.write_all(&[5, 2]).await?;
        let mut version = [0u8; 2];
        tcp.read_exact(&mut version).await?;
        let mut name = vec![0u8; usize::from(version[1])];
        tcp.read_exact(&mut name).await?;
        let mut size = [0u8; 1];
        tcp.read_exact(&mut size).await?;
        let mut pass = vec![0u8; usize::from(size[0])];
        tcp.read_exact(&mut pass).await?;
        let pair = (
            String::from_utf8_lossy(&name).to_string(),
            String::from_utf8_lossy(&pass).to_string(),
        );
        let ok = pair.0 == user && pair.1 == secret;
        credentials = Some(pair);
        tcp.write_all(&[1, u8::from(!ok)]).await?;
        if !ok {
            record(log, String::new(), credentials);
            return Ok(());
        }
    } else {
        tcp.write_all(&[5, 0]).await?;
    }
    let mut request = [0u8; 4];
    tcp.read_exact(&mut request).await?;
    let host = match request[3] {
        1 => {
            let mut addr = [0u8; 4];
            tcp.read_exact(&mut addr).await?;
            Ipv4Addr::from(addr).to_string()
        }
        3 => {
            let mut size = [0u8; 1];
            tcp.read_exact(&mut size).await?;
            let mut name = vec![0u8; usize::from(size[0])];
            tcp.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).to_string()
        }
        _ => {
            let mut addr = [0u8; 16];
            tcp.read_exact(&mut addr).await?;
            Ipv6Addr::from(addr).to_string()
        }
    };
    let mut port = [0u8; 2];
    tcp.read_exact(&mut port).await?;
    record(
        log,
        format!("{host}:{}", u16::from_be_bytes(port)),
        credentials,
    );
    if answer == Answer::Refuse {
        tcp.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Ok(());
    }
    let mut up = TcpStream::connect(("127.0.0.1", upstream)).await?;
    tcp.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await?;
    tokio::io::copy_bidirectional(tcp, &mut up).await?;
    Ok(())
}

async fn http(
    tcp: &mut TcpStream,
    auth: Option<(&str, &str)>,
    answer: Answer,
    upstream: u16,
    log: &Mutex<Vec<Seen>>,
) -> std::io::Result<()> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if tcp.read(&mut byte).await? == 0 || head.len() > 8192 {
            return Ok(());
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let mut lines = text.split("\r\n");
    let first: Vec<&str> = lines.next().unwrap_or_default().split(' ').collect();
    let credentials = lines
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .starts_with("proxy-authorization: basic ")
                .then(|| line["proxy-authorization: basic ".len()..].to_string())
        })
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .ok()
        })
        .and_then(|raw| String::from_utf8(raw).ok())
        .and_then(|pair| {
            pair.split_once(':')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        });
    if first.first() != Some(&"CONNECT") {
        tcp.write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n")
            .await?;
        return Ok(());
    }
    record(
        log,
        first.get(1).copied().unwrap_or_default().to_string(),
        credentials.clone(),
    );
    if let Some((user, secret)) = auth
        && credentials != Some((user.to_string(), secret.to_string()))
    {
        tcp.write_all(
            b"HTTP/1.1 407 Proxy Authentication Required\r\nproxy-authenticate: Basic\r\ncontent-length: 0\r\n\r\n",
        )
        .await?;
        return Ok(());
    }
    if answer == Answer::Refuse {
        tcp.write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n")
            .await?;
        return Ok(());
    }
    let mut up = TcpStream::connect(("127.0.0.1", upstream)).await?;
    tcp.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await?;
    tokio::io::copy_bidirectional(tcp, &mut up).await?;
    Ok(())
}

fn trusting() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(CA.as_bytes()).unwrap())
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
}

async fn fetch(proxy: &Proxy, url: &str) -> Result<String, reqwest::Error> {
    let client = routed(trusting(), Some(proxy)).build().unwrap();
    client.get(url).send().await?.text().await
}

async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn туннель_socks5_и_http_с_логином_и_паролем() {
    let up = upstream().await;
    let socks = serve("socks", Some((USER, SECRET)), Answer::Tunnel, up).await;
    let proxy = socks.url("socks5h", Some((USER, SECRET)));
    assert_eq!(
        fetch(&proxy, "https://api.example.com/v1/models")
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(
        socks.seen(),
        vec![Seen {
            target: "api.example.com:443".into(),
            credentials: Some((USER.into(), SECRET.into())),
        }]
    );
    let local = socks.url("socks5", Some((USER, SECRET)));
    assert_eq!(fetch(&local, "https://203.0.113.10/").await.unwrap(), "ok");
    assert_eq!(socks.seen()[1].target, "203.0.113.10:443");

    let http = serve("http", Some((USER, SECRET)), Answer::Tunnel, up).await;
    let proxy = http.url("http", Some((USER, SECRET)));
    assert_eq!(
        fetch(&proxy, "https://api.example.com/bot1:SECRETTOKEN/getMe")
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(
        http.seen(),
        vec![Seen {
            target: "api.example.com:443".into(),
            credentials: Some((USER.into(), SECRET.into())),
        }]
    );
}

#[tokio::test]
async fn локальные_и_открытые_адреса_идут_мимо_прокси() {
    let up = upstream().await;
    let socks = serve("socks", None, Answer::Tunnel, up).await;
    let proxy = socks.url("socks5h", None);
    let client = routed(trusting(), Some(&proxy)).build().unwrap();
    let _ = client.get(format!("https://127.0.0.1:{up}/")).send().await;
    let _ = client
        .get(format!("http://localhost:{}/", closed_port().await))
        .send()
        .await;
    let _ = client.get("http://api.example.com:9/").send().await;
    assert!(socks.seen().is_empty(), "{:?}", socks.seen());
    let direct = routed(trusting(), None).build().unwrap();
    let _ = direct.get("https://api.example.com:9/").send().await;
    assert!(socks.seen().is_empty());
}

#[tokio::test]
async fn ошибки_прокси_понятны_и_без_пароля() {
    let up = upstream().await;
    let target = reqwest::Url::parse("https://api.example.com/").unwrap();
    let closed = closed_port().await;
    for scheme in ["socks5h", "http"] {
        let dead = Proxy::new(
            ProxyServer::parse(&format!("{scheme}://127.0.0.1:{closed}")).unwrap(),
            Some(USER),
            Some(SECRET),
        )
        .unwrap();
        let failure = probe(&dead, &target).await.unwrap_err();
        assert_eq!(failure.problem, Problem::ProxyUnreachable, "{scheme}");
        assert!(failure.text.contains(&format!("127.0.0.1:{closed}")));
    }

    let socks = serve("socks", Some((USER, SECRET)), Answer::Tunnel, up).await;
    let cases = [
        (
            socks.url("socks5h", Some((USER, "wrong"))),
            Problem::ProxyAuthRejected,
        ),
        (socks.url("socks5h", None), Problem::ProxyAuthNeeded),
    ];
    let http = serve("http", Some((USER, SECRET)), Answer::Tunnel, up).await;
    let refusing_socks = serve("socks", None, Answer::Refuse, up).await;
    let refusing_http = serve("http", None, Answer::Refuse, up).await;
    let more = [
        (
            http.url("http", Some((USER, "wrong"))),
            Problem::ProxyAuthRejected,
        ),
        (http.url("http", None), Problem::ProxyAuthNeeded),
        (
            refusing_socks.url("socks5h", None),
            Problem::ProxyRefusedTarget,
        ),
        (refusing_http.url("http", None), Problem::ProxyRefusedTarget),
        (
            socks.url("http", Some((USER, SECRET))),
            Problem::ProxyProtocol,
        ),
        (
            http.url("socks5h", Some((USER, SECRET))),
            Problem::ProxySilent,
        ),
        (socks.url("socks5h", Some((USER, SECRET))), Problem::Tls),
    ];
    for (proxy, expected) in cases.into_iter().chain(more) {
        let failure = probe(&proxy, &target).await.unwrap_err();
        assert_eq!(failure.problem, expected, "{proxy} → {}", failure.text);
        assert!(!failure.text.contains("p@ss"), "{}", failure.text);
        assert!(!failure.text.contains("w/rd"), "{}", failure.text);
        assert!(!failure.text.is_empty());
    }
    let wrong_type = probe(&socks.url("http", Some((USER, SECRET))), &target)
        .await
        .unwrap_err();
    assert!(
        wrong_type.text.contains("socks5://127.0.0.1:"),
        "{}",
        wrong_type.text
    );
}

#[tokio::test]
async fn клиент_меняется_вместе_с_планом() {
    static PROFILE: Profile = Profile {
        name: "test-switch",
        build: trusting,
    };
    let up = upstream().await;
    let first = serve("socks", None, Answer::Tunnel, up).await;
    let second = serve("http", None, Answer::Tunnel, up).await;
    let egress = Egress::new();
    let settings = |server: String, routes: &[Route], providers: &[&str]| ProxySettings {
        enabled: true,
        server: Some(server),
        username: None,
        routes: routes.iter().copied().collect(),
        providers: providers.iter().map(|p| p.to_string()).collect(),
    };
    egress.set_plan(Plan::from_panel(
        &settings(
            format!("socks5h://127.0.0.1:{}", first.port),
            &[Route::Payments],
            &["cryptomus"],
        ),
        None,
    ));
    let chosen = Http::new(&PROFILE, Route::Payments, Some("cryptomus")).bound(&egress);
    let other = Http::new(&PROFILE, Route::Payments, Some("yookassa")).bound(&egress);
    assert_eq!(
        chosen
            .get("https://api.example.com/")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
    let _ = other.get("https://api.example.com:9/").send().await;
    assert_eq!(first.seen().len(), 1);
    egress.set_plan(Plan::from_panel(
        &settings(
            format!("http://127.0.0.1:{}", second.port),
            &[Route::Payments],
            &[],
        ),
        None,
    ));
    for http in [&chosen, &other] {
        assert_eq!(
            http.get("https://api.example.com/")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
    }
    assert_eq!(first.seen().len(), 1);
    assert_eq!(second.seen().len(), 2);
    egress.set_plan(Plan::direct());
    let _ = chosen.get("https://api.example.com:9/").send().await;
    assert_eq!(second.seen().len(), 2);
    assert_eq!(first.seen().len(), 1);
}

#[test]
fn отладочный_вывод_не_раскрывает_пароль() {
    let proxy = Proxy::parse("http://shop:VerySecret1@proxy.example.com:3128").unwrap();
    let plan = Plan::from_env(&EnvProxy {
        proxy: proxy.clone(),
        routes: [Route::Telegram].into_iter().collect(),
        providers: Default::default(),
    });
    let egress = Egress::new();
    egress.set_plan(plan.clone());
    for text in [
        format!("{proxy:?}"),
        format!("{proxy}"),
        format!("{plan:?}"),
        format!("{egress:?}"),
        format!(
            "{:?}",
            EnvProxy::from_env(
                Some("http://shop:VerySecret1@proxy.example.com:3128"),
                None,
                None
            )
        ),
    ] {
        assert!(!text.contains("VerySecret1"), "{text}");
    }
}
