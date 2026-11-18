use crate::auth::{TOTP_LOCKED_AUDIT, token_hash};
use crate::error::{ServiceError, ServiceResult};
use crate::inbox::{BLOCKED_REFUND_AUDIT, BLOCKED_REFUND_REQUESTED_AUDIT};
use crate::node_watch::NodeNotice;
use crate::services::Services;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sn_domain::alerts::{
    AlertKind, AlertRoute, AlertsConfig, SETTING_KEY, START_PREFIX, TopicMap, clean_topic,
};
use sn_domain::client::IdentityKind;
use sn_domain::format;
use sn_domain::ids::{AdminId, ClientId};
use sn_domain::money::{Amount, Currency, Money};
use sn_domain::settings::keys;
use sn_store::Db;
use std::collections::{BTreeMap, BTreeSet};

mod digest;

pub use digest::BATCH_LINES;

pub const LINK_TTL_MINUTES: i64 = 30;
pub const PER_KIND: usize = 3;
const LINK_KEY: &str = "alerts.link";
const TEST_KEY: &str = "alerts.test";
const TOPICS_KEY: &str = "alerts.topics";
const SUPPORT_CURSOR: &str = "alerts.cursor.support";
const AI_HANDOFF_CURSOR: &str = "alerts.cursor.ai_handoffs";
const AUTO_REFUND_CURSOR: &str = "alerts.cursor.auto_refunds";
const PAYMENTS_CURSOR: &str = "alerts.cursor.payments";
const TRAFFIC_CURSOR: &str = "alerts.cursor.traffic";
const TRAFFIC_SETTLE_SECONDS: i64 = 60;
const BODY_MAX: usize = 300;
const NOTICE_MAX: usize = 600;
const SCAN_LIMIT: i64 = 500;
const NODES_PATH: &str = "/admin/infra";
const ADMINS_PATH: &str = "/admin/settings/admins";
const ORDERS_PATH: &str = "/admin/money#todo-orders";
const CHARGES_PATH: &str = "/admin/money#todo-charges";
const STARS_PATH: &str = "/admin/money#todo-stars";
const REFUNDS_PATH: &str = "/admin/money#todo-refunds";
const CHARGES_SEEN: &str = "alerts.seen.orders.charges";
const SECURITY_DAYS: i32 = 2;

