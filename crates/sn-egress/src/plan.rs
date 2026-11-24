use crate::address::{Proxy, ProxyError, ProxyServer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Telegram,
    Payments,
    Ai,
    Mail,
    Updates,
}

impl Route {
    pub const ALL: [Route; 5] = [
        Route::Telegram,
        Route::Payments,
        Route::Ai,
        Route::Mail,
        Route::Updates,
    ];

    pub const ENV_DEFAULT: [Route; 2] = [Route::Telegram, Route::Ai];

    pub const fn as_str(self) -> &'static str {
        match self {
            Route::Telegram => "telegram",
            Route::Payments => "payments",
            Route::Ai => "ai",
            Route::Mail => "mail",
            Route::Updates => "updates",
        }
    }

    pub fn parse(raw: &str) -> Option<Route> {
        let raw = raw.trim();
        Route::ALL
            .into_iter()
            .find(|route| route.as_str().eq_ignore_ascii_case(raw))
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            Route::Telegram => "Telegram",
            Route::Payments => "Платёжки",
            Route::Ai => "ИИ-чат поддержки",
            Route::Mail => "Почта через Resend",
            Route::Updates => "Проверка обновлений",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProxySettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default)]
    pub routes: BTreeSet<Route>,
    #[serde(default)]
    pub providers: BTreeSet<String>,
}

