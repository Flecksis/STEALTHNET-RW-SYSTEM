use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use sn_domain::settings::{
    BotDelivery, FallbackReason, UpdatesMode, WebhookFallback, keys, secret_keys,
};
use sn_store::Db;

pub const PATH_PREFIX: &str = "/api/webhooks/telegram/";
pub const SECRET_HEADER: &str = "x-telegram-bot-api-secret-token";
pub const UPDATES_CHANNEL: &str = "sn_bot_updates";
pub const MAX_BODY: usize = 1024 * 1024;
pub const MAX_CONNECTIONS: u32 = 40;
pub const ALLOWED_UPDATES: [&str; 4] = [
    "message",
    "callback_query",
    "pre_checkout_query",
    "widget_query",
];
pub const WEBHOOK_PORTS: [u16; 4] = [443, 80, 88, 8443];
pub const STATE_KEY: &str = "telegram.webhook";
pub const KEEP_HOURS: i32 = 48;
pub const PURGE_BATCH: i64 = 5_000;
pub const BROKEN_ACTION: &str = "settings.bot_webhook_broken";
pub const RESTORED_ACTION: &str = "settings.bot_webhook_restored";
pub const FALLBACK_ACTION: &str = "settings.bot_webhook_fallback";
pub const MODE_ACTION: &str = "settings.bot_updates";
pub const NEEDS_HTTPS: &str = "bot.webhook_needs_https";
pub const BAD_PORT: &str = "bot.webhook_port";
pub const NEEDS_TOKEN: &str = "bot.webhook_needs_token";
const ERROR_MAX: usize = 300;
const PATH_LABEL: &[u8] = b"telegram-webhook-path";
const TOKEN_LABEL: &[u8] = b"telegram-webhook-token";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, PartialEq, Eq)]
pub struct WebhookKeys {
    master: [u8; 32],
}

impl std::fmt::Debug for WebhookKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookKeys(…)")
    }
}

impl WebhookKeys {
    pub fn generate() -> Self {
        let mut master = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut master);
        WebhookKeys { master }
    }

    pub fn from_secret(text: &str) -> Option<Self> {
        let raw = STANDARD.decode(text.trim()).ok()?;
        let master: [u8; 32] = raw.try_into().ok()?;
        Some(WebhookKeys { master })
    }

    pub fn to_secret(&self) -> String {
        STANDARD.encode(self.master)
    }

    fn mac(&self, label: &[u8]) -> Option<HmacSha256> {
        let mut mac = HmacSha256::new_from_slice(&self.master).ok()?;
        mac.update(label);
        Some(mac)
    }

    fn derived(&self, label: &[u8]) -> String {
        self.mac(label)
            .map(|mac| URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
            .unwrap_or_default()
    }

    fn verify(&self, label: &[u8], presented: &str) -> bool {
        let Ok(raw) = URL_SAFE_NO_PAD.decode(presented.trim().as_bytes()) else {
            return false;
        };
        if raw.len() != 32 {
            return false;
        }
        self.mac(label)
            .is_some_and(|mac| mac.verify_slice(&raw).is_ok())
    }

    pub fn path_segment(&self) -> String {
        self.derived(PATH_LABEL)
    }

    pub fn header_token(&self) -> String {
        self.derived(TOKEN_LABEL)
    }

    pub fn check_path(&self, segment: &str) -> bool {
        self.verify(PATH_LABEL, segment)
    }

    pub fn check_token(&self, token: &str) -> bool {
        self.verify(TOKEN_LABEL, token)
    }
}

pub fn https_base(public_url: Option<&str>) -> Option<String> {
    let base = public_url?.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(base).ok()?;
    (parsed.scheme() == "https" && parsed.host_str().is_some_and(|h| !h.is_empty()))
        .then(|| base.to_string())
}

pub fn port_fits(base: &str) -> bool {
    reqwest::Url::parse(base)
        .ok()
        .and_then(|url| url.port_or_known_default())
        .is_some_and(|port| WEBHOOK_PORTS.contains(&port))
}