fn seen_key(kind: AlertKind) -> String {
    format!("alerts.seen.{}", kind.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alert {
    pub kind: AlertKind,
    pub html: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AlertLink {
    pub link: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KindView {
    pub kind: AlertKind,
    pub title: &'static str,
    pub default_topic: &'static str,
    pub on: bool,
    pub topic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AlertsView {
    pub chat_id: Option<i64>,
    pub title: Option<String>,
    pub forum: bool,
    pub bot_username: Option<String>,
    pub kinds: Vec<KindView>,
    pub test_pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RouteInput {
    pub kind: AlertKind,
    pub on: bool,
    #[serde(default)]
    pub topic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AlertsUpdate {
    pub routes: Vec<RouteInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingLink {
    hash: String,
    expires_at: DateTime<Utc>,
}

struct Item {
    id: i64,
    html: String,
    path: String,
}

#[derive(sqlx::FromRow)]
struct OrderRow {
    id: i64,
    client_id: i64,
    total_minor: i64,
    currency: String,
    stars_total: Option<i64>,
    review_reason: Option<String>,
}

#[derive(sqlx::FromRow)]
struct TrafficRow {
    client_id: i64,
    period: String,
    used_bytes: i64,
    limit_bytes: i64,
    restriction: String,
    restricted_at: Option<DateTime<Utc>>,
    window_end: DateTime<Utc>,
    restrict_note: Option<String>,
    warned_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct HandoffRow {
    ticket_id: i64,
    client_id: i64,
    reason: Option<String>,
    error: Option<String>,
    subject: Option<String>,
    body: Option<String>,
}

#[derive(sqlx::FromRow)]
struct LockRow {
    id: i64,
    admin_id: Option<i64>,
    email: Option<String>,
    details: serde_json::Value,
    ip: Option<String>,
}

#[derive(sqlx::FromRow)]
struct RefundRow {
    id: i64,
    amount_minor: i64,
    currency: String,
    action_error: Option<String>,
    client_id: Option<i64>,
}

pub struct AlertsOps<'a> {
    services: &'a Services,
}

fn escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn cut(raw: &str, max: usize) -> String {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max).collect();
    out.push('…');
    out
}

fn money(minor: i64, currency: &str) -> Option<String> {
    Currency::parse(currency)
        .ok()
        .map(|currency| format::money(Money::new(minor, currency)))
}

fn amount_line(amount: &serde_json::Value) -> Option<String> {
    let minor = amount.get("minor")?.as_i64()?;
    let currency = amount.get("currency")?.as_str()?;
    if currency == "XTR" {
        return Some(format!(
            "{minor} {}",
            format::plural_ru(minor.unsigned_abs(), "звезда", "звезды", "звёзд")
        ));
    }
    money(minor, currency)
}

fn amount_text(amount: Amount) -> String {
    match amount.as_money() {
        Some(value) => format::money(value),
        None => format!(
            "{} {}",
            amount.minor,
            format::plural_ru(amount.minor.unsigned_abs(), "звезда", "звезды", "звёзд")
        ),
    }
}

fn payout_method(method: &str, title: Option<&str>) -> String {
    sn_modules::catalog::referral::payout_title(method, title)
}

fn summary(kind: AlertKind, rest: usize) -> Alert {
    let n = rest as u64;
    let (text, path) = match kind {
        AlertKind::Support => (
            format!(
                "Ещё {rest} {} с новыми сообщениями",
                format::plural_ru(n, "обращение", "обращения", "обращений")
            ),
            "/admin/support",
        ),
        AlertKind::Orders => (
            format!(
                "Ещё {rest} {} ждут ручной проверки",
                format::plural_ru(n, "заказ", "заказа", "заказов")
            ),
            ORDERS_PATH,
        ),
        AlertKind::Stars => (
            format!(
                "Ещё {rest} {} звёздами на разборе",
                format::plural_ru(n, "оплата", "оплаты", "оплат")
            ),
            STARS_PATH,
        ),
        AlertKind::Refunds => (
            format!(
                "Ещё {rest} {} не завершены",
                format::plural_ru(n, "возврат", "возврата", "возвратов")
            ),
            REFUNDS_PATH,
        ),
        AlertKind::Withdrawals => (
            format!(
                "Ещё {rest} {} на вывод",
                format::plural_ru(n, "заявка", "заявки", "заявок")
            ),
            "/admin/m/referral",
        ),
        AlertKind::Payments => (
            format!(
                "Ещё {rest} {} по платежам",
                format::plural_ru(n, "событие", "события", "событий")
            ),
            "/admin/money",
        ),
        AlertKind::Traffic => (
            format!(
                "Ещё {rest} {} трафика",
                format::plural_ru(n, "превышение", "превышения", "превышений")
            ),
            "/admin/clients/limits",
        ),
        AlertKind::Nodes => (
            format!(
                "Ещё {rest} {}",
                format::plural_ru(n, "нода не отвечает", "ноды не отвечают", "нод не отвечают")
            ),
            NODES_PATH,
        ),
        AlertKind::Clients => (
            format!(
                "Ещё {rest} {}",
                format::plural_ru(n, "новый клиент", "новых клиента", "новых клиентов")
            ),
            "/admin/clients",
        ),
        AlertKind::Sales => (
            format!(
                "Ещё {rest} {}",
                format::plural_ru(n, "оплата", "оплаты", "оплат")
            ),
            "/admin/money",
        ),
        AlertKind::Security => (
            format!(
                "Ещё {rest} {} входа по коду",
                format::plural_ru(n, "блокировка", "блокировки", "блокировок")
            ),
            ADMINS_PATH,
        ),
    };
    Alert {
        kind,
        html: text,
        path: path.to_string(),
    }
}

fn charges_summary(rest: usize) -> Alert {
    Alert {
        kind: AlertKind::Orders,
        html: format!(
            "Ещё {rest} {}",
            format::plural_ru(
                rest as u64,
                "списание с карты ждёт ручной проверки",
                "списания с карты ждут ручной проверки",
                "списаний с карты ждут ручной проверки"
            )
        ),
        path: CHARGES_PATH.to_string(),
    }
}

fn window_text(seconds: i64) -> String {
    if seconds == 86_400 {
        return "сутки".to_string();
    }
    if seconds % 3_600 == 0 {
        let hours = seconds / 3_600;
        return format!(
            "{hours} {}",
            format::plural_ru(hours.unsigned_abs(), "час", "часа", "часов")
        );
    }
    let minutes = (seconds / 60).max(1);
    format!(
        "{minutes} {}",
        format::plural_ru(minutes.unsigned_abs(), "минуту", "минуты", "минут")
    )
}

fn lock_text(row: &LockRow) -> String {
    let admin = match (row.email.as_deref(), row.admin_id) {
        (Some(email), _) => escape(email),
        (None, Some(id)) => format!("администратор №{id}"),
        (None, None) => "администратор".to_string(),
    };
    let mut lines = vec![
        "<b>Вход по коду из приложения закрыт</b>".to_string(),
        format!("Администратор: {admin}"),
    ];
    let failures = row
        .details
        .get("failures")
        .and_then(serde_json::Value::as_u64);
    let window = row
        .details
        .get("window_seconds")
        .and_then(serde_json::Value::as_i64);
    if let (Some(failures), Some(window)) = (failures, window) {
        lines.push(format!(
            "{failures} {} за {}, пароль при этом ввели верно",
            format::plural_ru(failures, "неверный код", "неверных кода", "неверных кодов"),
            window_text(window)
        ));
    }
    if let Some(ip) = row.ip.as_deref().map(str::trim).filter(|ip| !ip.is_empty()) {
        lines.push(format!("Адрес: {}", escape(ip)));
    }
    lines.push(
        "Вход по коду откроется сам. Если коды вводили не вы, пароль знает кто-то ещё: смените его"
            .to_string(),
    );
    lines.join("\n")
}

fn clock_text(at: DateTime<Utc>, offset: FixedOffset, now: DateTime<Utc>) -> String {
    let local = at.with_timezone(&offset);
    if local.date_naive() == now.with_timezone(&offset).date_naive() {
        local.format("%H:%M").to_string()
    } else {
        format!("{} в {}", local.format("%d.%m"), local.format("%H:%M"))
    }
}

pub fn outage_text(span: Duration) -> String {
    let minutes = ((span.num_seconds() + 30) / 60).max(1);
    let n = minutes.unsigned_abs();
    if minutes < 60 {
        return format!(
            "{minutes} {}",
            format::plural_ru(n, "минуту", "минуты", "минут")
        );
    }
    if minutes < 24 * 60 {
        let (hours, rest) = (minutes / 60, minutes % 60);
        return if rest == 0 {
            format!("{hours} ч")
        } else {
            format!("{hours} ч {rest} мин")
        };
    }
    let (days, hours) = (minutes / (24 * 60), minutes % (24 * 60) / 60);
    let day = format::plural_ru(days.unsigned_abs(), "день", "дня", "дней");
    if hours == 0 {
        format!("{days} {day}")
    } else {
        format!("{days} {day} {hours} ч")
    }
}

fn node_alert(notice: &NodeNotice, offset: FixedOffset, now: DateTime<Utc>) -> Alert {
    let lines = match notice {
        NodeNotice::Down {
            name,
            since,
            message,
        } => {
            let mut lines = vec![
                format!("<b>Нода {} не отвечает</b>", escape(name)),
                format!("С {}", clock_text(*since, offset, now)),
            ];
            if let Some(message) = message.as_deref().filter(|m| !m.trim().is_empty()) {
                lines.push(format!("Причина: {}", escape(&cut(message, BODY_MAX))));
            }
            lines
        }
        NodeNotice::Back {
            name,
            down_at,
            back_at,
        } => vec![
            format!("<b>Нода {} снова в сети</b>", escape(name)),
            format!("Лежала {}", outage_text(*back_at - *down_at)),
        ],
    };
    Alert {
        kind: AlertKind::Nodes,
        html: lines.join("\n"),
        path: NODES_PATH.to_string(),
    }
}

async fn client_label(db: &mut impl Db, client_id: i64) -> String {
    let Ok(identities) = db.clients().identities(ClientId(client_id)).await else {
        return format!("клиент №{client_id}");
    };
    if let Some(telegram) = identities.iter().find(|i| i.kind == IdentityKind::Telegram) {
        let field = |key: &str| {
            telegram
                .meta
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let name = [field("first_name"), field("last_name")]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        return match (name.is_empty(), field("username")) {
            (false, Some(user)) => format!("{name} (@{user})"),
            (false, None) => name,
            (true, Some(user)) => format!("@{user}"),
            (true, None) => format!("клиент №{client_id}"),
        };
    }
    identities
        .iter()
        .find(|i| i.kind == IdentityKind::Email)
        .map(|i| i.value.clone())
        .unwrap_or_else(|| format!("клиент №{client_id}"))
}

impl<'a> AlertsOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        AlertsOps { services }
    }

    pub async fn config(&self) -> ServiceResult<AlertsConfig> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .settings()
            .get_as::<AlertsConfig>(SETTING_KEY)
            .await?
            .unwrap_or_default())
    }

    async fn save(&self, config: &AlertsConfig, by: Option<AdminId>) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.settings().set_as(SETTING_KEY, config, by).await?;
        Ok(())
    }

    async fn state(&self, key: &str) -> ServiceResult<Option<serde_json::Value>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.bot_state().get(key).await?.filter(|v| !v.is_null()))
    }

    async fn put_state(&self, key: &str, value: &serde_json::Value) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.bot_state().set(key, value).await?;
        Ok(())
    }

    async fn bot_username(&self) -> ServiceResult<Option<String>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .settings()
            .get_as::<String>(keys::BOT_USERNAME)
            .await?
            .map(|name| name.trim().trim_start_matches('@').to_string())
            .filter(|name| !name.is_empty()))
    }

    pub fn kinds(&self) -> Vec<AlertKind> {
        let limits = self
            .services
            .modules()
            .is_enabled(sn_domain::module::ModuleId::Limits);
        AlertKind::ALL
            .into_iter()
            .filter(|kind| limits || *kind != AlertKind::Traffic)
            .collect()
    }

    pub async fn view(&self) -> ServiceResult<AlertsView> {
        let config = self.config().await?;
        let kinds = self
            .kinds()
            .iter()
            .map(|kind| {
                let route = config.route(*kind);
                KindView {
                    kind: *kind,
                    title: kind.title_ru(),
                    default_topic: kind.topic_ru(),
                    on: route.on,
                    topic: route.topic,
                }
            })
            .collect();
        Ok(AlertsView {
            chat_id: config.chat_id,
            title: config.title.clone(),
            forum: config.forum,
            bot_username: self.bot_username().await?,
            kinds,
            test_pending: self.state(TEST_KEY).await?.is_some(),
        })
    }

    pub async fn update(&self, update: &AlertsUpdate, admin: AdminId) -> ServiceResult<AlertsView> {
        let mut config = self.config().await?;
        for input in &update.routes {
            let topic = input
                .topic
                .as_deref()
                .map(clean_topic)
                .filter(|t| !t.is_empty());
            config.routes.insert(
                input.kind,
                AlertRoute {
                    on: input.on,
                    topic,
                },
            );
        }
        self.save(&config, Some(admin)).await?;
        self.view().await
    }

    pub async fn link(&self) -> ServiceResult<AlertLink> {
        let Some(username) = self.bot_username().await? else {
            return Err(ServiceError::rejected(
                "alerts.no_bot",
                "Сначала подключите бота в разделе «Настройки → Бот»",
            ));
        };
        let mut raw = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut raw);
        let code = hex::encode(raw);
        let expires_at = self.services.now() + Duration::minutes(LINK_TTL_MINUTES);
        let pending = PendingLink {
            hash: hex::encode(token_hash(&code)),
            expires_at,
        };
        self.put_state(
            LINK_KEY,
            &serde_json::to_value(&pending).map_err(|e| ServiceError::Internal(e.to_string()))?,
        )
        .await?;
        Ok(AlertLink {
            link: format!("https://t.me/{username}?startgroup={START_PREFIX}{code}"),
            expires_at,
        })
    }

    pub async fn unlink(&self, admin: AdminId) -> ServiceResult<AlertsView> {
        let mut config = self.config().await?;
        config.chat_id = None;
        config.title = None;
        config.forum = false;
        self.save(&config, Some(admin)).await?;
        self.put_state(TOPICS_KEY, &serde_json::json!({})).await?;
        self.put_state(LINK_KEY, &serde_json::Value::Null).await?;
        self.view().await
    }

    pub async fn bind_group(
        &self,
        code: &str,
        chat_id: i64,
        title: Option<String>,
        forum: bool,
    ) -> ServiceResult<bool> {
        let Some(raw) = self.state(LINK_KEY).await? else {
            return Ok(false);
        };
        let Ok(pending) = serde_json::from_value::<PendingLink>(raw) else {
            return Ok(false);
        };
        let given = hex::encode(token_hash(code.trim()));
        if pending.expires_at <= self.services.now() || pending.hash != given {
            return Ok(false);
        }
        let mut config = self.config().await?;
        if config.chat_id != Some(chat_id) {
            self.put_state(TOPICS_KEY, &serde_json::json!({})).await?;
        }
        config.chat_id = Some(chat_id);
        config.title = title;
        config.forum = forum;
        self.save(&config, None).await?;
        self.put_state(LINK_KEY, &serde_json::Value::Null).await?;
        Ok(true)
    }

    pub async fn refresh_chat(
        &self,
        chat_id: i64,
        title: Option<String>,
        forum: bool,
    ) -> ServiceResult<()> {
        let mut config = self.config().await?;
        if config.chat_id != Some(chat_id) || (config.title == title && config.forum == forum) {
            return Ok(());
        }
        config.title = title;
        config.forum = forum;
        self.save(&config, None).await
    }

    pub async fn adopt_chat(
        &self,
        chat_id: i64,
        title: Option<String>,
        forum: bool,
    ) -> ServiceResult<()> {
        let mut config = self.config().await?;
        config.chat_id = Some(chat_id);
        config.title = title;
        config.forum = forum;
        self.save(&config, None).await
    }

    pub async fn drop_chat(&self, chat_id: i64) -> ServiceResult<()> {
        let mut config = self.config().await?;
        if config.chat_id != Some(chat_id) {
            return Ok(());
        }
        config.chat_id = None;
        config.title = None;
        config.forum = false;
        self.save(&config, None).await?;
        self.put_state(TOPICS_KEY, &serde_json::json!({})).await
    }

    pub async fn topics(&self) -> ServiceResult<TopicMap> {
        Ok(self
            .state(TOPICS_KEY)
            .await?
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default())
    }

    pub async fn remember_topic(&self, name: &str, thread_id: i64) -> ServiceResult<()> {
        let mut topics = self.topics().await?;
        topics.insert(name.to_string(), thread_id);
        self.put_state(TOPICS_KEY, &serde_json::json!(topics)).await
    }

    pub async fn forget_topic(&self, name: &str) -> ServiceResult<()> {
        let mut topics = self.topics().await?;
        if topics.remove(name).is_some() {
            self.put_state(TOPICS_KEY, &serde_json::json!(topics))
                .await?;
        }
        Ok(())
    }

    pub async fn request_test(&self) -> ServiceResult<()> {
        if self.config().await?.chat_id.is_none() {
            return Err(ServiceError::rejected(
                "alerts.no_group",
                "Сначала подключите группу для уведомлений",
            ));
        }
        self.put_state(TEST_KEY, &serde_json::json!(self.services.now()))
            .await
    }

    pub async fn take_test(&self) -> ServiceResult<bool> {
        if self.state(TEST_KEY).await?.is_none() {
            return Ok(false);
        }
        self.put_state(TEST_KEY, &serde_json::Value::Null).await?;
        Ok(true)
    }

    pub async fn collect(&self) -> ServiceResult<Vec<Alert>> {
        let config = self.config().await?;
        let mut out = self.support(&config).await?;
        out.extend(self.ai_handoffs(&config).await?);
        out.extend(self.payments(&config).await?);
        for kind in [
            AlertKind::Orders,
            AlertKind::Stars,
            AlertKind::Refunds,
            AlertKind::Withdrawals,
            AlertKind::Security,
        ] {
            out.extend(self.watched(kind, &config).await?);
        }
        out.extend(self.charges(&config).await?);
        out.extend(self.auto_refunds(&config).await?);
        out.extend(self.traffic(&config).await?);
        out.extend(self.nodes(&config).await?);
        out.extend(self.new_clients(&config).await?);
        out.extend(self.new_sales(&config).await?);
        Ok(out)
    }

    pub async fn collect_nodes(&self) -> ServiceResult<Vec<Alert>> {
        let config = self.config().await?;
        self.nodes(&config).await
    }

    async fn nodes(&self, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let notices = self
            .services
            .node_watch()
            .notices(config.route(AlertKind::Nodes).on)
            .await?;
        let offset = self.services.settings().remna_offset();
        let now = self.services.now();
        let (down, back): (Vec<NodeNotice>, Vec<NodeNotice>) = notices
            .into_iter()
            .partition(|notice| matches!(notice, NodeNotice::Down { .. }));
        let mut out: Vec<Alert> = down
            .iter()
            .take(PER_KIND)
            .map(|notice| node_alert(notice, offset, now))
            .collect();
        if down.len() > PER_KIND {
            out.push(summary(AlertKind::Nodes, down.len() - PER_KIND));
        }
        out.extend(
            back.iter()
                .take(PER_KIND)
                .map(|notice| node_alert(notice, offset, now)),
        );
        if back.len() > PER_KIND {
            let rest = back.len() - PER_KIND;
            out.push(Alert {
                kind: AlertKind::Nodes,
                html: format!(
                    "Ещё {rest} {} снова в сети",
                    format::plural_ru(rest as u64, "нода", "ноды", "нод")
                ),
                path: NODES_PATH.to_string(),
            });
        }
        Ok(out)
    }

    async fn auto_refunds(&self, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let cursor = self
            .state(AUTO_REFUND_CURSOR)
            .await?
            .and_then(|v| v.as_i64());
        let mut conn = self.services.store().conn().await?;
        let newest: i64 = sqlx::query_scalar("SELECT coalesce(max(id), 0) FROM audit_log")
            .fetch_one(conn.conn())
            .await?;
        let Some(cursor) = cursor else {
            drop(conn);
            self.put_state(AUTO_REFUND_CURSOR, &serde_json::json!(newest))
                .await?;
            return Ok(Vec::new());
        };
        if newest <= cursor {
            return Ok(Vec::new());
        }
        let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT action, details FROM audit_log \
             WHERE id > $1 AND id <= $2 AND action IN ($3, $4) ORDER BY id",
        )
        .bind(cursor)
        .bind(newest)
        .bind(BLOCKED_REFUND_AUDIT)
        .bind(BLOCKED_REFUND_REQUESTED_AUDIT)
        .fetch_all(conn.conn())
        .await?;
        let mut out = Vec::new();
        if config.route(AlertKind::Refunds).on {
            for (action, details) in rows.iter().take(PER_KIND) {
                let done = action == BLOCKED_REFUND_AUDIT;
                let mut lines = vec![if done {
                    "<b>Оплату вернули автоматически</b>".to_string()
                } else {
                    "<b>Оплату возвращаем автоматически</b>".to_string()
                }];
                match details.get("order_id").and_then(serde_json::Value::as_i64) {
                    Some(order) => {
                        lines.push(format!("Клиент заблокирован, заказ №{order} не выдан"))
                    }
                    None => lines.push("Клиент заблокирован, заказ не выдан".to_string()),
                }
                if let Some(client) = details.get("client_id").and_then(serde_json::Value::as_i64) {
                    lines.push(format!(
                        "Клиент: {}",
                        escape(&client_label(&mut conn, client).await)
                    ));
                }
                if let Some(sum) = details.get("amount").and_then(amount_line) {
                    lines.push(format!("Сумма: {sum}"));
                }
                if !done {
                    lines.push("Ждём подтверждения платёжной системы".to_string());
                }
                out.push(Alert {
                    kind: AlertKind::Refunds,
                    html: lines.join("\n"),
                    path: "/admin/money".to_string(),
                });
            }
            if rows.len() > PER_KIND {
                let rest = rows.len() - PER_KIND;
                out.push(Alert {
                    kind: AlertKind::Refunds,
                    html: format!(
                        "Ещё {rest} {}",
                        format::plural_ru(
                            rest as u64,
                            "автоматический возврат",
                            "автоматических возврата",
                            "автоматических возвратов"
                        )
                    ),
                    path: "/admin/money".to_string(),
                });
            }
        }
        drop(conn);
        self.put_state(AUTO_REFUND_CURSOR, &serde_json::json!(newest))
            .await?;
        Ok(out)
    }

    async fn traffic(&self, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let cursor = self.state(TRAFFIC_CURSOR).await?.and_then(|v| v.as_i64());
        let settled = self.services.now() - Duration::seconds(TRAFFIC_SETTLE_SECONDS);
        let mut conn = self.services.store().conn().await?;
        let newest: i64 = sqlx::query_scalar(
            "SELECT coalesce(max(id), 0) FROM traffic_exceeds WHERE exceeded_at <= $1",
        )
        .bind(settled)
        .fetch_one(conn.conn())
        .await?;
        let Some(cursor) = cursor else {
            drop(conn);
            self.put_state(TRAFFIC_CURSOR, &serde_json::json!(newest))
                .await?;
            return Ok(Vec::new());
        };
        if newest <= cursor {
            return Ok(Vec::new());
        }
        let rows: Vec<TrafficRow> = sqlx::query_as(
            "SELECT client_id, period, used_bytes, limit_bytes, restriction, restricted_at, \
                    window_end, restrict_note, warned_at \
             FROM traffic_exceeds WHERE alert AND id > $1 AND id <= $2 ORDER BY id",
        )
        .bind(cursor)
        .bind(newest)
        .fetch_all(conn.conn())
        .await?;
        let mut out = Vec::new();
        if config.route(AlertKind::Traffic).on {
            let offset = self.services.settings().remna_offset();
            for row in rows.iter().take(PER_KIND) {
                let used = format::traffic(u64::try_from(row.used_bytes).unwrap_or(0));
                let limit = format::traffic(u64::try_from(row.limit_bytes).unwrap_or(0));
                let when = if row.period == "week" {
                    "За неделю"
                } else {
                    "За сутки"
                };
                let mut lines = vec![
                    "<b>Превышение трафика</b>".to_string(),
                    format!(
                        "Клиент: {}",
                        escape(&client_label(&mut conn, row.client_id).await)
                    ),
                    format!("{when}: {used} при пороге {limit}"),
                ];
                match (row.restriction.as_str(), row.restricted_at) {
                    ("none", _) => {}
                    (_, Some(_)) => lines.push(format!(
                        "Доступ ограничен до {}",
                        sn_modules::catalog::limits::until_text(row.window_end, offset)
                    )),
                    (_, None) => lines.push(match row.restrict_note.as_deref() {
                        Some(note) if !note.trim().is_empty() => {
                            format!("Не ограничен: {}", escape(&cut(note, BODY_MAX)))
                        }
                        _ => "Ограничение ещё не применено".to_string(),
                    }),
                }
                if row.warned_at.is_some() {
                    lines.push("Клиент предупреждён".to_string());
                }
                out.push(Alert {
                    kind: AlertKind::Traffic,
                    html: lines.join("\n"),
                    path: "/admin/clients/limits".to_string(),
                });
            }
            if rows.len() > PER_KIND {
                out.push(summary(AlertKind::Traffic, rows.len() - PER_KIND));
            }
        }
        drop(conn);
        self.put_state(TRAFFIC_CURSOR, &serde_json::json!(newest))
            .await?;
        Ok(out)
    }

    async fn support(&self, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let cursor = self.state(SUPPORT_CURSOR).await?.and_then(|v| v.as_i64());
        let mut conn = self.services.store().conn().await?;
        let newest: i64 = sqlx::query_scalar(
            "SELECT coalesce(max(id), 0) FROM ticket_messages WHERE author = 'client'",
        )
        .fetch_one(conn.conn())
        .await?;
        let Some(cursor) = cursor else {
            drop(conn);
            self.put_state(SUPPORT_CURSOR, &serde_json::json!(newest))
                .await?;
            return Ok(Vec::new());
        };
        if newest <= cursor {
            return Ok(Vec::new());
        }
        let ai_answers = self
            .services
            .modules()
            .is_enabled(sn_domain::module::ModuleId::Ai);
        let rows: Vec<(i64, i64, String, Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT m.id, m.ticket_id, m.body, t.subject, t.client_id, \
                    (SELECT min(f.id) FROM ticket_messages f WHERE f.ticket_id = m.ticket_id) \
             FROM ticket_messages m JOIN tickets t ON t.id = m.ticket_id \
             WHERE m.author = 'client' AND m.id > $1 AND m.id <= $2 \
               AND NOT ($3 AND t.ai_state IS NOT DISTINCT FROM 'active') ORDER BY m.id",
        )
        .bind(cursor)
        .bind(newest)
        .bind(ai_answers)
        .fetch_all(conn.conn())
        .await?;
        let mut order: Vec<i64> = Vec::new();
        let mut grouped: BTreeMap<i64, (bool, usize, String, Option<String>, i64)> =
            BTreeMap::new();
        for (_, ticket, body, subject, client, first) in rows {
            let entry = grouped.entry(ticket).or_insert_with(|| {
                order.push(ticket);
                (first > cursor, 0, String::new(), subject.clone(), client)
            });
            entry.1 += 1;
            entry.2 = body;
        }
        let mut out = Vec::new();
        if config.route(AlertKind::Support).on {
            for ticket in order.iter().take(PER_KIND) {
                let Some((fresh, count, body, subject, client)) = grouped.get(ticket) else {
                    continue;
                };
                let head = if *fresh {
                    format!("<b>Новое обращение №{ticket}</b>")
                } else {
                    format!("<b>Новое сообщение в обращении №{ticket}</b>")
                };
                let mut lines = vec![
                    head,
                    format!(
                        "Клиент: {}",
                        escape(&client_label(&mut conn, *client).await)
                    ),
                ];
                if let Some(subject) = subject.as_deref().filter(|s| !s.trim().is_empty()) {
                    lines.push(format!("Тема: {}", escape(&cut(subject, 80))));
                }
                lines.push(format!("«{}»", escape(&cut(body, BODY_MAX))));
                if *count > 1 {
                    lines.push(format!("Сообщений: {count}"));
                }
                out.push(Alert {
                    kind: AlertKind::Support,
                    html: lines.join("\n"),
                    path: format!("/admin/support?ticket={ticket}"),
                });
            }
            if order.len() > PER_KIND {
                out.push(summary(AlertKind::Support, order.len() - PER_KIND));
            }
        }
        drop(conn);
        self.put_state(SUPPORT_CURSOR, &serde_json::json!(newest))
            .await?;
        Ok(out)
    }

    async fn ai_handoffs(&self, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let cursor = self
            .state(AI_HANDOFF_CURSOR)
            .await?
            .and_then(|v| v.as_i64());
        let mut conn = self.services.store().conn().await?;
        let newest: i64 = sqlx::query_scalar(
            "SELECT coalesce(max(id), 0) FROM ai_support_log WHERE outcome = 'handoff'",
        )
        .fetch_one(conn.conn())
        .await?;
        let Some(cursor) = cursor else {
            drop(conn);
            self.put_state(AI_HANDOFF_CURSOR, &serde_json::json!(newest))
                .await?;
            return Ok(Vec::new());
        };
        if newest <= cursor {
            return Ok(Vec::new());
        }
        let rows: Vec<HandoffRow> = sqlx::query_as(
            "SELECT l.ticket_id, l.client_id, l.reason, l.error, t.subject, \
                    (SELECT m.body FROM ticket_messages m WHERE m.ticket_id = l.ticket_id \
                     AND m.author = 'client' ORDER BY m.id DESC LIMIT 1) AS body \
             FROM ai_support_log l JOIN tickets t ON t.id = l.ticket_id \
             WHERE l.outcome = 'handoff' AND l.id > $1 AND l.id <= $2 ORDER BY l.id",
        )
        .bind(cursor)
        .bind(newest)
        .fetch_all(conn.conn())
        .await?;
        let mut out = Vec::new();
        if config.route(AlertKind::Support).on {
            for row in rows.iter().take(PER_KIND) {
                let ticket = row.ticket_id;
                let mut lines = vec![
                    format!("<b>ИИ передал обращение №{ticket} оператору</b>"),
                    format!(
                        "Клиент: {}",
                        escape(&client_label(&mut conn, row.client_id).await)
                    ),
                ];
                if let Some(reason) = row
                    .reason
                    .as_deref()
                    .and_then(|r| sn_domain::support::HandoffReason::parse(r).ok())
                {
                    lines.push(format!("Причина: {}", reason.title_ru()));
                }
                if let Some(error) = row.error.as_deref().filter(|e| !e.trim().is_empty()) {
                    lines.push(format!("Ошибка: {}", escape(&cut(error, BODY_MAX))));
                }
                if let Some(subject) = row.subject.as_deref().filter(|s| !s.trim().is_empty()) {
                    lines.push(format!("Тема: {}", escape(&cut(subject, 80))));
                }
                if let Some(body) = row.body.as_deref().filter(|b| !b.trim().is_empty()) {
                    lines.push(format!("«{}»", escape(&cut(body, BODY_MAX))));
                }
                out.push(Alert {
                    kind: AlertKind::Support,
                    html: lines.join("\n"),
                    path: format!("/admin/support?ticket={ticket}"),
                });
            }
            if rows.len() > PER_KIND {
                let rest = rows.len() - PER_KIND;
                out.push(Alert {
                    kind: AlertKind::Support,
                    html: format!(
                        "Ещё {rest} {} ИИ передал оператору",
                        format::plural_ru(rest as u64, "обращение", "обращения", "обращений")
                    ),
                    path: "/admin/support".to_string(),
                });
            }
        }
        drop(conn);
        self.put_state(AI_HANDOFF_CURSOR, &serde_json::json!(newest))
            .await?;
        Ok(out)
    }

    async fn payments(&self, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let cursor = self.state(PAYMENTS_CURSOR).await?.and_then(|v| v.as_i64());
        let mut conn = self.services.store().conn().await?;
        let newest = conn.payment_notices().newest_id().await?;
        let Some(cursor) = cursor else {
            drop(conn);
            self.put_state(PAYMENTS_CURSOR, &serde_json::json!(newest))
                .await?;
            return Ok(Vec::new());
        };
        if newest <= cursor {
            return Ok(Vec::new());
        }
        let notices = conn
            .payment_notices()
            .between(cursor, newest, SCAN_LIMIT)
            .await?;
        let mut out = Vec::new();
        if config.route(AlertKind::Payments).on {
            let registry = self.services.payments();
            for notice in notices.iter().take(PER_KIND) {
                let provider = registry
                    .get(&notice.provider)
                    .map_or_else(|| notice.provider.clone(), |p| p.title().to_string());
                let mut lines = vec![
                    format!("<b>{}</b>", escape(notice.kind.title_ru())),
                    format!("Платёжная система: {}", escape(&provider)),
                ];
                if let Some(client) = notice.client_id {
                    lines.push(format!(
                        "Клиент: {}",
                        escape(&client_label(&mut conn, client.0).await)
                    ));
                }
                if let Some(amount) = notice.amount {
                    lines.push(format!("Сумма: {}", escape(&amount_text(amount))));
                }
                lines.push(escape(&cut(&notice.text, NOTICE_MAX)));
                out.push(Alert {
                    kind: AlertKind::Payments,
                    html: lines.join("\n"),
                    path: match notice.client_id {
                        Some(client) => format!("/admin/clients/{}", client.0),
                        None => "/admin/money".to_string(),
                    },
                });
            }
            if notices.len() > PER_KIND {
                out.push(summary(AlertKind::Payments, notices.len() - PER_KIND));
            }
        }
        drop(conn);
        self.put_state(PAYMENTS_CURSOR, &serde_json::json!(newest))
            .await?;
        Ok(out)
    }

    async fn items(&self, kind: AlertKind) -> ServiceResult<Vec<Item>> {
        let mut conn = self.services.store().conn().await?;
        let mut items = Vec::new();
        match kind {
            AlertKind::Orders => {
                let rows: Vec<OrderRow> = sqlx::query_as(
                    "SELECT id, client_id, total_minor, currency, stars_total, review_reason \
                     FROM orders WHERE status = 'review' ORDER BY id LIMIT $1",
                )
                .bind(SCAN_LIMIT)
                .fetch_all(conn.conn())
                .await?;
                for row in rows {
                    let (id, client, total, currency, stars, reason) = (
                        row.id,
                        row.client_id,
                        row.total_minor,
                        row.currency,
                        row.stars_total,
                        row.review_reason,
                    );
                    let mut lines = vec![
                        format!("<b>Заказ №{id} ждёт ручной проверки</b>"),
                        format!("Клиент: {}", escape(&client_label(&mut conn, client).await)),
                    ];
                    match (stars.filter(|s| *s > 0), money(total, &currency)) {
                        (Some(stars), _) => lines.push(format!(
                            "Сумма: {stars} {}",
                            format::plural_ru(stars.unsigned_abs(), "звезда", "звезды", "звёзд")
                        )),
                        (None, Some(sum)) => lines.push(format!("Сумма: {sum}")),
                        (None, None) => {}
                    }
                    if let Some(reason) = reason.as_deref().filter(|r| !r.trim().is_empty()) {
                        lines.push(format!("Причина: {}", escape(&cut(reason, BODY_MAX))));
                    }
                    items.push(Item {
                        id,
                        html: lines.join("\n"),
                        path: ORDERS_PATH.to_string(),
                    });
                }
            }
            AlertKind::Stars => {
                let rows: Vec<(i64, serde_json::Value, Option<String>)> = sqlx::query_as(
                    "SELECT id, payload, last_error FROM bot_star_events \
                     WHERE status = 'review' ORDER BY id LIMIT $1",
                )
                .bind(SCAN_LIMIT)
                .fetch_all(conn.conn())
                .await?;
                for (id, payload, error) in rows {
                    let mut lines = vec!["<b>Оплата звёздами на разборе</b>".to_string()];
                    if let Some(amount) = payload
                        .get("total_amount")
                        .and_then(serde_json::Value::as_i64)
                    {
                        lines.push(format!(
                            "Сумма: {amount} {}",
                            format::plural_ru(amount.unsigned_abs(), "звезда", "звезды", "звёзд")
                        ));
                    }
                    if let Some(error) = error.as_deref().filter(|e| !e.trim().is_empty()) {
                        lines.push(format!("Причина: {}", escape(&cut(error, BODY_MAX))));
                    }
                    items.push(Item {
                        id,
                        html: lines.join("\n"),
                        path: STARS_PATH.to_string(),
                    });
                }
            }
            AlertKind::Refunds => {
                let rows: Vec<RefundRow> = sqlx::query_as(
                    "SELECT r.id, r.amount_minor, r.currency, r.action_error, p.client_id \
                     FROM provider_refunds r LEFT JOIN payments p ON p.id = r.payment_id \
                     WHERE r.action_status = 'failed' ORDER BY r.id LIMIT $1",
                )
                .bind(SCAN_LIMIT)
                .fetch_all(conn.conn())
                .await?;
                for row in rows {
                    let (id, amount, currency, error, client) = (
                        row.id,
                        row.amount_minor,
                        row.currency,
                        row.action_error,
                        row.client_id,
                    );
                    let mut lines = vec![format!("<b>Возврат №{id} не завершён</b>")];
                    if let Some(client) = client {
                        lines.push(format!(
                            "Клиент: {}",
                            escape(&client_label(&mut conn, client).await)
                        ));
                    }
                    if let Some(sum) = money(amount, &currency) {
                        lines.push(format!("Сумма: {sum}"));
                    }
                    if let Some(error) = error.as_deref().filter(|e| !e.trim().is_empty()) {
                        lines.push(format!("Причина: {}", escape(&cut(error, BODY_MAX))));
                    }
                    items.push(Item {
                        id,
                        html: lines.join("\n"),
                        path: REFUNDS_PATH.to_string(),
                    });
                }
            }
            AlertKind::Withdrawals => {
                let rows: Vec<(i64, i64, i64, String, String, Option<String>)> = sqlx::query_as(
                    "SELECT id, client_id, amount_minor, currency, method, method_title FROM referral_withdrawals \
                     WHERE status IN ('holding', 'pending') ORDER BY id LIMIT $1",
                )
                .bind(SCAN_LIMIT)
                .fetch_all(conn.conn())
                .await?;
                for (id, client, amount, currency, method, title) in rows {
                    let method = payout_method(&method, title.as_deref());
                    let mut lines = vec![
                        format!("<b>Заявка на вывод №{id}</b>"),
                        format!("Клиент: {}", escape(&client_label(&mut conn, client).await)),
                    ];
                    match money(amount, &currency) {
                        Some(sum) => lines.push(format!("Сумма: {sum}, {}", escape(&method))),
                        None => lines.push(format!("Способ: {}", escape(&method))),
                    }
                    items.push(Item {
                        id,
                        html: lines.join("\n"),
                        path: "/admin/m/referral".to_string(),
                    });
                }
            }
            AlertKind::Security => {
                let mut rows: Vec<LockRow> = sqlx::query_as(
                    "SELECT l.id, l.entity_id AS admin_id, a.email, l.details, l.ip \
                     FROM audit_log l LEFT JOIN admins a ON a.id = l.entity_id \
                     WHERE l.entity_kind = 'admin' AND l.action = $1 \
                       AND l.created_at > now() - make_interval(days => $2) \
                     ORDER BY l.id DESC LIMIT $3",
                )
                .bind(TOTP_LOCKED_AUDIT)
                .bind(SECURITY_DAYS)
                .bind(SCAN_LIMIT)
                .fetch_all(conn.conn())
                .await?;
                rows.reverse();
                for row in rows {
                    items.push(Item {
                        id: row.id,
                        html: lock_text(&row),
                        path: ADMINS_PATH.to_string(),
                    });
                }
            }
            AlertKind::Support
            | AlertKind::Payments
            | AlertKind::Traffic
            | AlertKind::Nodes
            | AlertKind::Clients
            | AlertKind::Sales => {}
        }
        Ok(items)
    }

    async fn charge_items(&self) -> ServiceResult<Vec<Item>> {
        let mut conn = self.services.store().conn().await?;
        let mut charges = conn.payments().charges_review_list(SCAN_LIMIT).await?;
        charges.reverse();
        let mut items = Vec::new();
        for charge in charges {
            let payment = &charge.payment;
            let mut lines = vec![
                format!(
                    "<b>Списание с карты №{} ждёт ручной проверки</b>",
                    payment.id.0
                ),
                format!(
                    "Клиент: {}",
                    escape(&client_label(&mut conn, payment.client_id.0).await)
                ),
                format!("Сумма: {}", escape(&amount_text(payment.amount))),
            ];
            if let Some(reason) = charge.reason.as_deref().filter(|r| !r.trim().is_empty()) {
                lines.push(format!("Причина: {}", escape(&cut(reason, BODY_MAX))));
            }
            items.push(Item {
                id: payment.id.0,
                html: lines.join("\n"),
                path: CHARGES_PATH.to_string(),
            });
        }
        Ok(items)
    }

    async fn charges(&self, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let items = self.charge_items().await?;
        self.unseen(
            AlertKind::Orders,
            CHARGES_SEEN,
            items,
            config,
            charges_summary,
        )
        .await
    }

    async fn watched(&self, kind: AlertKind, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let items = self.items(kind).await?;
        self.unseen(kind, &seen_key(kind), items, config, |rest| {
            summary(kind, rest)
        })
        .await
    }

    async fn unseen(
        &self,
        kind: AlertKind,
        key: &str,
        items: Vec<Item>,
        config: &AlertsConfig,
        more: impl Fn(usize) -> Alert,
    ) -> ServiceResult<Vec<Alert>> {
        let current: BTreeSet<i64> = items.iter().map(|i| i.id).collect();
        let seen: Option<BTreeSet<i64>> = self
            .state(key)
            .await?
            .and_then(|v| serde_json::from_value(v).ok());
        self.put_state(key, &serde_json::json!(current)).await?;
        let Some(seen) = seen else {
            return Ok(Vec::new());
        };
        if !config.route(kind).on {
            return Ok(Vec::new());
        }
        let fresh: Vec<Item> = items
            .into_iter()
            .filter(|i| !seen.contains(&i.id))
            .collect();
        let rest = fresh.len().saturating_sub(PER_KIND);
        let mut out: Vec<Alert> = fresh
            .into_iter()
            .take(PER_KIND)
            .map(|item| Alert {
                kind,
                html: item.html,
                path: item.path,
            })
            .collect();
        if rest > 0 {
            out.push(more(rest));
        }
        Ok(out)
    }
}