impl ProxySettings {
    pub fn server(&self) -> Option<Result<ProxyServer, ProxyError>> {
        self.server
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ProxyServer::parse)
    }

    pub fn proxy(&self, password: Option<&str>) -> Option<Result<Proxy, ProxyError>> {
        self.server().map(|server| {
            server.and_then(|server| Proxy::new(server, self.username.as_deref(), password))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvProxy {
    pub proxy: Proxy,
    pub routes: BTreeSet<Route>,
    pub providers: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvError {
    Url(ProxyError),
    Route(String),
    Provider(String),
}

impl std::fmt::Display for EnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvError::Url(err) => write!(f, "SN_PROXY_URL: {err}"),
            EnvError::Route(name) => write!(
                f,
                "SN_PROXY_ROUTES: неизвестное направление «{name}», подойдут telegram, payments, ai, mail, updates"
            ),
            EnvError::Provider(name) => write!(
                f,
                "SN_PROXY_PAYMENTS: «{name}» — не код платёжки (латинские буквы, цифры и _)"
            ),
        }
    }
}

impl std::error::Error for EnvError {}

fn items(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split([',', ' ', ';'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

impl EnvProxy {
    pub fn from_env(
        url: Option<&str>,
        routes: Option<&str>,
        providers: Option<&str>,
    ) -> Result<Option<EnvProxy>, EnvError> {
        let Some(url) = url.map(str::trim).filter(|u| !u.is_empty()) else {
            return Ok(None);
        };
        let proxy = Proxy::parse(url).map_err(EnvError::Url)?;
        let listed = items(routes);
        let routes: BTreeSet<Route> = if listed.is_empty() {
            Route::ENV_DEFAULT.into_iter().collect()
        } else {
            listed
                .iter()
                .map(|name| Route::parse(name).ok_or_else(|| EnvError::Route(name.clone())))
                .collect::<Result<_, _>>()?
        };
        let providers = items(providers)
            .into_iter()
            .map(|id| {
                let id = id.to_ascii_lowercase();
                if id.len() <= 40 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                    Ok(id)
                } else {
                    Err(EnvError::Provider(id.chars().take(40).collect()))
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(Some(EnvProxy {
            proxy,
            routes,
            providers,
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    None,
    Panel,
    Env,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub proxy: Option<Arc<Proxy>>,
    pub routes: BTreeSet<Route>,
    pub providers: BTreeSet<String>,
    pub source: Source,
}

impl Plan {
    pub fn direct() -> Self {
        Plan::default()
    }

    pub fn from_env(env: &EnvProxy) -> Self {
        Plan {
            proxy: Some(Arc::new(env.proxy.clone())),
            routes: env.routes.clone(),
            providers: env.providers.clone(),
            source: Source::Env,
        }
    }

    pub fn from_panel(settings: &ProxySettings, password: Option<&str>) -> Self {
        let proxy = if settings.enabled {
            match settings.proxy(password) {
                Some(Ok(proxy)) => Some(Arc::new(proxy)),
                Some(Err(err)) => {
                    tracing::warn!(error = %err, "адрес прокси в настройках не разобран, прокси выключен");
                    None
                }
                None => None,
            }
        } else {
            None
        };
        Plan {
            proxy,
            routes: settings.routes.clone(),
            providers: settings.providers.clone(),
            source: Source::Panel,
        }
    }

    pub fn resolve(panel: Option<(&ProxySettings, Option<&str>)>, env: Option<&EnvProxy>) -> Self {
        match (panel, env) {
            (Some((settings, password)), _) => Plan::from_panel(settings, password),
            (None, Some(env)) => Plan::from_env(env),
            (None, None) => Plan::direct(),
        }
    }

    pub fn proxy_for(&self, route: Route, provider: Option<&str>) -> Option<Arc<Proxy>> {
        let proxy = self.proxy.as_ref()?;
        if !self.routes.contains(&route) {
            return None;
        }
        if route == Route::Payments && !self.providers.is_empty() {
            let id = provider?;
            if !self.providers.contains(id) {
                return None;
            }
        }
        Some(proxy.clone())
    }

    pub fn active(&self) -> bool {
        self.proxy.is_some() && !self.routes.is_empty()
    }

    pub fn uses(&self, route: Route) -> bool {
        self.proxy.is_some() && self.routes.contains(&route)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(routes: &[Route], providers: &[&str]) -> ProxySettings {
        ProxySettings {
            enabled: true,
            server: Some("socks5://203.0.113.5:1080".into()),
            username: Some("user".into()),
            routes: routes.iter().copied().collect(),
            providers: providers.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn направления_и_выбранные_платёжки() {
        let plan = Plan::from_panel(
            &settings(&[Route::Telegram, Route::Payments], &["cryptomus"]),
            Some("secret"),
        );
        let telegram = plan.proxy_for(Route::Telegram, None).unwrap();
        assert_eq!(telegram.password(), Some("secret"));
        assert!(plan.proxy_for(Route::Ai, None).is_none());
        assert!(plan.proxy_for(Route::Payments, Some("cryptomus")).is_some());
        assert!(plan.proxy_for(Route::Payments, Some("yookassa")).is_none());
        assert!(plan.proxy_for(Route::Payments, None).is_none());
        let all = Plan::from_panel(&settings(&[Route::Payments], &[]), None);
        assert!(all.proxy_for(Route::Payments, Some("yookassa")).is_some());
        let off = Plan::from_panel(
            &ProxySettings {
                enabled: false,
                ..settings(&Route::ALL, &[])
            },
            None,
        );
        assert!(!off.active());
        assert_eq!(off.source, Source::Panel);
        assert!(
            Route::ALL
                .iter()
                .all(|r| off.proxy_for(*r, Some("x")).is_none())
        );
    }

    #[test]
    fn настройки_панели_важнее_окружения() {
        let env = EnvProxy::from_env(Some("http://env.example.com:3128"), None, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            env.routes,
            [Route::Telegram, Route::Ai].into_iter().collect()
        );
        let from_env = Plan::resolve(None, Some(&env));
        assert_eq!(from_env.source, Source::Env);
        assert!(from_env.proxy_for(Route::Telegram, None).is_some());
        assert!(
            from_env
                .proxy_for(Route::Payments, Some("cryptomus"))
                .is_none()
        );
        let panel = settings(&[Route::Mail], &[]);
        let from_panel = Plan::resolve(Some((&panel, None)), Some(&env));
        assert_eq!(from_panel.source, Source::Panel);
        assert!(from_panel.proxy_for(Route::Telegram, None).is_none());
        assert!(from_panel.proxy_for(Route::Mail, None).is_some());
        assert_eq!(Plan::resolve(None, None), Plan::direct());
    }

    #[test]
    fn переменные_окружения_проверяются() {
        let env = EnvProxy::from_env(
            Some("socks5h://u:p%3Ass@proxy.example.com:1080"),
            Some("telegram, payments ,AI"),
            Some("cryptomus,Heleket"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(env.proxy.password(), Some("p:ss"));
        assert_eq!(
            env.routes,
            [Route::Telegram, Route::Payments, Route::Ai]
                .into_iter()
                .collect()
        );
        assert_eq!(
            env.providers,
            ["cryptomus".to_string(), "heleket".to_string()]
                .into_iter()
                .collect()
        );
        assert!(
            EnvProxy::from_env(None, Some("telegram"), None)
                .unwrap()
                .is_none()
        );
        let bad = EnvProxy::from_env(Some("socks5://u:TopSecret@proxy.example.com:x"), None, None)
            .unwrap_err();
        assert!(!bad.to_string().contains("TopSecret"));
        assert!(matches!(
            EnvProxy::from_env(
                Some("socks5://proxy.example.com:1080"),
                Some("telegram,smtp"),
                None
            ),
            Err(EnvError::Route(_))
        ));
        assert!(matches!(
            EnvProxy::from_env(
                Some("socks5://proxy.example.com:1080"),
                None,
                Some("bad-id")
            ),
            Err(EnvError::Provider(_))
        ));
    }

    #[test]
    fn настройки_хранятся_без_пароля() {
        let stored = serde_json::to_value(settings(&[Route::Telegram], &["cryptomus"])).unwrap();
        assert_eq!(
            stored,
            serde_json::json!({
                "enabled": true,
                "server": "socks5://203.0.113.5:1080",
                "username": "user",
                "routes": ["telegram"],
                "providers": ["cryptomus"],
            })
        );
        let back: ProxySettings = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(back, ProxySettings::default());
    }
}
