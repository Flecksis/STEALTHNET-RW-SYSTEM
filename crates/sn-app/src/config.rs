use anyhow::{Context, Result, anyhow, bail};
use sn_server::ServerConfig;
use sn_server::extract::parse_proxy_list;
use sn_server::security::{CspMode, HSTS_MAX_AGE};
use sn_services::updates::{BuildInfo, UpdateConfig};
use sn_services::{EnvFallbacks, RemnaConnection};
use std::collections::HashMap;
use std::net::SocketAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub database_url: String,
    pub db_max_connections: u32,
    pub secret_key: Option<String>,
    pub log_filter: String,
    pub log_format: LogFormat,
    pub migrate_on_start: bool,
    pub run_worker: bool,
    pub run_bot: bool,
    pub bot_concurrency: usize,
    pub telegram_api: String,
    pub server: ServerConfig,
    pub env: EnvFallbacks,
    pub updates: UpdateConfig,
    pub backup_dir: Option<std::path::PathBuf>,
    pub backup_host_dir: Option<String>,
    pub warnings: Vec<String>,
}

const TELEGRAM_API: &str = "https://api.telegram.org";

fn flag(vars: &HashMap<String, String>, key: &str, default: bool) -> Result<bool> {
    match vars.get(key).map(|v| v.trim().to_ascii_lowercase()) {
        None => Ok(default),
        Some(v) if v.is_empty() => Ok(default),
        Some(v) if ["1", "true", "yes", "on"].contains(&v.as_str()) => Ok(true),
        Some(v) if ["0", "false", "no", "off"].contains(&v.as_str()) => Ok(false),
        Some(v) => bail!("{key}: ожидается true или false, получено «{v}»"),
    }
}

