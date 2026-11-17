use crate::auth::token_hash;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sn_domain::alerts::{
    AlertKind, AlertRoute, AlertsConfig, SETTING_KEY, START_PREFIX, TopicMap, clean_topic,
};
use sn_domain::client::IdentityKind;
use sn_domain::format;
use sn_domain::ids::{AdminId, ClientId};
use sn_domain::money::{Currency, Money};
use sn_domain::settings::keys;
use sn_store::Db;
use std::collections::{BTreeMap, BTreeSet};

pub const LINK_TTL_MINUTES: i64 = 30;
pub const PER_KIND: usize = 3;
const LINK_KEY: &str = "alerts.link";
const TEST_KEY: &str = "alerts.test";
const TOPICS_KEY: &str = "alerts.topics";
const SUPPORT_CURSOR: &str = "alerts.cursor.support";
const BODY_MAX: usize = 300;
const SCAN_LIMIT: i64 = 500;

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

fn payout_method(method: &str) -> &str {
    match method {
        "sbp" => "СБП",
        "card" => "банковская карта",
        "usdt_trc20" => "USDT (TRC20)",
        other => other,
    }
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
            "/admin/money",
        ),
        AlertKind::Stars => (
            format!(
                "Ещё {rest} {} звёздами на разборе",
                format::plural_ru(n, "оплата", "оплаты", "оплат")
            ),
            "/admin/money",
        ),
        AlertKind::Refunds => (
            format!(
                "Ещё {rest} {} не завершены",
                format::plural_ru(n, "возврат", "возврата", "возвратов")
            ),
            "/admin/money",
        ),
        AlertKind::Withdrawals => (
            format!(
                "Ещё {rest} {} на вывод",
                format::plural_ru(n, "заявка", "заявки", "заявок")
            ),
            "/admin/m/referral",
        ),
    };
    Alert {
        kind,
        html: text,
        path: path.to_string(),
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

    pub async fn view(&self) -> ServiceResult<AlertsView> {
        let config = self.config().await?;
        let kinds = AlertKind::ALL
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
        for kind in [
            AlertKind::Orders,
            AlertKind::Stars,
            AlertKind::Refunds,
            AlertKind::Withdrawals,
        ] {
            out.extend(self.watched(kind, &config).await?);
        }
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
        let rows: Vec<(i64, i64, String, Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT m.id, m.ticket_id, m.body, t.subject, t.client_id, \
                    (SELECT min(f.id) FROM ticket_messages f WHERE f.ticket_id = m.ticket_id) \
             FROM ticket_messages m JOIN tickets t ON t.id = m.ticket_id \
             WHERE m.author = 'client' AND m.id > $1 AND m.id <= $2 ORDER BY m.id",
        )
        .bind(cursor)
        .bind(newest)
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
                        path: "/admin/money".to_string(),
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
                        path: "/admin/money".to_string(),
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
                        lines.push(format!("Ошибка: {}", escape(&cut(error, BODY_MAX))));
                    }
                    items.push(Item {
                        id,
                        html: lines.join("\n"),
                        path: "/admin/money".to_string(),
                    });
                }
            }
            AlertKind::Withdrawals => {
                let rows: Vec<(i64, i64, i64, String, String)> = sqlx::query_as(
                    "SELECT id, client_id, amount_minor, currency, method FROM referral_withdrawals \
                     WHERE status IN ('holding', 'pending') ORDER BY id LIMIT $1",
                )
                .bind(SCAN_LIMIT)
                .fetch_all(conn.conn())
                .await?;
                for (id, client, amount, currency, method) in rows {
                    let mut lines = vec![
                        format!("<b>Заявка на вывод №{id}</b>"),
                        format!("Клиент: {}", escape(&client_label(&mut conn, client).await)),
                    ];
                    match money(amount, &currency) {
                        Some(sum) => {
                            lines.push(format!("Сумма: {sum}, {}", escape(payout_method(&method))))
                        }
                        None => lines.push(format!("Способ: {}", escape(payout_method(&method)))),
                    }
                    items.push(Item {
                        id,
                        html: lines.join("\n"),
                        path: "/admin/m/referral".to_string(),
                    });
                }
            }
            AlertKind::Support => {}
        }
        Ok(items)
    }

    async fn watched(&self, kind: AlertKind, config: &AlertsConfig) -> ServiceResult<Vec<Alert>> {
        let items = self.items(kind).await?;
        let current: BTreeSet<i64> = items.iter().map(|i| i.id).collect();
        let key = seen_key(kind);
        let seen: Option<BTreeSet<i64>> = self
            .state(&key)
            .await?
            .and_then(|v| serde_json::from_value(v).ok());
        self.put_state(&key, &serde_json::json!(current)).await?;
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
            out.push(summary(kind, rest));
        }
        Ok(out)
    }
}