pub fn webhook_url(base: &str, keys: &WebhookKeys) -> String {
    format!(
        "{}{PATH_PREFIX}{}",
        base.trim_end_matches('/'),
        keys.path_segment()
    )
}

pub fn shown_url(base: &str) -> String {
    format!("{}{PATH_PREFIX}…", base.trim_end_matches('/'))
}

pub fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

pub fn bot_id(token: &str) -> Option<i64> {
    token
        .split_once(':')
        .and_then(|(id, _)| id.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
}

pub fn is_ours(url: &str, hosts: &[String]) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    parsed.path().contains(PATH_PREFIX)
        && parsed
            .host_str()
            .is_some_and(|host| hosts.iter().any(|h| h.eq_ignore_ascii_case(host)))
}

pub fn clip_error(text: &str) -> String {
    let clean: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= ERROR_MAX {
        clean
    } else {
        let mut cut: String = clean.chars().take(ERROR_MAX - 1).collect();
        cut.push('…');
        cut
    }
}

pub fn mode_word(mode: UpdatesMode) -> &'static str {
    match mode {
        UpdatesMode::Polling => "опрос",
        UpdatesMode::Webhook => "вебхук",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookSnapshot {
    #[serde(default)]
    pub bot_id: Option<i64>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub registered_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub checked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub pending: Option<i64>,
    #[serde(default)]
    pub last_error_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub broken_since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reported: bool,
    #[serde(default)]
    pub max_connections: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inbound {
    Queued(i64),
    Duplicate(i64),
    Sifted(i64),
    Malformed,
    Off,
    BadPath,
    BadToken,
    NoBot,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct QueueView {
    pub pending: i64,
    pub oldest_at: Option<DateTime<Utc>>,
    pub last_received_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdatesView {
    pub mode: UpdatesMode,
    pub fallback: Option<WebhookFallback>,
    pub address: Option<String>,
    pub blocker: Option<&'static str>,
    pub custom_api: bool,
    pub telegram: Option<WebhookSnapshot>,
    pub queue: QueueView,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    pub update_id: i64,
    pub payload: Value,
}

pub async fn take(db: &mut impl Db, bot_id: i64, limit: i64) -> ServiceResult<Vec<Queued>> {
    let rows: Vec<(i64, Option<Value>)> = sqlx::query_as(
        "SELECT update_id, payload FROM bot_updates \
         WHERE bot_id = $1 AND taken_at IS NULL ORDER BY update_id LIMIT $2 FOR UPDATE",
    )
    .bind(bot_id)
    .bind(limit.max(1))
    .fetch_all(db.conn())
    .await
    .map_err(sn_store::StoreError::from)?;
    Ok(rows
        .into_iter()
        .map(|(update_id, payload)| Queued {
            update_id,
            payload: payload.unwrap_or(Value::Null),
        })
        .collect())
}

pub async fn mark_taken(db: &mut impl Db, bot_id: i64, ids: &[i64]) -> ServiceResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE bot_updates SET taken_at = now(), payload = NULL \
         WHERE bot_id = $1 AND update_id = ANY($2) AND taken_at IS NULL",
    )
    .bind(bot_id)
    .bind(ids)
    .execute(db.conn())
    .await
    .map_err(sn_store::StoreError::from)?;
    Ok(())
}

fn blocked(code: &'static str) -> ServiceError {
    match code {
        NEEDS_HTTPS => ServiceError::rejected(
            NEEDS_HTTPS,
            "Для вебхука нужен адрес сервиса с https. Домен задаётся при установке или командой stealthnet domain на сервере, а пока его нет, бот получает обновления опросом",
        ),
        BAD_PORT => ServiceError::rejected(
            BAD_PORT,
            "Telegram отправляет вебхуки только на порты 443, 80, 88 и 8443, а в адресе сервиса другой порт. Оставьте опрос или откройте панель на 443",
        ),
        _ => ServiceError::NoSecretKey,
    }
}

pub struct BotWebhook<'a> {
    services: &'a Services,
}

impl<'a> BotWebhook<'a> {
    pub fn new(services: &'a Services) -> Self {
        BotWebhook { services }
    }

    pub fn blocker(&self) -> Option<&'static str> {
        let settings = self.services.settings();
        let Some(base) = https_base(settings.public_url.as_deref()) else {
            return Some(NEEDS_HTTPS);
        };
        if self.services.env().telegram_api.is_none() && !port_fits(&base) {
            return Some(BAD_PORT);
        }
        if self.services.secrets().is_none() {
            return Some("no_secret_key");
        }
        None
    }

    pub fn check_ready(&self) -> ServiceResult<()> {
        match self.blocker() {
            Some(code) => Err(blocked(code)),
            None => Ok(()),
        }
    }

    pub fn target(&self) -> Option<(String, WebhookKeys)> {
        let settings = self.services.settings();
        let base = https_base(settings.public_url.as_deref())?;
        let keys = self.services.bot_webhook_keys()?;
        Some((webhook_url(&base, &keys), keys))
    }

    pub async fn our_hosts(&self) -> ServiceResult<Vec<String>> {
        let mut hosts = Vec::new();
        if let Some(host) = https_base(self.services.settings().public_url.as_deref())
            .as_deref()
            .and_then(host_of)
        {
            hosts.push(host);
        }
        if let Some(host) = self.snapshot().await?.and_then(|s| s.host)
            && !hosts.iter().any(|h| h.eq_ignore_ascii_case(&host))
        {
            hosts.push(host);
        }
        Ok(hosts)
    }

    pub async fn is_ours(&self, url: &str) -> ServiceResult<bool> {
        Ok(is_ours(url, &self.our_hosts().await?))
    }

    pub fn wanted(&self) -> bool {
        self.services.settings().bot_updates.mode == UpdatesMode::Webhook
            && self.blocker().is_none()
    }

    pub async fn accept(
        &self,
        segment: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> ServiceResult<Inbound> {
        if self.services.settings().bot_updates.mode != UpdatesMode::Webhook {
            return Ok(Inbound::Off);
        }
        let Some(keys) = self.services.bot_webhook_keys() else {
            return Ok(Inbound::Off);
        };
        if !keys.check_path(segment) {
            return Ok(Inbound::BadPath);
        }
        if !token.is_some_and(|t| keys.check_token(t)) {
            return Ok(Inbound::BadToken);
        }
        let Some(bot) = self.services.bot_token().as_deref().and_then(bot_id) else {
            return Ok(Inbound::NoBot);
        };
        let Ok(value) = serde_json::from_slice::<Value>(body) else {
            return Ok(Inbound::Malformed);
        };
        let Some(update_id) = value
            .as_object()
            .and_then(|o| o.get("update_id"))
            .and_then(Value::as_i64)
        else {
            return Ok(Inbound::Malformed);
        };
        let probe = crate::bot_flood::Probe::from_json(&value);
        let flood = self.services.settings().bot_flood;
        if !self
            .services
            .bot_sieve()
            .pass(&probe, &flood, std::time::Instant::now())
        {
            return Ok(Inbound::Sifted(update_id));
        }
        let mut conn = self.services.store().conn().await?;
        let added: i64 = sqlx::query_scalar(
            "WITH added AS ( \
                 INSERT INTO bot_updates (bot_id, update_id, payload) VALUES ($1, $2, $3) \
                 ON CONFLICT DO NOTHING RETURNING update_id \
             ) \
             SELECT count(*) FROM added, LATERAL (SELECT pg_notify($4, $5)) AS signal",
        )
        .bind(bot)
        .bind(update_id)
        .bind(&value)
        .bind(UPDATES_CHANNEL)
        .bind(self.services.instance_id().to_string())
        .fetch_one(conn.conn())
        .await
        .map_err(sn_store::StoreError::from)?;
        drop(conn);
        if added == 0 {
            return Ok(Inbound::Duplicate(update_id));
        }
        self.services.bot_updates_signal().notify_one();
        Ok(Inbound::Queued(update_id))
    }

    pub async fn has_pending(&self, bot_id: i64) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM bot_updates WHERE bot_id = $1 AND taken_at IS NULL)",
        )
        .bind(bot_id)
        .fetch_one(conn.conn())
        .await
        .map_err(sn_store::StoreError::from)?)
    }

    pub async fn queue(&self, bot_id: i64) -> ServiceResult<QueueView> {
        let mut conn = self.services.store().conn().await?;
        let (pending, oldest_at): (i64, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT count(*), min(received_at) FROM bot_updates \
             WHERE bot_id = $1 AND taken_at IS NULL",
        )
        .bind(bot_id)
        .fetch_one(conn.conn())
        .await
        .map_err(sn_store::StoreError::from)?;
        let last_received_at: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT max(received_at) FROM bot_updates")
                .fetch_one(conn.conn())
                .await
                .map_err(sn_store::StoreError::from)?;
        Ok(QueueView {
            pending,
            oldest_at,
            last_received_at,
        })
    }

    pub async fn last_received(&self) -> ServiceResult<Option<DateTime<Utc>>> {
        let mut conn = self.services.store().conn().await?;
        Ok(
            sqlx::query_scalar("SELECT max(received_at) FROM bot_updates")
                .fetch_one(conn.conn())
                .await
                .map_err(sn_store::StoreError::from)?,
        )
    }

    pub async fn purge(&self, bot_id: i64) -> ServiceResult<u64> {
        let mut total = 0;
        let mut conn = self.services.store().conn().await?;
        for _ in 0..20 {
            let removed = sqlx::query(
                "DELETE FROM bot_updates WHERE ctid = ANY (ARRAY( \
                     SELECT ctid FROM bot_updates \
                     WHERE (taken_at IS NOT NULL OR bot_id <> $1) \
                       AND received_at < now() - make_interval(hours => $2) \
                     LIMIT $3))",
            )
            .bind(bot_id)
            .bind(KEEP_HOURS)
            .bind(PURGE_BATCH)
            .execute(conn.conn())
            .await
            .map_err(sn_store::StoreError::from)?
            .rows_affected();
            total += removed;
            if removed < PURGE_BATCH as u64 {
                break;
            }
        }
        Ok(total)
    }

    pub async fn snapshot(&self) -> ServiceResult<Option<WebhookSnapshot>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .bot_state()
            .get(STATE_KEY)
            .await?
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::from_value(v).ok()))
    }

    pub async fn save_snapshot(&self, snapshot: &WebhookSnapshot) -> ServiceResult<()> {
        let value =
            serde_json::to_value(snapshot).map_err(|e| ServiceError::Internal(e.to_string()))?;
        let mut conn = self.services.store().conn().await?;
        conn.bot_state().set(STATE_KEY, &value).await?;
        Ok(())
    }

    pub async fn rotate_keys(&self) -> ServiceResult<WebhookKeys> {
        let keys = WebhookKeys::generate();
        let mut conn = self.services.store().conn().await?;
        crate::settings::write_secret(
            &mut conn,
            self.services.secrets(),
            secret_keys::BOT_WEBHOOK,
            Some(&keys.to_secret()),
            None,
        )
        .await?;
        drop(conn);
        self.services.reload().await?;
        self.services.notify_settings_changed().await?;
        Ok(keys)
    }

    pub async fn fall_back(
        &self,
        reason: FallbackReason,
        error: Option<String>,
        host: Option<String>,
        minutes: Option<i64>,
    ) -> ServiceResult<()> {
        let error = error.map(|e| clip_error(&e));
        let delivery = BotDelivery {
            mode: UpdatesMode::Polling,
            fallback: Some(WebhookFallback {
                at: self.services.now(),
                reason,
                error: error.clone(),
            }),
        };
        let value =
            serde_json::to_value(&delivery).map_err(|e| ServiceError::Internal(e.to_string()))?;
        let mut tx = self.services.store().begin().await?;
        tx.settings().set(keys::BOT_UPDATES, &value, None).await?;
        tx.audit()
            .record(&Actor::System.audit(
                FALLBACK_ACTION,
                "settings",
                None,
                serde_json::json!({
                    "from": mode_word(UpdatesMode::Webhook),
                    "to": mode_word(UpdatesMode::Polling),
                    "kind": match reason {
                        FallbackReason::Rejected => "rejected",
                        FallbackReason::Undelivered => "undelivered",
                    },
                    "reason": match reason {
                        FallbackReason::Rejected => "Telegram не принял адрес вебхука",
                        FallbackReason::Undelivered => "Telegram долго не мог доставить обновления",
                    },
                    "error": error,
                    "host": host,
                    "minutes": minutes,
                }),
            ))
            .await?;
        tx.commit().await?;
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    pub async fn incident(&self, action: &str, details: Value) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::System.audit(action, "settings", None, details))
            .await?;
        Ok(())
    }

    pub async fn view(&self) -> ServiceResult<UpdatesView> {
        let settings = self.services.settings();
        let base = https_base(settings.public_url.as_deref());
        let queue = match self.services.bot_token().as_deref().and_then(bot_id) {
            Some(bot) => self.queue(bot).await?,
            None => QueueView::default(),
        };
        Ok(UpdatesView {
            mode: settings.bot_updates.mode,
            fallback: settings.bot_updates.fallback.clone(),
            address: base.as_deref().map(shown_url),
            blocker: self.blocker(),
            custom_api: self.services.env().telegram_api.is_some(),
            telegram: self.snapshot().await?,
            queue,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn путь_и_заголовок_проверяются_своими_ключами() {
        let keys = WebhookKeys::generate();
        let other = WebhookKeys::generate();
        let path = keys.path_segment();
        let token = keys.header_token();
        assert_eq!(path.len(), 43);
        assert_eq!(token.len(), 43);
        assert_ne!(path, token);
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        );
        assert!(keys.check_path(&path));
        assert!(keys.check_token(&token));
        assert!(!keys.check_path(&token));
        assert!(!keys.check_token(&path));
        assert!(!other.check_path(&path));
        assert!(!other.check_token(&token));
        assert!(!keys.check_token(""));
        assert!(!keys.check_token("не base64"));
        assert!(!keys.check_token(&token[..42]));
        let restored = WebhookKeys::from_secret(&keys.to_secret()).unwrap();
        assert_eq!(restored, keys);
        assert!(WebhookKeys::from_secret("c2hvcnQ=").is_none());
        assert_eq!(format!("{keys:?}"), "WebhookKeys(…)");
    }

    #[test]
    fn адрес_вебхука_строится_только_из_https() {
        assert_eq!(
            https_base(Some("https://panel.example.com/")).as_deref(),
            Some("https://panel.example.com")
        );
        assert_eq!(https_base(Some("http://panel.example.com")), None);
        assert_eq!(https_base(Some("panel.example.com")), None);
        assert_eq!(https_base(Some("  ")), None);
        assert_eq!(https_base(None), None);
        assert!(port_fits("https://panel.example.com"));
        assert!(port_fits("https://panel.example.com:8443"));
        assert!(!port_fits("https://panel.example.com:9443"));
        let keys = WebhookKeys::generate();
        let url = webhook_url("https://panel.example.com/", &keys);
        assert_eq!(
            url,
            format!(
                "https://panel.example.com/api/webhooks/telegram/{}",
                keys.path_segment()
            )
        );
        assert_eq!(
            shown_url("https://panel.example.com"),
            "https://panel.example.com/api/webhooks/telegram/…"
        );
        let ours = ["PANEL.example.com".to_string()];
        assert!(is_ours(&url, &ours));
        assert!(!is_ours(&url, &[]));
        assert!(!is_ours(&url, &["other.example.com".to_string()]));
        assert!(!is_ours(
            "https://old.example.com/hook/secret",
            &["old.example.com".to_string()]
        ));
        assert_eq!(bot_id("123456:ABC-def"), Some(123_456));
        assert_eq!(bot_id("abc:def"), None);
        assert_eq!(bot_id("0:def"), None);
        assert_eq!(clip_error("  a \n b  "), "a b");
        assert_eq!(clip_error(&"я".repeat(400)).chars().count(), ERROR_MAX);
    }
}