fn text(vars: &HashMap<String, String>, key: &str) -> Option<String> {
    vars.get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn list(vars: &HashMap<String, String>, key: &str) -> Vec<String> {
    text(vars, key)
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn updates(vars: &HashMap<String, String>) -> Result<UpdateConfig> {
    let build = BuildInfo::new(
        text(vars, "SN_BUILD_COMMIT").as_deref(),
        text(vars, "SN_BUILD_DATE").as_deref(),
    )
    .with_label(text(vars, "SN_BUILD_VERSION").as_deref());
    let mut config = UpdateConfig::new(build);
    config.dir = text(vars, "SN_UPDATER_DIR").map(std::path::PathBuf::from);
    if let Some(registry) = text(vars, "SN_UPDATE_REGISTRY") {
        if !(registry.starts_with("https://") || registry.starts_with("http://")) {
            bail!("SN_UPDATE_REGISTRY должен начинаться с https://");
        }
        config.registry = registry;
    }
    if let Some(repository) = text(vars, "SN_UPDATE_REPOSITORY") {
        config.repository = repository;
    }
    if let Some(tag) = text(vars, "SN_UPDATE_TAG") {
        config.tag = tag;
    }
    config.checks_allowed = flag(vars, "SN_UPDATE_CHECK", true)?;
    config.helper_why = text(vars, "SN_HELPER_WHY");
    config.modules_host = text(vars, "SN_MODULES_HOST");
    Ok(config)
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_vars(&std::env::vars().collect())
    }

    pub fn from_vars(vars: &HashMap<String, String>) -> Result<Self> {
        let database_url = text(vars, "DATABASE_URL").context("не задан DATABASE_URL")?;
        let bind: SocketAddr = text(vars, "SN_BIND")
            .unwrap_or_else(|| "0.0.0.0:8080".into())
            .parse()
            .context("SN_BIND: ожидается адрес вида 0.0.0.0:8080")?;
        let public_url = text(vars, "SN_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string());
        if let Some(url) = &public_url
            && !(url.starts_with("https://") || url.starts_with("http://"))
        {
            bail!("SN_PUBLIC_URL должен начинаться с https://");
        }
        let proxies = parse_proxy_list(&text(vars, "SN_TRUSTED_PROXIES").unwrap_or_default())
            .map_err(|err| anyhow!("SN_TRUSTED_PROXIES: {err}"))?;
        let mut warnings = proxies.warnings();
        let db_max_connections = text(vars, "SN_DB_MAX_CONNECTIONS")
            .map(|v| {
                v.parse::<u32>()
                    .context("SN_DB_MAX_CONNECTIONS: ожидается число")
            })
            .transpose()?
            .unwrap_or(16)
            .clamp(2, 200);
        let bot_concurrency = text(vars, "SN_BOT_CONCURRENCY")
            .map(|v| {
                v.parse::<usize>()
                    .context("SN_BOT_CONCURRENCY: ожидается число")
            })
            .transpose()?
            .unwrap_or(sn_bot::DEFAULT_CONCURRENCY)
            .clamp(1, 256);
        let telegram_api = text(vars, "SN_TELEGRAM_API")
            .map(|u| u.trim_end_matches('/').to_string())
            .unwrap_or_else(|| TELEGRAM_API.into());
        if !(telegram_api.starts_with("https://") || telegram_api.starts_with("http://")) {
            bail!("SN_TELEGRAM_API: ожидается адрес Bot API вида https://api.telegram.org");
        }
        let custom_api = (telegram_api != TELEGRAM_API).then(|| telegram_api.clone());
        let log_format = match text(vars, "SN_LOG_FORMAT").as_deref() {
            None | Some("text") => LogFormat::Text,
            Some("json") => LogFormat::Json,
            Some(other) => bail!("SN_LOG_FORMAT: text или json, получено «{other}»"),
        };
        let csp = match text(vars, "SN_CSP") {
            None => CspMode::default(),
            Some(raw) => CspMode::parse(&raw)
                .ok_or_else(|| anyhow!("SN_CSP: admin, all или report, получено «{raw}»"))?,
        };
        let hsts_max_age = text(vars, "SN_HSTS_MAX_AGE")
            .map(|v| {
                v.parse::<u64>()
                    .context("SN_HSTS_MAX_AGE: ожидается число секунд, 0 — выключить")
            })
            .transpose()?
            .unwrap_or(HSTS_MAX_AGE);
        let routes = text(vars, "SN_PROXY_ROUTES");
        let chosen = text(vars, "SN_PROXY_PAYMENTS");
        let proxy = sn_egress::EnvProxy::from_env(
            text(vars, "SN_PROXY_URL").as_deref(),
            routes.as_deref(),
            chosen.as_deref(),
        )
        .map_err(|err| anyhow!("{err}"))?;
        if proxy.is_none() && (routes.is_some() || chosen.is_some()) {
            warnings
                .push("SN_PROXY_ROUTES и SN_PROXY_PAYMENTS без SN_PROXY_URL не действуют".into());
        }
        let ignored: Vec<&str> = [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ]
        .into_iter()
        .filter(|key| text(vars, key).is_some())
        .collect();
        if !ignored.is_empty() {
            warnings.push(format!(
                "{} в окружении панель не применяет: прокси для Telegram, платёжек, ИИ, почты и обновлений задаётся в SN_PROXY_URL или в «Настройки → Общие», Remnawave и внешние модули ходят напрямую",
                ignored.join(", ")
            ));
        }
        let remna = match (text(vars, "REMNAWAVE_URL"), text(vars, "REMNAWAVE_TOKEN")) {
            (url, token)
                if url.is_some()
                    || token.is_some()
                    || text(vars, "REMNAWAVE_WEBHOOK_SECRET").is_some() =>
            {
                Some(RemnaConnection {
                    url: url.unwrap_or_default(),
                    token: token.unwrap_or_default(),
                    cookie: text(vars, "REMNAWAVE_COOKIE"),
                    api_key: text(vars, "REMNAWAVE_X_API_KEY"),
                    webhook_secret: text(vars, "REMNAWAVE_WEBHOOK_SECRET"),
                })
            }
            _ => None,
        };
        Ok(Config {
            database_url,
            db_max_connections,
            secret_key: text(vars, "SN_SECRET_KEY"),
            log_filter: text(vars, "SN_LOG").unwrap_or_else(|| "info".into()),
            log_format,
            migrate_on_start: flag(vars, "SN_MIGRATE_ON_START", true)?,
            run_worker: flag(vars, "SN_RUN_WORKER", true)?,
            run_bot: flag(vars, "SN_RUN_BOT", true)?,
            bot_concurrency,
            telegram_api,
            server: ServerConfig {
                bind,
                public_url: public_url.clone(),
                trusted_proxies: proxies.networks,
                cors_origins: list(vars, "SN_CORS_ORIGINS"),
                web_dir: text(vars, "SN_WEB_DIR").map(std::path::PathBuf::from),
                csp,
                hsts_max_age,
                ..ServerConfig::default()
            },
            env: EnvFallbacks {
                public_url,
                remna,
                bot_token: text(vars, "TELEGRAM_BOT_TOKEN"),
                telegram_api: custom_api,
                ext_modules_loopback: flag(vars, "SN_EXT_MODULES_LOOPBACK", false)?,
                proxy,
            },
            updates: updates(vars)?,
            backup_dir: text(vars, "SN_BACKUP_DIR").map(std::path::PathBuf::from),
            backup_host_dir: text(vars, "SN_BACKUP_HOST_DIR"),
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn прокси_задаётся_в_окружении_без_утечки_пароля() {
        let base = ("DATABASE_URL", "postgres://sn@db/sn");
        let config = Config::from_vars(&vars(&[
            base,
            (
                "SN_PROXY_URL",
                "socks5h://shop:Pa55word@proxy.example.com:1080",
            ),
            ("SN_PROXY_ROUTES", "telegram, payments"),
            ("SN_PROXY_PAYMENTS", "cryptomus"),
        ]))
        .unwrap();
        let proxy = config.env.proxy.clone().unwrap();
        assert_eq!(proxy.proxy.password(), Some("Pa55word"));
        assert_eq!(
            proxy.routes,
            [sn_egress::Route::Telegram, sn_egress::Route::Payments]
                .into_iter()
                .collect()
        );
        assert!(!format!("{config:?}").contains("Pa55word"));
        assert!(config.warnings.is_empty());
        let err = Config::from_vars(&vars(&[
            base,
            (
                "SN_PROXY_URL",
                "socks5://shop:Pa55word@proxy.example.com:port",
            ),
        ]))
        .unwrap_err();
        assert!(format!("{err:#}").contains("SN_PROXY_URL"));
        assert!(!format!("{err:#}").contains("Pa55word"));
        assert!(
            Config::from_vars(&vars(&[
                base,
                ("SN_PROXY_URL", "socks5://proxy.example.com:1080"),
                ("SN_PROXY_ROUTES", "telegram,remnawave"),
            ]))
            .is_err()
        );
        let warned = Config::from_vars(&vars(&[
            base,
            ("HTTPS_PROXY", "http://proxy.example.com:3128"),
            ("SN_PROXY_ROUTES", "telegram"),
        ]))
        .unwrap();
        assert!(warned.env.proxy.is_none());
        assert_eq!(warned.warnings.len(), 2, "{:?}", warned.warnings);
        assert!(warned.warnings.iter().any(|w| w.contains("HTTPS_PROXY")));
    }

    #[test]
    fn без_базы_не_запускаемся() {
        assert!(Config::from_vars(&vars(&[])).is_err());
    }

    #[test]
    fn значения_по_умолчанию() {
        let config = Config::from_vars(&vars(&[("DATABASE_URL", "postgres://sn@db/sn")])).unwrap();
        assert_eq!(config.server.bind.to_string(), "0.0.0.0:8080");
        assert!(config.migrate_on_start && config.run_worker && config.run_bot);
        assert_eq!(config.bot_concurrency, 64);
        assert_eq!(config.db_max_connections, 16);
        assert_eq!(config.log_format, LogFormat::Text);
        assert!(config.env.remna.is_none());
        assert!(config.secret_key.is_none());
        assert_eq!(config.telegram_api, "https://api.telegram.org");
        assert!(config.backup_dir.is_none());
        assert!(!config.env.ext_modules_loopback);
    }

    #[test]
    fn политика_csp_и_hsts_задаются_переменными() {
        let config = Config::from_vars(&vars(&[("DATABASE_URL", "postgres://sn@db/sn")])).unwrap();
        assert_eq!(config.server.csp, CspMode::Admin);
        assert_eq!(config.server.hsts_max_age, 31_536_000);
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "postgres://sn@db/sn"),
            ("SN_CSP", "all"),
            ("SN_HSTS_MAX_AGE", "0"),
        ]))
        .unwrap();
        assert_eq!(config.server.csp, CspMode::All);
        assert_eq!(config.server.hsts_max_age, 0);
        assert!(Config::from_vars(&vars(&[("DATABASE_URL", "x"), ("SN_CSP", "строго")])).is_err());
        assert!(
            Config::from_vars(&vars(&[("DATABASE_URL", "x"), ("SN_HSTS_MAX_AGE", "год")])).is_err()
        );
    }

    #[test]
    fn модули_на_этом_же_сервере_включаются_переменной() {
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "postgres://sn@db/sn"),
            ("SN_EXT_MODULES_LOOPBACK", "1"),
        ]))
        .unwrap();
        assert!(config.env.ext_modules_loopback);
        assert!(
            Config::from_vars(&vars(&[
                ("DATABASE_URL", "x"),
                ("SN_EXT_MODULES_LOOPBACK", "может быть")
            ]))
            .is_err()
        );
    }

    #[test]
    fn папка_копий_задаётся_переменной() {
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "postgres://sn@db/sn"),
            ("SN_BACKUP_DIR", "/var/lib/stealthnet/backups"),
            ("SN_BACKUP_HOST_DIR", "/opt/stealthnet/deploy/backups/panel"),
        ]))
        .unwrap();
        assert_eq!(
            config.backup_dir.as_deref(),
            Some(std::path::Path::new("/var/lib/stealthnet/backups"))
        );
        assert_eq!(
            config.backup_host_dir.as_deref(),
            Some("/opt/stealthnet/deploy/backups/panel")
        );
    }

    #[test]
    fn адрес_bot_api_можно_заменить() {
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "postgres://sn@db/sn"),
            ("SN_TELEGRAM_API", "http://telegram-bot-api:8081/"),
        ]))
        .unwrap();
        assert_eq!(config.telegram_api, "http://telegram-bot-api:8081");
        assert!(
            Config::from_vars(&vars(&[
                ("DATABASE_URL", "x"),
                ("SN_TELEGRAM_API", "api.telegram.org")
            ]))
            .is_err()
        );
    }

    #[test]
    fn доверенные_прокси_задаются_адресами_и_сетями() {
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "postgres://sn@db/sn"),
            (
                "SN_TRUSTED_PROXIES",
                "10.0.0.2, 172.18.0.0/16,fd00::/8, 2001:db8::7",
            ),
        ]))
        .unwrap();
        assert_eq!(
            config
                .server
                .trusted_proxies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["10.0.0.2", "172.18.0.0/16", "fd00::/8", "2001:db8::7"]
        );
        assert_eq!(
            Config::from_vars(&vars(&[
                ("DATABASE_URL", "x"),
                ("SN_TRUSTED_PROXIES", "10.0.0.2,172.18.0.0/33")
            ]))
            .unwrap_err()
            .to_string(),
            "SN_TRUSTED_PROXIES: «172.18.0.0/33»: длина префикса сети должна быть от 0 до 32"
        );
        assert!(config.warnings.is_empty());
    }

    #[test]
    fn адрес_шлюза_docker_с_маской_принимается_с_предупреждением() {
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "postgres://sn@db/sn"),
            ("SN_TRUSTED_PROXIES", "172.18.0.1/16"),
        ]))
        .unwrap();
        assert_eq!(config.server.trusted_proxies.len(), 1);
        assert_eq!(
            config.server.trusted_proxies[0].to_string(),
            "172.18.0.0/16"
        );
        assert_eq!(config.warnings.len(), 1);
        assert!(
            config.warnings[0].contains("доверяем всей частной сети 172.18.0.0/16"),
            "{}",
            config.warnings[0]
        );
        assert_eq!(
            Config::from_vars(&vars(&[
                ("DATABASE_URL", "x"),
                ("SN_TRUSTED_PROXIES", "203.0.113.7/24")
            ]))
            .unwrap_err()
            .to_string(),
            "SN_TRUSTED_PROXIES: «203.0.113.7/24»: это адрес внутри сети, а не сама сеть: укажите 203.0.113.0/24 или один адрес без длины префикса"
        );
    }

    #[test]
    fn доверие_всем_адресам_запускается_с_предупреждением() {
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "postgres://sn@db/sn"),
            ("SN_TRUSTED_PROXIES", "0.0.0.0/0"),
        ]))
        .unwrap();
        assert_eq!(config.server.trusted_proxies.len(), 1);
        assert_eq!(config.warnings.len(), 1);
        assert!(
            config.warnings[0].contains("0.0.0.0/0 — доверие всем адресам"),
            "{}",
            config.warnings[0]
        );
        let quiet = Config::from_vars(&vars(&[("DATABASE_URL", "postgres://sn@db/sn")])).unwrap();
        assert!(quiet.warnings.is_empty());
    }

    #[test]
    fn переменные_разбираются_строго() {
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "postgres://sn@db/sn"),
            ("SN_PUBLIC_URL", "https://panel.example.com/"),
            ("SN_TRUSTED_PROXIES", "10.0.0.2, 10.0.0.3"),
            ("SN_RUN_BOT", "false"),
            ("REMNAWAVE_URL", "http://remnawave:3000"),
            ("REMNAWAVE_TOKEN", "token"),
            ("REMNAWAVE_COOKIE", "zIdejKtN=MWbLrRzx"),
            ("REMNAWAVE_X_API_KEY", "Basic dXNlcm5hbWU6cGFzc3dvcmQ="),
        ]))
        .unwrap();
        assert_eq!(
            config.server.public_url.as_deref(),
            Some("https://panel.example.com")
        );
        assert_eq!(config.server.trusted_proxies.len(), 2);
        assert!(!config.run_bot);
        let remna = config.env.remna.unwrap();
        assert_eq!(remna.url, "http://remnawave:3000");
        assert_eq!(remna.cookie.as_deref(), Some("zIdejKtN=MWbLrRzx"));
        assert_eq!(
            remna.api_key.as_deref(),
            Some("Basic dXNlcm5hbWU6cGFzc3dvcmQ=")
        );
        assert!(
            Config::from_vars(&vars(&[("DATABASE_URL", "x"), ("SN_RUN_BOT", "maybe")])).is_err()
        );
        assert_eq!(
            Config::from_vars(&vars(&[
                ("DATABASE_URL", "x"),
                ("SN_TRUSTED_PROXIES", "proxy")
            ]))
            .unwrap_err()
            .to_string(),
            "SN_TRUSTED_PROXIES: «proxy»: ожидается IP-адрес или сеть в записи CIDR, например 10.0.0.2, 172.18.0.0/16 или fd00::/8"
        );
        assert!(
            Config::from_vars(&vars(&[
                ("DATABASE_URL", "x"),
                ("SN_PUBLIC_URL", "panel.example.com")
            ]))
            .is_err()
        );
    }

    #[test]
    fn число_обработчиков_бота_берётся_из_окружения() {
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "x"),
            ("SN_DB_MAX_CONNECTIONS", "40"),
        ]))
        .unwrap();
        assert_eq!(config.bot_concurrency, 64, "от пула базы не зависит");
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "x"),
            ("SN_BOT_CONCURRENCY", "24"),
        ]))
        .unwrap();
        assert_eq!(config.bot_concurrency, 24);
        let config = Config::from_vars(&vars(&[
            ("DATABASE_URL", "x"),
            ("SN_BOT_CONCURRENCY", "100000"),
        ]))
        .unwrap();
        assert_eq!(config.bot_concurrency, 256);
        let config =
            Config::from_vars(&vars(&[("DATABASE_URL", "x"), ("SN_BOT_CONCURRENCY", "0")]))
                .unwrap();
        assert_eq!(config.bot_concurrency, 1);
        assert!(
            Config::from_vars(&vars(&[
                ("DATABASE_URL", "x"),
                ("SN_BOT_CONCURRENCY", "много")
            ]))
            .is_err()
        );
    }
}
