use super::manifest::{event_info, permission_title};
use super::store::{self, Access, Claimed, KeyState, Outcome};
use super::{address, secret_key, signing};
use crate::error::ServiceResult;
use crate::services::Services;
use crate::settings::read_secret;
use chrono::{DateTime, Duration, Utc};
use reqwest::header::CONTENT_TYPE;
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use std::time::Instant;
use tokio::task::JoinSet;
use tokio::time::{MissedTickBehavior, interval};

pub const MAX_ATTEMPTS: u32 = 8;
const RETRY_SECONDS: [i64; 7] = [30, 120, 600, 1_800, 3_600, 10_800, 21_600];
const BATCH: i64 = 20;
const IN_FLIGHT: usize = 4;
const LEASE_SECONDS: i64 = 120;
const RESPONSE_MAX: usize = 1000;
const ERROR_MAX: usize = 300;
const KEEP_DAYS: i64 = 14;
const TICK: std::time::Duration = std::time::Duration::from_secs(1);
const CLEANUP_EVERY: std::time::Duration = std::time::Duration::from_secs(600);
const HEALTH_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

pub const PING: &str = "ping";
pub const SETTINGS_CHANGED: &str = "settings_changed";

pub fn retry_delay(attempts: u32) -> Option<Duration> {
    if attempts == 0 || attempts >= MAX_ATTEMPTS {
        return None;
    }
    let index = usize::try_from(attempts - 1)
        .unwrap_or(usize::MAX)
        .min(RETRY_SECONDS.len() - 1);
    Some(Duration::seconds(RETRY_SECONDS[index]))
}

pub fn message_id(delivery: &Claimed) -> String {
    match delivery.event_id {
        Some(event) => format!("evt_{event}"),
        None => format!("msg_{}", delivery.id),
    }
}

pub fn body(module: &str, delivery: &Claimed) -> Vec<u8> {
    let mut data = delivery.payload.clone();
    if let Value::Object(map) = &mut data {
        map.remove("type");
    }
    serde_json::to_vec(&serde_json::json!({
        "id": message_id(delivery),
        "type": delivery.kind,
        "module": module,
        "occurred_at": delivery.occurred_at,
        "data": data,
    }))
    .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct Target {
    pub module: String,
    pub url: String,
    pub key: Vec<u8>,
    pub loopback: bool,
    pub internal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub http_status: Option<u16>,
    pub duration_ms: u32,
    pub error: Option<String>,
    pub response: Option<String>,
    pub retry: bool,
}

impl Sent {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }

    fn failed(message: impl Into<String>) -> Self {
        Sent {
            http_status: None,
            duration_ms: 0,
            error: Some(cut(&message.into(), ERROR_MAX)),
            response: None,
            retry: true,
        }
    }

    fn refused(message: impl Into<String>) -> Self {
        Sent {
            retry: false,
            ..Sent::failed(message)
        }
    }
}

fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn not_found(err: &(dyn Error + 'static)) -> bool {
    let mut current: Option<&(dyn Error + 'static)> = Some(err);
    while let Some(e) = current {
        if e.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
        {
            return true;
        }
        current = e.source();
    }
    false
}

fn transport(err: &reqwest::Error) -> String {
    if let Some(blocked) = address::blocked_in(err) {
        return blocked.to_string();
    }
    if let Some(outside) = address::outside_in(err) {
        return outside.to_string();
    }
    if err.is_timeout() {
        "модуль не ответил за 10 секунд".into()
    } else if not_found(err) {
        "домен модуля не найден".into()
    } else if err.is_connect() {
        "не удалось соединиться с модулем".into()
    } else {
        "запрос к модулю не прошёл".into()
    }
}

async fn excerpt(mut response: reqwest::Response) -> Option<String> {
    let mut buffer: Vec<u8> = Vec::new();
    while buffer.len() < RESPONSE_MAX {
        match response.chunk().await {
            Ok(Some(chunk)) => buffer.extend_from_slice(&chunk),
            _ => break,
        }
    }
    buffer.truncate(RESPONSE_MAX);
    let text = String::from_utf8_lossy(&buffer).trim().to_string();
    (!text.is_empty()).then(|| cut(&text, RESPONSE_MAX))
}

fn elapsed(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

pub async fn send(target: &Target, delivery: &Claimed, now: DateTime<Utc>) -> Sent {
    let url = if target.internal {
        match reqwest::Url::parse(&target.url) {
            Ok(url) if url.scheme() == "http" => url,
            _ => return Sent::refused("адрес модуля внутри сервера не разобран"),
        }
    } else {
        match address::check_with(&target.url, target.loopback) {
            Ok(url) => url,
            Err(message) => return Sent::refused(message),
        }
    };
    let client = if target.internal {
        address::internal_client()
    } else {
        address::client(target.loopback)
    };
    let id = message_id(delivery);
    let timestamp = now.timestamp();
    let body = body(&target.module, delivery);
    let Some(signature) = signing::signature(&target.key, &id, timestamp, &body) else {
        return Sent::failed("секрет подписи не прочитан");
    };
    let started = Instant::now();
    let result = client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .header("webhook-id", &id)
        .header("webhook-timestamp", timestamp.to_string())
        .header("webhook-signature", signature)
        .body(body)
        .send()
        .await;
    let response = match result {
        Ok(response) => response,
        Err(err) => {
            return Sent {
                duration_ms: elapsed(started),
                ..Sent::failed(transport(&err))
            };
        }
    };
    let status = response.status();
    let text = excerpt(response).await;
    let error = if status.is_success() {
        None
    } else if status.is_redirection() {
        Some(format!(
            "модуль ответил перенаправлением (код {}) — перенаправления не выполняются",
            status.as_u16()
        ))
    } else {
        Some(format!("модуль ответил кодом {}", status.as_u16()))
    };
    Sent {
        http_status: Some(status.as_u16()),
        duration_ms: elapsed(started),
        error,
        response: text,
        retry: true,
    }
}

pub fn withheld(access: &Access, kind: &str) -> Option<String> {
    let needs = event_info(kind)?.needs;
    let reason = match access.state {
        KeyState::Active if access.granted.contains(&needs) => return None,
        KeyState::Active => format!(
            "у администратора, который выпустил ключ модуля, нет права «{}»",
            permission_title(needs)
        ),
        KeyState::Missing | KeyState::Revoked => "ключ API модуля отозван".to_string(),
        KeyState::Expired => "срок ключа API модуля истёк".to_string(),
        KeyState::OwnerDisabled => {
            "администратор, который выпустил ключ модуля, отключён".to_string()
        }
    };
    Some(format!("{reason}: событие не отправлено"))
}

pub fn outcome(attempts: u32, sent: Sent, now: DateTime<Utc>, retries: bool) -> Outcome {
    let (status, next_attempt_at) = if sent.ok() {
        ("done", None)
    } else {
        match retry_delay(attempts).filter(|_| retries && sent.retry) {
            Some(delay) => ("pending", Some(now + delay)),
            None => ("failed", None),
        }
    };
    Outcome {
        status,
        attempts,
        next_attempt_at,
        http_status: sent.http_status,
        duration_ms: sent.duration_ms,
        error: sent.error,
        response: sent.response,
    }
}

#[derive(Clone)]
pub struct Deliverer {
    services: Services,
}

impl Deliverer {
    pub fn new(services: Services) -> Self {
        Deliverer { services }
    }

    pub async fn target(
        &self,
        module: &str,
    ) -> ServiceResult<Option<(bool, Result<Target, String>)>> {
        let mut conn = self.services.store().conn().await?;
        let Some(row) = store::get(&mut conn, module).await? else {
            return Ok(None);
        };
        let internal = row.managed;
        let url = match &row.manifest.run {
            Some(run) if internal => run.events_path.as_deref().map(|path| {
                super::runtime::url(
                    self.services.update_config().as_deref(),
                    module,
                    run.port,
                    path,
                )
            }),
            _ => row.manifest.webhook.clone(),
        };
        let Some(url) = url else {
            return Ok(Some((
                row.enabled,
                Err("у модуля нет адреса для событий".to_string()),
            )));
        };
        let secret = read_secret(&mut conn, self.services.secrets(), &secret_key(module)).await?;
        let key = secret.as_deref().and_then(signing::key);
        Ok(Some((
            row.enabled,
            key.map(|key| Target {
                module: module.to_string(),
                url,
                key,
                loopback: self.services.env().ext_modules_loopback,
                internal,
            })
            .ok_or_else(|| {
                "секрет подписи не прочитан: проверьте, что SN_SECRET_KEY не менялся".to_string()
            }),
        )))
    }

    pub async fn deliver_module(&self, module: &str) -> ServiceResult<usize> {
        let Some((enabled, target)) = self.target(module).await? else {
            return Ok(0);
        };
        if !enabled {
            let mut conn = self.services.store().conn().await?;
            store::skip_pending(&mut conn, module).await?;
            return Ok(0);
        }
        let now = self.services.now();
        let (batch, access) = {
            let mut conn = self.services.store().conn().await?;
            let batch = store::claim(
                &mut conn,
                module,
                now,
                now + Duration::seconds(LEASE_SECONDS),
                BATCH,
            )
            .await?;
            if batch.is_empty() {
                return Ok(0);
            }
            (batch, store::access(&mut conn, module, now).await?)
        };
        let count = batch.len();
        let mut held: Vec<(Claimed, String)> = Vec::new();
        let mut ready: Vec<Claimed> = Vec::with_capacity(count);
        for delivery in batch {
            match withheld(&access, &delivery.kind) {
                Some(reason) => held.push((delivery, reason)),
                None => ready.push(delivery),
            }
        }
        let mut results: Vec<(Claimed, Sent)> = match target {
            Err(message) => ready
                .into_iter()
                .map(|d| (d, Sent::failed(message.clone())))
                .collect(),
            Ok(target) => {
                let mut pending = ready.into_iter();
                let mut set = JoinSet::new();
                let mut results = Vec::with_capacity(count);
                for delivery in pending.by_ref().take(IN_FLIGHT) {
                    let target = target.clone();
                    set.spawn(async move {
                        let sent = send(&target, &delivery, now).await;
                        (delivery, sent)
                    });
                }
                while let Some(joined) = set.join_next().await {
                    match joined {
                        Ok(pair) => results.push(pair),
                        Err(err) => {
                            tracing::error!(module, error = %err, "отправка события модулю упала")
                        }
                    }
                    if let Some(delivery) = pending.next() {
                        let target = target.clone();
                        set.spawn(async move {
                            let sent = send(&target, &delivery, now).await;
                            (delivery, sent)
                        });
                    }
                }
                results
            }
        };
        results.sort_by_key(|(delivery, _)| delivery.id);
        let at = self.services.now();
        let mut conn = self.services.store().conn().await?;
        if let Some((_, reason)) = held.first() {
            tracing::warn!(
                module,
                count = held.len(),
                reason = reason.as_str(),
                "события внешнему модулю не отправлены"
            );
        }
        for (delivery, reason) in held {
            let result = Outcome {
                status: "skipped",
                attempts: delivery.attempts.saturating_add(1),
                next_attempt_at: None,
                http_status: None,
                duration_ms: 0,
                error: Some(reason),
                response: None,
            };
            store::record(&mut conn, module, delivery.id, at, &result).await?;
        }
        for (delivery, sent) in results {
            let attempts = delivery.attempts.saturating_add(1);
            if !sent.ok() {
                tracing::warn!(
                    module,
                    delivery = delivery.id,
                    kind = delivery.kind.as_str(),
                    attempts,
                    error = sent.error.as_deref().unwrap_or_default(),
                    "внешний модуль не принял событие"
                );
            }
            let retries = delivery.kind != PING;
            let result = outcome(attempts, sent, at, retries);
            store::record(&mut conn, module, delivery.id, at, &result).await?;
        }
        Ok(count)
    }

    pub async fn round(&self) -> ServiceResult<usize> {
        let modules = {
            let mut conn = self.services.store().conn().await?;
            store::due_modules(&mut conn, self.services.now()).await?
        };
        let mut sent = 0;
        for module in modules {
            sent += self.deliver_module(&module).await?;
        }
        Ok(sent)
    }

    pub async fn cleanup(&self) -> ServiceResult<u64> {
        let mut conn = self.services.store().conn().await?;
        store::cleanup(&mut conn, self.services.now() - Duration::days(KEEP_DAYS)).await
    }

    async fn due(&self) -> ServiceResult<Vec<String>> {
        let mut conn = self.services.store().conn().await?;
        store::due_modules(&mut conn, self.services.now()).await
    }

    pub async fn run(self, shutdown: impl Future<Output = ()>) {
        tokio::pin!(shutdown);
        let mut tick = interval(TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut cleanup = interval(CLEANUP_EVERY);
        cleanup.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut health = interval(HEALTH_EVERY);
        health.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut probing: Option<tokio::task::JoinHandle<()>> = None;
        let mut tasks: JoinSet<()> = JoinSet::new();
        let mut busy: HashMap<tokio::task::Id, String> = HashMap::new();
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                Some(done) = tasks.join_next_with_id(), if !tasks.is_empty() => {
                    let id = match done {
                        Ok((id, ())) => id,
                        Err(err) => {
                            tracing::error!(error = %err, "доставка внешнему модулю упала");
                            err.id()
                        }
                    };
                    busy.remove(&id);
                }
                _ = cleanup.tick() => {
                    if let Err(err) = self.cleanup().await {
                        tracing::warn!(error = %err, "старые доставки внешним модулям не удалены");
                    }
                    self.services.ext_modules().cleanup_uploads();
                }
                _ = health.tick() => {
                    if self.services.pause_gate().is_paused()
                        || probing.as_ref().is_some_and(|h| !h.is_finished())
                    {
                        continue;
                    }
                    let services = self.services.clone();
                    probing = Some(tokio::spawn(async move {
                        if let Err(err) = services.ext_modules().probe_health().await {
                            tracing::warn!(error = %err, "проверка модулей из архива не прошла");
                        }
                    }));
                }
                _ = tick.tick() => {
                    if self.services.pause_gate().is_paused() {
                        continue;
                    }
                    let due = match self.due().await {
                        Ok(due) => due,
                        Err(err) => {
                            tracing::warn!(error = %err, "очередь внешних модулей недоступна");
                            continue;
                        }
                    };
                    for module in due {
                        if busy.values().any(|m| *m == module) {
                            continue;
                        }
                        let this = self.clone();
                        let name = module.clone();
                        let handle = tasks.spawn(async move {
                            if let Err(err) = this.deliver_module(&name).await {
                                tracing::warn!(module = name.as_str(), error = %err, "события внешнему модулю не отправлены");
                            }
                        });
                        busy.insert(handle.id(), module);
                    }
                }
            }
        }
        tasks.shutdown().await;
        if let Some(handle) = probing {
            handle.abort();
        }
    }
}
