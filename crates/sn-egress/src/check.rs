use crate::address::{Proxy, Scheme};
use crate::client::build;
use serde::Serialize;
use std::time::{Duration, Instant};

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const DETAIL_MAX: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    ProxyUnreachable,
    ProxySilent,
    ProxyAuthNeeded,
    ProxyAuthRejected,
    ProxyAuthUnsupported,
    ProxyRefusedTarget,
    ProxyProtocol,
    Tls,
    Timeout,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub problem: Problem,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reached {
    pub status: u16,
    pub millis: u64,
}

fn chain(err: &reqwest::Error) -> Vec<String> {
    let mut out = vec![err.to_string().to_lowercase()];
    let mut source = std::error::Error::source(err);
    while let Some(inner) = source {
        out.push(inner.to_string().to_lowercase());
        source = inner.source();
    }
    out
}

const SOCKS_REFUSALS: [&str; 8] = [
    "general server failure",
    "connection not allowed",
    "network unreachable",
    "host unreachable",
    "socks error: connection refused",
    "ttl expired",
    "command not supported",
    "address type not supported",
];

pub fn diagnose(err: &reqwest::Error, proxy: &Proxy) -> Problem {
    let texts = chain(err);
    let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
    let credentials = proxy.has_credentials();
    if has("credentials not accepted") {
        return Problem::ProxyAuthRejected;
    }
    if has("server does not support user/pass authentication") {
        return if credentials {
            Problem::ProxyAuthUnsupported
        } else {
            Problem::ProxyAuthNeeded
        };
    }
    if has("proxy authorization required") {
        return if credentials {
            Problem::ProxyAuthRejected
        } else {
            Problem::ProxyAuthNeeded
        };
    }
    if has("failed to create underlying connection") {
        return Problem::ProxyUnreachable;
    }
    if has("socks error:") && SOCKS_REFUSALS.iter().any(|needle| has(needle)) {
        return Problem::ProxyRefusedTarget;
    }
    if has("tunnel error: unsuccessful") {
        return Problem::ProxyRefusedTarget;
    }
    if has("tunnel error")
        || has("socks error")
        || has("unexpected end of file")
        || has("connection reset")
        || has("connection closed before message completed")
    {
        return Problem::ProxyProtocol;
    }
    if has("certificate") || has("tls") || has("handshake") {
        return Problem::Tls;
    }
    if err.is_timeout() {
        return if err.is_connect() {
            Problem::ProxyUnreachable
        } else {
            Problem::Timeout
        };
    }
    if err.is_connect() {
        return Problem::ProxyUnreachable;
    }
    Problem::Other
}

fn detail(err: &reqwest::Error, proxy: &Proxy) -> String {
    let mut deepest = None;
    let mut source = std::error::Error::source(err);
    while let Some(inner) = source {
        deepest = Some(inner.to_string());
        source = inner.source();
    }
    let text = proxy.scrub(&deepest.unwrap_or_default());
    text.chars().take(DETAIL_MAX).collect()
}

pub fn explain(problem: Problem, proxy: &Proxy, target: &str, extra: Option<&str>) -> String {
    let server = proxy.server();
    let seconds = PROBE_TIMEOUT.as_secs();
    match problem {
        Problem::ProxyUnreachable => {
            format!("Прокси {server} не отвечает: проверьте адрес, порт и что прокси запущен")
        }
        Problem::ProxySilent => format!(
            "Прокси {server} принимает соединения, но не ответил вовремя: проверьте тип в адресе (socks5 или http) и что прокси пропускает соединения к {target}"
        ),
        Problem::ProxyAuthNeeded => "Прокси просит логин и пароль".to_string(),
        Problem::ProxyAuthRejected => "Прокси не принял логин или пароль".to_string(),
        Problem::ProxyAuthUnsupported => {
            "Прокси не поддерживает вход по логину и паролю: уберите их или возьмите другой прокси"
                .to_string()
        }
        Problem::ProxyRefusedTarget => format!("Прокси не смог соединиться с {target}"),
        Problem::ProxyProtocol => match server.scheme() {
            Scheme::Http | Scheme::Https => format!(
                "Прокси ответил не как HTTP-прокси: проверьте тип в адресе, возможно, нужен socks5://{}:{}",
                server.host(),
                server.port()
            ),
            Scheme::Socks5 | Scheme::Socks5h => format!(
                "Прокси ответил не как SOCKS5: проверьте тип в адресе, возможно, нужен http://{}:{}",
                server.host(),
                server.port()
            ),
        },
        Problem::Tls => match server.scheme() {
            Scheme::Https => format!(
                "Не удалось установить защищённое соединение с прокси или с {target}: сертификат не прошёл проверку"
            ),
            _ => format!(
                "Сертификат {target} не прошёл проверку: прокси подменяет защищённое соединение, такой прокси не подойдёт"
            ),
        },
        Problem::Timeout => {
            format!("{target} не ответил через прокси за {seconds} секунд")
        }
        Problem::Other => match extra.filter(|e| !e.is_empty()) {
            Some(extra) => format!("Запрос к {target} через прокси не прошёл: {extra}"),
            None => format!("Запрос к {target} через прокси не прошёл"),
        },
    }
}

async fn speaks_socks(proxy: &Proxy) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = proxy.server();
    let talk = async {
        let mut tcp = tokio::net::TcpStream::connect((server.host(), server.port())).await?;
        tcp.write_all(&[5, 1, 0]).await?;
        let mut reply = [0u8; 2];
        tcp.read_exact(&mut reply).await?;
        Ok::<bool, std::io::Error>(reply[0] == 5)
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(3), talk).await,
        Ok(Ok(true))
    )
}

async fn listening(proxy: &Proxy) -> bool {
    let server = proxy.server();
    let connect = tokio::net::TcpStream::connect((server.host(), server.port()));
    matches!(
        tokio::time::timeout(Duration::from_secs(3), connect).await,
        Ok(Ok(_))
    )
}

pub async fn probe(proxy: &Proxy, target: &reqwest::Url) -> Result<Reached, Failure> {
    let host = target.host_str().unwrap_or_default().to_string();
    let client = build(
        reqwest::Client::builder()
            .timeout(PROBE_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0)
            .user_agent(concat!("stealthnet/", env!("CARGO_PKG_VERSION"))),
        Some(proxy),
    );
    let started = Instant::now();
    match client.get(target.clone()).send().await {
        Ok(response) => Ok(Reached {
            status: response.status().as_u16(),
            millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }),
        Err(err) => {
            let mut problem = diagnose(&err, proxy);
            if problem == Problem::ProxyUnreachable && listening(proxy).await {
                problem = Problem::ProxySilent;
            }
            if problem == Problem::ProxyRefusedTarget
                && !proxy.server().scheme().is_socks()
                && speaks_socks(proxy).await
            {
                problem = Problem::ProxyProtocol;
            }
            let extra = (problem == Problem::Other).then(|| detail(&err, proxy));
            Err(Failure {
                problem,
                text: explain(problem, proxy, &host, extra.as_deref()),
            })
        }
    }
}
