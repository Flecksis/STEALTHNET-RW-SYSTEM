use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use crate::settings::{read_secret, write_secret};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::format;
use sn_domain::ids::AdminId;
use sn_domain::module::ModuleId;
use sn_domain::payment::providers;
use sn_domain::settings::{keys, secret_keys};
use sn_egress::{
    EnvProxy, Failure, Plan, Problem, Proxy, ProxyServer, ProxySettings, Route, Source,
};
use sn_store::{Db, SecretBox};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

pub const SETTINGS_ACTION: &str = "settings.proxy";
pub const RESET_ACTION: &str = "settings.proxy_reset";
pub const DOWN_ACTION: &str = "settings.proxy_down";
pub const RESTORED_ACTION: &str = "settings.proxy_restored";
pub const HEALTH_KEY: &str = "egress.health";
pub const PAGE_PATH: &str = "/admin/settings/general#proxy";
pub const WATCH_EVERY: Duration = Duration::from_secs(60);
pub const DOWN_AFTER: u32 = 2;
pub const WAIT: chrono::Duration = chrono::Duration::seconds(60);
const TELEGRAM_API: &str = "https://api.telegram.org";
const CHECK_TARGETS_MAX: usize = 16;
const NO_PROXY_PROVIDERS: [&str; 4] = [
    providers::BALANCE,
    providers::MANUAL,
    providers::STARS,
    providers::YOOMONEY,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    #[default]
    Unknown,
    Up,
    Down,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    #[serde(default)]
    pub state: HealthState,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub problem: Option<String>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub checked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub millis: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvView {
    pub server: String,
    pub username: Option<String>,
    pub password_set: bool,
    pub routes: BTreeSet<Route>,
    pub providers: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderOption {
    pub id: String,
    pub title: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProxyView {
    pub source: Source,
    pub enabled: bool,
    pub server: Option<String>,
    pub username: Option<String>,
    pub password_set: bool,
    pub routes: BTreeSet<Route>,
    pub providers: BTreeSet<String>,
    pub env: Option<EnvView>,
    pub secret_key: bool,
    pub payment_options: Vec<ProviderOption>,
    pub telegram_api: String,
    pub health: Health,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ProxyUpdate {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub clear_password: bool,
    #[serde(default)]
    pub routes: BTreeSet<Route>,
    #[serde(default)]
    pub providers: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ProxyCheckRequest {
    #[serde(default)]
    pub current: bool,
    #[serde(flatten)]
    pub form: ProxyUpdate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckItem {
    pub route: Route,
    pub title: String,
    pub host: String,
    pub ok: bool,
    pub direct: bool,
    pub status: Option<u16>,
    pub millis: Option<u64>,
    pub problem: Option<Problem>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckView {
    pub ok: bool,
    pub proxy: String,
    pub items: Vec<CheckItem>,
}

struct Target {
    route: Route,
    title: String,
    url: Option<reqwest::Url>,
    note: Option<String>,
}

fn field(name: &str, reason: impl std::fmt::Display) -> ServiceError {
    ServiceError::rejected("settings.invalid_field", format!("поле «{name}»: {reason}"))
}

pub(crate) async fn stored(db: &mut impl Db) -> ServiceResult<Option<ProxySettings>> {
    let Some(value) = db.settings().get(keys::PROXY).await? else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    match serde_json::from_value::<ProxySettings>(value) {
        Ok(settings) => Ok(Some(settings)),
        Err(err) => {
            tracing::warn!(error = %err, "настройки прокси не разобраны, прокси выключен");
            Ok(Some(ProxySettings::default()))
        }
    }
}

pub(crate) async fn load_plan(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    env: Option<&EnvProxy>,
) -> ServiceResult<Plan> {
    let panel = stored(db).await?;
    let password = match &panel {
        Some(settings) if settings.enabled => {
            read_secret(db, secrets, secret_keys::PROXY_PASSWORD).await?
        }
        _ => None,
    };
    Ok(Plan::resolve(
        panel.as_ref().map(|s| (s, password.as_deref())),
        env,
    ))
}

fn same_server(a: Option<&str>, b: Option<&str>) -> bool {
    let parse = |raw: Option<&str>| raw.and_then(|r| ProxyServer::parse(r).ok());
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

pub fn routes_text(routes: &BTreeSet<Route>) -> String {
    routes
        .iter()
        .map(|r| r.title_ru())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn alert_text(action: &str, details: &serde_json::Value) -> String {
    let text = |key: &str| {
        details
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(crate::alerts::escape)
    };
    let proxy = text("proxy").unwrap_or_else(|| "прокси".to_string());
    let routes = text("routes");
    if action == RESTORED_ACTION {
        let target = text("target").unwrap_or_else(|| "сервис".to_string());
        let mut lines = vec![
            "<b>Прокси снова работает</b>".to_string(),
            format!("{target} отвечает через {proxy}"),
        ];
        if let Some(minutes) = details.get("minutes").and_then(serde_json::Value::as_i64) {
            lines.push(format!(
                "Не работал {minutes}\u{a0}{}",
                format::plural_ru(minutes.unsigned_abs(), "минуту", "минуты", "минут")
            ));
        }
        return lines.join("\n");
    }
    let mut lines = vec![
        "<b>Прокси не отвечает</b>".to_string(),
        text("problem").unwrap_or_else(|| format!("{proxy} не отвечает")),
    ];
    if let Some(routes) = routes {
        lines.push(format!("Через него идут: {routes}"));
    }
    lines.push(
        "Пока прокси не работает, эти запросы не уходят напрямую. Уведомления клиентам и проверки оплат ждут и отправятся, когда прокси оживёт. Входящие уведомления платёжек и вебхук бота работают как обычно"
            .to_string(),
    );
    lines.push(format!("Прокси: {proxy}"));
    lines.join("\n")
}

pub struct EgressOps<'a> {
    services: &'a Services,
}

impl<'a> EgressOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        EgressOps { services }
    }

    fn telegram_api(&self) -> String {
        self.services
            .env()
            .telegram_api
            .clone()
            .unwrap_or_else(|| TELEGRAM_API.to_string())
    }

    fn payment_options(&self) -> Vec<ProviderOption> {
        self.services
            .payments()
            .views()
            .into_iter()
            .filter(|view| !NO_PROXY_PROVIDERS.contains(&view.id))
            .map(|view| ProviderOption {
                id: view.id.to_string(),
                title: view.title.to_string(),
                enabled: view.enabled,
            })
            .collect()
    }

    pub async fn health(&self) -> ServiceResult<Health> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .bot_state()
            .get(HEALTH_KEY)
            .await?
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default())
    }

    async fn put_health(&self, health: &Health) -> ServiceResult<()> {
        let value =
            serde_json::to_value(health).map_err(|e| ServiceError::Internal(e.to_string()))?;
        let mut conn = self.services.store().conn().await?;
        conn.bot_state().set(HEALTH_KEY, &value).await?;
        Ok(())
    }

    pub async fn view(&self) -> ServiceResult<ProxyView> {
        let mut conn = self.services.store().conn().await?;
        let panel = stored(&mut conn).await?;
        let password_set = conn
            .secrets()
            .get(secret_keys::PROXY_PASSWORD)
            .await?
            .is_some();
        drop(conn);
        let env = self.services.env().proxy.as_ref().map(|env| EnvView {
            server: env.proxy.server().to_string(),
            username: env.proxy.username().map(str::to_string),
            password_set: env.proxy.has_password(),
            routes: env.routes.clone(),
            providers: env.providers.clone(),
        });
        let plan = self.services.egress().plan();
        let mut health = self.health().await?;
        if health.proxy.as_deref()
            != plan
                .proxy
                .as_ref()
                .map(|p| p.server().to_string())
                .as_deref()
        {
            health = Health::default();
        }
        let base = ProxyView {
            source: plan.source,
            enabled: false,
            server: None,
            username: None,
            password_set: false,
            routes: BTreeSet::new(),
            providers: BTreeSet::new(),
            env: env.clone(),
            secret_key: self.services.secrets().is_some(),
            payment_options: self.payment_options(),
            telegram_api: self.telegram_api(),
            health,
        };
        Ok(match (panel, env) {
            (Some(settings), _) => ProxyView {
                enabled: settings.enabled,
                server: settings
                    .server()
                    .and_then(Result::ok)
                    .map(|s| s.to_string())
                    .or(settings.server.clone()),
                username: settings.username.clone(),
                password_set,
                routes: settings.routes.clone(),
                providers: settings.providers.clone(),
                ..base
            },
            (None, Some(env)) => ProxyView {
                enabled: true,
                server: Some(env.server.clone()),
                username: env.username.clone(),
                password_set: env.password_set,
                routes: env.routes.clone(),
                providers: env.providers.clone(),
                ..base
            },
            (None, None) => base,
        })
    }

    fn clean(&self, update: &ProxyUpdate) -> ServiceResult<ProxySettings> {
        let server = match update
            .server
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => Some(ProxyServer::parse(raw).map_err(|e| field("server", e))?),
            None => None,
        };
        let username = update
            .username
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        if let Some(server) = &server {
            Proxy::new(server.clone(), username.as_deref(), None)
                .map_err(|e| field("username", e))?;
        } else if update.enabled {
            return Err(field("server", "укажите адрес прокси"));
        }
        if update.enabled && update.routes.is_empty() {
            return Err(field(
                "routes",
                "выберите, что пойдёт через прокси: Telegram, платёжки, ИИ, почта или обновления",
            ));
        }
        let known: BTreeSet<String> = self
            .payment_options()
            .into_iter()
            .map(|option| option.id)
            .collect();
        let providers: BTreeSet<String> = if update.routes.contains(&Route::Payments) {
            update.providers.clone()
        } else {
            BTreeSet::new()
        };
        if let Some(unknown) = providers.iter().find(|id| !known.contains(*id)) {
            return Err(field(
                "providers",
                format!(
                    "платёжки «{}» нет или через прокси она не ходит",
                    unknown.chars().take(40).collect::<String>()
                ),
            ));
        }
        if let Some(password) = update.password.as_deref().filter(|p| !p.is_empty())
            && let Some(server) = &server
        {
            if username.is_none() {
                return Err(field("password", "для пароля прокси нужен и логин"));
            }
            Proxy::new(server.clone(), username.as_deref(), Some(password))
                .map_err(|e| field("password", e))?;
        }
        Ok(ProxySettings {
            enabled: update.enabled,
            server: server.map(|s| s.to_string()),
            username,
            routes: update.routes.clone(),
            providers,
        })
    }

    pub async fn update(&self, update: &ProxyUpdate, admin: AdminId) -> ServiceResult<ProxyView> {
        let next = self.clean(update)?;
        let mut tx = self.services.store().begin().await?;
        let before = stored(&mut tx).await?;
        let had_password = tx
            .secrets()
            .get(secret_keys::PROXY_PASSWORD)
            .await?
            .is_some();
        let given = update.password.as_deref().filter(|p| !p.is_empty());
        let moved = !same_server(
            before.as_ref().and_then(|b| b.server.as_deref()),
            next.server.as_deref(),
        );
        let password = if update.clear_password || next.username.is_none() {
            if had_password {
                write_secret(
                    &mut tx,
                    self.services.secrets(),
                    secret_keys::PROXY_PASSWORD,
                    None,
                    Some(admin),
                )
                .await?;
                "cleared"
            } else {
                "none"
            }
        } else if let Some(given) = given {
            write_secret(
                &mut tx,
                self.services.secrets(),
                secret_keys::PROXY_PASSWORD,
                Some(given),
                Some(admin),
            )
            .await?;
            "set"
        } else if had_password && moved {
            write_secret(
                &mut tx,
                self.services.secrets(),
                secret_keys::PROXY_PASSWORD,
                None,
                Some(admin),
            )
            .await?;
            "dropped"
        } else if had_password {
            "kept"
        } else {
            "none"
        };
        tx.settings()
            .set_as(keys::PROXY, &next, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                SETTINGS_ACTION,
                "settings",
                None,
                serde_json::json!({
                    "enabled": next.enabled,
                    "server": next.server,
                    "username": next.username.is_some(),
                    "password": password,
                    "routes": next.routes,
                    "providers": next.providers,
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        self.view().await
    }

    pub async fn reset(&self, admin: AdminId) -> ServiceResult<ProxyView> {
        let mut tx = self.services.store().begin().await?;
        tx.settings().delete(keys::PROXY).await?;
        write_secret(
            &mut tx,
            self.services.secrets(),
            secret_keys::PROXY_PASSWORD,
            None,
            Some(admin),
        )
        .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                RESET_ACTION,
                "settings",
                None,
                serde_json::json!({ "env": self.services.env().proxy.is_some() }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        self.view().await
    }

    async fn saved(&self) -> ServiceResult<()> {
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    async fn candidate(&self, request: &ProxyCheckRequest) -> ServiceResult<Plan> {
        if request.current {
            let plan = self.services.egress().plan();
            if !plan.active() {
                return Err(ServiceError::rejected(
                    "settings.proxy_off",
                    "Прокси не задан или выключен: проверять нечего",
                ));
            }
            return Ok(plan.as_ref().clone());
        }
        let settings = self.clean(&request.form)?;
        let Some(server) = settings.server().and_then(Result::ok) else {
            return Err(field("server", "укажите адрес прокси"));
        };
        if settings.routes.is_empty() {
            return Err(field(
                "routes",
                "выберите, что пойдёт через прокси: Telegram, платёжки, ИИ, почта или обновления",
            ));
        }
        let given = request.form.password.as_deref().filter(|p| !p.is_empty());
        let password = match given {
            Some(given) => Some(given.to_string()),
            None if request.form.clear_password || settings.username.is_none() => None,
            None => {
                let mut conn = self.services.store().conn().await?;
                let before = stored(&mut conn).await?;
                if same_server(
                    before.as_ref().and_then(|b| b.server.as_deref()),
                    settings.server.as_deref(),
                ) {
                    read_secret(
                        &mut conn,
                        self.services.secrets(),
                        secret_keys::PROXY_PASSWORD,
                    )
                    .await?
                } else {
                    None
                }
            }
        };
        let proxy = Proxy::new(server, settings.username.as_deref(), password.as_deref())
            .map_err(|e| field("password", e))?;
        Ok(Plan {
            proxy: Some(Arc::new(proxy)),
            routes: settings.routes.clone(),
            providers: settings.providers.clone(),
            source: Source::Panel,
        })
    }

    fn targets(&self, plan: &Plan) -> Vec<Target> {
        let mut out = Vec::new();
        let parse = |raw: &str| reqwest::Url::parse(raw.trim()).ok();
        for route in &plan.routes {
            match route {
                Route::Telegram => out.push(Target {
                    route: *route,
                    title: "Telegram".into(),
                    url: parse(&format!("{}/", self.telegram_api().trim_end_matches('/'))),
                    note: None,
                }),
                Route::Payments => {
                    let registry = self.services.payments();
                    let snapshot = registry.snapshot();
                    let mut found = false;
                    for option in self.payment_options() {
                        let chosen = if plan.providers.is_empty() {
                            option.enabled
                        } else {
                            plan.providers.contains(&option.id)
                        };
                        if !chosen {
                            continue;
                        }
                        let Some(provider) = registry.get(&option.id) else {
                            continue;
                        };
                        let config = snapshot
                            .states
                            .get(&option.id)
                            .map(|state| state.config.clone())
                            .unwrap_or_default();
                        found = true;
                        out.push(Target {
                            route: *route,
                            title: option.title.clone(),
                            url: provider.api_url(&config).as_deref().and_then(parse),
                            note: None,
                        });
                    }
                    if !found {
                        out.push(Target {
                            route: *route,
                            title: "Платёжки".into(),
                            url: None,
                            note: Some(
                                "Нет включённых платёжек: проверять нечего, прокси подключится, когда вы их включите".into(),
                            ),
                        });
                    }
                }
                Route::Ai => {
                    let config = self.services.modules().config(ModuleId::Ai);
                    let settings = sn_modules::catalog::ai::Settings::from_config(&config);
                    out.push(Target {
                        route: *route,
                        title: "ИИ-чат поддержки".into(),
                        url: Some(settings.api_url)
                            .filter(|u| !u.is_empty())
                            .as_deref()
                            .and_then(parse),
                        note: Some("В «ИИ-чате поддержки» не задан адрес API".into()),
                    });
                }
                Route::Mail => {
                    let resend = self.services.mail().via_resend();
                    out.push(Target {
                        route: *route,
                        title: "Почта через Resend".into(),
                        url: if resend {
                            parse(crate::mail::RESEND_API)
                        } else {
                            None
                        },
                        note: Some(
                            "Почта уходит не через Resend: SMTP прокси не использует, письма идут напрямую".into(),
                        ),
                    });
                }
                Route::Updates => {
                    let registry = self
                        .services
                        .update_config()
                        .map(|c| c.registry.clone())
                        .unwrap_or_else(|| crate::updates::DEFAULT_REGISTRY.to_string());
                    out.push(Target {
                        route: *route,
                        title: "Проверка обновлений".into(),
                        url: parse(&format!("{}/v2/", registry.trim_end_matches('/'))),
                        note: None,
                    });
                }
            }
        }
        out.truncate(CHECK_TARGETS_MAX);
        out
    }

    pub async fn check(&self, request: &ProxyCheckRequest) -> ServiceResult<CheckView> {
        let plan = self.candidate(request).await?;
        let Some(proxy) = plan.proxy.clone() else {
            return Err(field("server", "укажите адрес прокси"));
        };
        let targets = self.targets(&plan);
        let mut items = Vec::new();
        let mut checks = Vec::new();
        for target in targets {
            let host = target
                .url
                .as_ref()
                .and_then(|u| u.host_str())
                .unwrap_or_default()
                .to_string();
            match target.url {
                None => items.push((
                    items.len() + checks.len(),
                    CheckItem {
                        route: target.route,
                        title: target.title,
                        host,
                        ok: true,
                        direct: true,
                        status: None,
                        millis: None,
                        problem: None,
                        text: target.note.unwrap_or_default(),
                    },
                )),
                Some(url) if !sn_egress::outward(&url) => items.push((
                    items.len() + checks.len(),
                    CheckItem {
                        route: target.route,
                        title: target.title,
                        text: format!(
                            "Адрес {host} локальный или без https: такие запросы идут напрямую, мимо прокси"
                        ),
                        host,
                        ok: true,
                        direct: true,
                        status: None,
                        millis: None,
                        problem: None,
                    },
                )),
                Some(url) => {
                    let order = items.len() + checks.len();
                    let proxy = proxy.clone();
                    checks.push(async move {
                        let result = sn_egress::probe(&proxy, &url).await;
                        (order, target.route, target.title, host, result)
                    });
                }
            }
        }
        for (order, route, title, host, result) in futures_util::future::join_all(checks).await {
            let item = match result {
                Ok(reached) => CheckItem {
                    route,
                    title,
                    text: format!("{host} отвечает через прокси за {} мс", reached.millis),
                    host,
                    ok: true,
                    direct: false,
                    status: Some(reached.status),
                    millis: Some(reached.millis),
                    problem: None,
                },
                Err(Failure { problem, text }) => CheckItem {
                    route,
                    title,
                    host,
                    ok: false,
                    direct: false,
                    status: None,
                    millis: None,
                    problem: Some(problem),
                    text,
                },
            };
            items.push((order, item));
        }
        items.sort_by_key(|(order, _)| *order);
        let items: Vec<CheckItem> = items.into_iter().map(|(_, item)| item).collect();
        Ok(CheckView {
            ok: items.iter().all(|item| item.ok),
            proxy: proxy.server().to_string(),
            items,
        })
    }

    fn watch_target(&self, plan: &Plan) -> Option<reqwest::Url> {
        let targets = self.targets(plan);
        [
            Route::Telegram,
            Route::Ai,
            Route::Mail,
            Route::Updates,
            Route::Payments,
        ]
        .into_iter()
        .find_map(|route| {
            targets
                .iter()
                .filter(|t| t.route == route)
                .find_map(|t| t.url.clone().filter(sn_egress::outward))
        })
    }

    pub async fn watch(&self) -> ServiceResult<Health> {
        let egress = self.services.egress();
        let plan = egress.plan();
        let now = self.services.now();
        let Some(proxy) = plan.proxy.clone().filter(|_| plan.active()) else {
            egress.set_down(false);
            return Ok(Health::default());
        };
        let Some(target) = self.watch_target(&plan) else {
            egress.set_down(false);
            return Ok(Health::default());
        };
        let label = proxy.server().to_string();
        let mut health = self.health().await?;
        if health.proxy.as_deref() != Some(label.as_str()) {
            health = Health {
                proxy: Some(label.clone()),
                ..Health::default()
            };
        }
        let host = target.host_str().unwrap_or_default().to_string();
        health.checked_at = Some(now);
        health.target = Some(host.clone());
        match sn_egress::probe(&proxy, &target).await {
            Ok(reached) => {
                if health.state == HealthState::Down {
                    let minutes = health
                        .since
                        .map(|since| (now - since).num_minutes().max(1))
                        .unwrap_or(1);
                    self.record(Actor::System.audit(
                        RESTORED_ACTION,
                        "settings",
                        None,
                        serde_json::json!({
                            "proxy": label,
                            "target": host,
                            "minutes": minutes,
                            "routes": routes_text(&plan.routes),
                        }),
                    ))
                    .await?;
                }
                if health.state != HealthState::Up {
                    health.since = Some(now);
                }
                health.state = HealthState::Up;
                health.failures = 0;
                health.problem = None;
                health.millis = Some(reached.millis);
                egress.set_down(false);
            }
            Err(failure) => {
                health.failures = health.failures.saturating_add(1);
                health.problem = Some(failure.text.clone());
                health.millis = None;
                if health.failures >= DOWN_AFTER && health.state != HealthState::Down {
                    health.state = HealthState::Down;
                    health.since = Some(now);
                    self.record(Actor::System.audit(
                        DOWN_ACTION,
                        "settings",
                        None,
                        serde_json::json!({
                            "proxy": label,
                            "target": host,
                            "problem": failure.text,
                            "kind": failure.problem,
                            "routes": routes_text(&plan.routes),
                        }),
                    ))
                    .await?;
                }
                egress.set_down(health.state == HealthState::Down);
            }
        }
        self.put_health(&health).await?;
        Ok(health)
    }

    async fn record(&self, entry: sn_store::AuditEntry) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.audit().record(&entry).await?;
        Ok(())
    }
}
