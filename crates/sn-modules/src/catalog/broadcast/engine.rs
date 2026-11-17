use super::store::{self, Claimed, ImageData, Lease, Message, Outcome};
use super::{ButtonSpec, Status, audit};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::sender::{BotSender, Delivery, OutButton, Outgoing, Photo};
use sn_store::Db;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio::time::Instant;
use uuid::Uuid;

pub const LEASE_KEY: &str = "broadcast.engine";
const DUE_BATCH: i64 = 5;
const STALE_FLIGHT_SECS: i64 = 300;
const CHAT_MEMORY: usize = 4096;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub per_second: u32,
    pub chat_gap: Duration,
    pub in_flight: usize,
    pub idle: Duration,
    pub retries: i32,
    pub retry_pause: Duration,
    pub bot_pause: Duration,
    pub lease: Duration,
    pub grace: Duration,
    pub reject_limit: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            per_second: 25,
            chat_gap: Duration::from_secs(1),
            in_flight: 8,
            idle: Duration::from_secs(2),
            retries: 3,
            retry_pause: Duration::from_secs(5),
            bot_pause: Duration::from_secs(30),
            lease: Duration::from_secs(20),
            grace: Duration::from_secs(8),
            reject_limit: 10,
        }
    }
}

struct Pace {
    spacing: Duration,
    gap: Duration,
    next: Instant,
    paused_until: Instant,
    chats: HashMap<i64, Instant>,
}

impl Pace {
    fn new(config: &EngineConfig) -> Self {
        let now = Instant::now();
        Pace {
            spacing: Duration::from_secs(1) / config.per_second.max(1),
            gap: config.chat_gap,
            next: now,
            paused_until: now,
            chats: HashMap::new(),
        }
    }

    fn wait(&self) -> Duration {
        self.next
            .max(self.paused_until)
            .saturating_duration_since(Instant::now())
    }

    fn take(&mut self) -> Duration {
        let now = Instant::now();
        let slot = self.next.max(self.paused_until).max(now);
        self.next = slot + self.spacing;
        slot.saturating_duration_since(now)
    }

    fn pause(&mut self, pause: Duration) {
        self.paused_until = self.paused_until.max(Instant::now() + pause);
    }

    fn chat(&mut self, chat: i64) -> Duration {
        let now = Instant::now();
        if self.chats.len() > CHAT_MEMORY {
            let gap = self.gap;
            self.chats.retain(|_, at| *at + gap > now);
        }
        let ready = self
            .chats
            .get(&chat)
            .map_or(now, |at| (*at + self.gap).max(now));
        self.chats.insert(chat, ready);
        ready.saturating_duration_since(now)
    }
}

enum Source {
    Text,
    Cached(Arc<str>),
    Upload(Arc<ImageData>),
}

impl Source {
    fn photo(&self) -> Option<Photo<'_>> {
        match self {
            Source::Text => None,
            Source::Cached(file_id) => Some(Photo::Cached(file_id)),
            Source::Upload(image) => Some(Photo::Upload {
                bytes: &image.bytes,
                content_type: &image.content_type,
            }),
        }
    }
}

struct Prepared {
    id: i64,
    text: Arc<str>,
    buttons: Arc<[OutButton]>,
    image: Option<Arc<ImageData>>,
    file: Option<(i64, Arc<str>)>,
    rejected: u32,
    notice: bool,
}

impl Prepared {
    fn new(id: i64, message: Message) -> Self {
        let file = message.image.as_ref().and_then(|image| {
            image
                .file_bot
                .zip(image.file_id.as_deref())
                .map(|(bot, file)| (bot, Arc::from(file)))
        });
        Prepared {
            id,
            text: Arc::from(message.text),
            buttons: message.buttons.iter().map(ButtonSpec::out).collect(),
            image: message.image.map(Arc::new),
            file,
            rejected: 0,
            notice: true,
        }
    }

    fn source(&self, bot: Option<i64>) -> Source {
        let Some(image) = &self.image else {
            return Source::Text;
        };
        match (&self.file, bot) {
            (Some((file_bot, file)), Some(bot)) if *file_bot == bot => Source::Cached(file.clone()),
            _ => Source::Upload(image.clone()),
        }
    }
}

struct Flight {
    recipient: Claimed,
    delivery: Delivery,
    cached: bool,
}

struct State {
    pace: Pace,
    lead: Option<Instant>,
    prepared: Option<Prepared>,
}

pub struct Engine {
    ctx: ModuleContext,
    sender: Arc<dyn BotSender>,
    enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    config: EngineConfig,
    owner: String,
}

impl Engine {
    pub fn new(
        ctx: ModuleContext,
        sender: Arc<dyn BotSender>,
        enabled: impl Fn() -> bool + Send + Sync + 'static,
        config: EngineConfig,
    ) -> Self {
        Engine {
            ctx,
            sender,
            enabled: Arc::new(enabled),
            config,
            owner: Uuid::new_v4().to_string(),
        }
    }

    pub async fn run(self, shutdown: impl Future<Output = ()> + Send + 'static) {
        let (signal, mut stop) = watch::channel(false);
        tokio::spawn(async move {
            shutdown.await;
            let _ = signal.send(true);
        });
        let mut state = State {
            pace: Pace::new(&self.config),
            lead: None,
            prepared: None,
        };
        tracing::info!("рассылки: отправка запущена");
        loop {
            if *stop.borrow() {
                break;
            }
            let pause = match self.step(&mut state, &mut stop).await {
                Ok(pause) => pause,
                Err(err) => {
                    tracing::warn!(error = %err, "рассылки: отправка споткнулась, повторим");
                    state.lead = None;
                    state.prepared = None;
                    self.config.idle * 3
                }
            };
            if pause.is_zero() {
                continue;
            }
            tokio::select! {
                _ = stop.changed() => {}
                _ = tokio::time::sleep(pause) => {}
            }
        }
        if state.lead.is_some()
            && let Ok(mut conn) = self.ctx.store.conn().await
            && let Err(err) = store::drop_lease(&mut conn, LEASE_KEY, &self.owner).await
        {
            tracing::debug!(error = %err, "рассылки: аренда отправки не снята");
        }
        tracing::info!("рассылки: отправка остановлена");
    }

    async fn lead(&self, lead: &mut Option<Instant>) -> Result<bool, ModuleError> {
        let now = Instant::now();
        if let Some(until) = *lead
            && until > now + self.config.lease / 2
        {
            return Ok(true);
        }
        let mut conn = self.ctx.store.conn().await?;
        let taken = store::take_lease(
            &mut conn,
            &Lease {
                key: LEASE_KEY,
                owner: &self.owner,
                seconds: self.config.lease.as_secs_f64(),
            },
        )
        .await?;
        if !taken {
            *lead = None;
            return Ok(false);
        }
        if lead.is_none() {
            let abandoned = store::abandon_flights(&mut conn, None).await?;
            if abandoned > 0 {
                tracing::warn!(
                    abandoned,
                    "рассылки: сообщения, которые отправлялись до перезапуска, не повторяются"
                );
            }
        }
        *lead = Some(now + self.config.lease);
        Ok(true)
    }

    async fn step(
        &self,
        state: &mut State,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<Duration, ModuleError> {
        if !(self.enabled)() || !self.lead(&mut state.lead).await? {
            return Ok(self.config.idle);
        }
        self.start_due().await?;
        let mut conn = self.ctx.store.conn().await?;
        let Some(id) = store::next_sending(&mut conn).await? else {
            state.prepared = None;
            return Ok(self.config.idle);
        };
        let prepared = match state.prepared.take() {
            Some(prepared) if prepared.id == id => prepared,
            _ => Prepared::new(id, store::message(&mut conn, id).await?),
        };
        drop(conn);
        let mut prepared = prepared;
        let pumped = self
            .pump(&mut prepared, &mut state.pace, &mut state.lead, stop)
            .await;
        state.prepared = Some(prepared);
        pumped
    }

    async fn start_due(&self) -> Result<(), ModuleError> {
        let mut tx = self.ctx.store.begin().await?;
        let due = store::due(&mut tx, DUE_BATCH).await?;
        if due.is_empty() {
            tx.rollback().await?;
            return Ok(());
        }
        let now = crate::catalog::db_now(&mut tx).await?;
        for (id, audience) in due {
            let total = store::launch(&mut tx, id, audience, now, None).await?;
            if total == 0 {
                store::finish_empty(&mut tx, id).await?;
            }
            tx.audit()
                .record(&audit(
                    "broadcast.started",
                    None,
                    Some(id),
                    serde_json::json!({ "total": total, "scheduled": true, "audience": audience }),
                ))
                .await?;
            tracing::info!(broadcast = id, total, "рассылка по расписанию начата");
        }
        tx.commit().await?;
        Ok(())
    }

    async fn pump(
        &self,
        prepared: &mut Prepared,
        pace: &mut Pace,
        lead: &mut Option<Instant>,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<Duration, ModuleError> {
        let mut flights: JoinSet<Flight> = JoinSet::new();
        let driven = self.drive(prepared, pace, lead, stop, &mut flights).await;
        match driven {
            Ok(stopping) => self.after(prepared.id, stopping).await,
            Err(err) => {
                flights.abort_all();
                *lead = None;
                Err(err)
            }
        }
    }

    async fn drive(
        &self,
        prepared: &mut Prepared,
        pace: &mut Pace,
        lead: &mut Option<Instant>,
        stop: &mut watch::Receiver<bool>,
        flights: &mut JoinSet<Flight>,
    ) -> Result<bool, ModuleError> {
        let id = prepared.id;
        let mut exhausted = false;
        let mut stopping_since: Option<Instant> = None;
        loop {
            while let Some(joined) = flights.try_join_next() {
                self.settle(id, joined, prepared, pace).await?;
            }
            if stopping_since.is_none()
                && (*stop.borrow() || !(self.enabled)() || !self.lead(lead).await?)
            {
                stopping_since = Some(Instant::now());
            }
            if let Some(since) = stopping_since
                && since.elapsed() > self.config.grace
            {
                if !flights.is_empty() {
                    tracing::warn!(
                        left = flights.len(),
                        "рассылки: не дождались ответа Telegram при остановке"
                    );
                    flights.abort_all();
                }
                return Ok(true);
            }
            let uploading = matches!(prepared.source(self.sender.bot_id()), Source::Upload(_));
            let limit = if uploading {
                1
            } else {
                self.config.in_flight.max(1)
            };
            if stopping_since.is_some() || exhausted || flights.len() >= limit {
                if flights.is_empty() {
                    return Ok(stopping_since.is_some());
                }
                let grace = stopping_since.map_or(self.config.idle, |since| {
                    self.config.grace.saturating_sub(since.elapsed())
                });
                tokio::select! {
                    joined = flights.join_next() => {
                        if let Some(joined) = joined {
                            self.settle(id, joined, prepared, pace).await?;
                        }
                    }
                    _ = tokio::time::sleep(grace) => {}
                }
                continue;
            }
            let wait = pace.wait();
            if !wait.is_zero() {
                tokio::select! {
                    joined = flights.join_next(), if !flights.is_empty() => {
                        if let Some(joined) = joined {
                            self.settle(id, joined, prepared, pace).await?;
                        }
                    }
                    _ = tokio::time::sleep(wait.min(Duration::from_secs(1))) => {}
                    _ = stop.changed() => {}
                }
                continue;
            }
            let claimed = {
                let mut conn = self.ctx.store.conn().await?;
                store::claim(&mut conn, id, 1).await?
            };
            if claimed.is_empty() {
                exhausted = true;
            }
            for recipient in claimed {
                let slot = pace.take();
                let delay = pace.chat(recipient.chat_id).max(slot);
                flights.spawn(self.flight(prepared, recipient, delay));
            }
        }
    }

    fn flight(
        &self,
        prepared: &Prepared,
        recipient: Claimed,
        delay: Duration,
    ) -> impl Future<Output = Flight> + Send + 'static {
        let sender = self.sender.clone();
        let text = prepared.text.clone();
        let buttons = prepared.buttons.clone();
        let source = prepared.source(self.sender.bot_id());
        async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let delivery = sender
                .send(
                    recipient.chat_id,
                    &Outgoing {
                        html: &text,
                        photo: source.photo(),
                        buttons: &buttons,
                    },
                )
                .await;
            Flight {
                recipient,
                delivery,
                cached: matches!(source, Source::Cached(_)),
            }
        }
    }

    async fn settle(
        &self,
        id: i64,
        joined: Result<Flight, JoinError>,
        prepared: &mut Prepared,
        pace: &mut Pace,
    ) -> Result<(), ModuleError> {
        let flight = match joined {
            Ok(flight) => flight,
            Err(err) => {
                tracing::warn!(error = %err, "рассылки: отправка сообщения оборвалась");
                return Ok(());
            }
        };
        let client = flight.recipient.client_id;
        let mut conn = self.ctx.store.conn().await?;
        match flight.delivery {
            Delivery::Sent { file_id } => {
                store::record(&mut conn, id, client, &Outcome::Sent).await?;
                prepared.rejected = 0;
                if let (Some(file_id), Some(image), Some(bot), false) = (
                    file_id,
                    prepared.image.as_ref(),
                    self.sender.bot_id(),
                    flight.cached,
                ) {
                    store::remember_file(&mut conn, image.id, bot, &file_id).await?;
                    prepared.file = Some((bot, Arc::from(file_id)));
                }
                if prepared.notice {
                    store::set_notice(&mut conn, id, None).await?;
                    prepared.notice = false;
                }
            }
            Delivery::Blocked(reason) => {
                store::record(&mut conn, id, client, &Outcome::Blocked(&reason)).await?;
            }
            Delivery::Wait(pause) => {
                pace.pause(pause);
                store::record(
                    &mut conn,
                    id,
                    client,
                    &Outcome::Retry {
                        after_ms: millis(pause),
                        error: None,
                        counted: false,
                    },
                )
                .await?;
                let notice = format!(
                    "Telegram попросил подождать {} с: отправка продолжится сама",
                    pause.as_secs().max(1)
                );
                store::set_notice(&mut conn, id, Some(&notice)).await?;
                prepared.notice = true;
            }
            Delivery::StaleFile => {
                if let Some(image) = &prepared.image {
                    store::forget_file(&mut conn, image.id).await?;
                }
                prepared.file = None;
                store::record(
                    &mut conn,
                    id,
                    client,
                    &Outcome::Retry {
                        after_ms: 0,
                        error: None,
                        counted: false,
                    },
                )
                .await?;
            }
            Delivery::Failed { error, retry: true }
                if flight.recipient.attempts < self.config.retries =>
            {
                let factor = u32::try_from(flight.recipient.attempts.max(1)).unwrap_or(1);
                store::record(
                    &mut conn,
                    id,
                    client,
                    &Outcome::Retry {
                        after_ms: millis(self.config.retry_pause * factor),
                        error: Some(&error),
                        counted: true,
                    },
                )
                .await?;
            }
            Delivery::Failed { error, retry } => {
                store::record(&mut conn, id, client, &Outcome::Failed(&error)).await?;
                if !retry {
                    prepared.rejected += 1;
                }
                if prepared.rejected >= self.config.reject_limit
                    && store::sent_count(&mut conn, id).await? == 0
                {
                    drop(conn);
                    let mut tx = self.ctx.store.begin().await?;
                    let notice = format!("Остановлена: Telegram не принимает сообщение — {error}");
                    store::stop(&mut tx, id, Some(&notice)).await?;
                    tx.audit()
                        .record(&audit(
                            "broadcast.canceled",
                            None,
                            Some(id),
                            serde_json::json!({ "reason": error }),
                        ))
                        .await?;
                    tx.commit().await?;
                    tracing::warn!(broadcast = id, %error, "рассылка остановлена: Telegram отклоняет сообщение");
                }
            }
            Delivery::NoBot(reason) => {
                pace.pause(self.config.bot_pause);
                store::record(
                    &mut conn,
                    id,
                    client,
                    &Outcome::Retry {
                        after_ms: 0,
                        error: None,
                        counted: false,
                    },
                )
                .await?;
                let notice = format!(
                    "Бот не подключён: {reason}. Отправка продолжится, когда бот заработает"
                );
                store::set_notice(&mut conn, id, Some(&notice)).await?;
                prepared.notice = true;
            }
        }
        Ok(())
    }

    async fn after(&self, id: i64, stopping: bool) -> Result<Duration, ModuleError> {
        let mut conn = self.ctx.store.conn().await?;
        if store::complete(&mut conn, id).await? {
            tracing::info!(broadcast = id, "рассылка отправлена");
            return Ok(Duration::ZERO);
        }
        if stopping {
            return Ok(self.config.idle);
        }
        if store::status(&mut conn, id).await? != Some(Status::Sending) {
            return Ok(Duration::ZERO);
        }
        if store::abandon_flights(&mut conn, Some((id, STALE_FLIGHT_SECS))).await? > 0 {
            return Ok(Duration::ZERO);
        }
        let wait = store::next_retry_ms(&mut conn, id)
            .await?
            .map_or(self.config.idle, |ms| {
                Duration::from_millis(u64::try_from(ms).unwrap_or(0))
            });
        Ok(wait.clamp(Duration::from_millis(10), self.config.idle))
    }
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod pace_tests {
    use super::*;

    fn config(per_second: u32, gap_ms: u64) -> EngineConfig {
        EngineConfig {
            per_second,
            chat_gap: Duration::from_millis(gap_ms),
            ..EngineConfig::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn темп_не_выше_заданного_в_секунду() {
        let mut pace = Pace::new(&config(25, 1000));
        let start = Instant::now();
        let mut sent = 0;
        while start.elapsed() < Duration::from_secs(1) {
            let wait = pace.wait();
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
                continue;
            }
            assert_eq!(pace.take(), Duration::ZERO);
            sent += 1;
        }
        assert!((24..=26).contains(&sent), "за секунду ушло {sent}");
    }

    #[tokio::test(start_paused = true)]
    async fn подряд_взятые_места_раздвигаются_по_темпу() {
        let mut pace = Pace::new(&config(25, 1000));
        assert_eq!(pace.take(), Duration::ZERO);
        assert_eq!(pace.take(), Duration::from_millis(40));
        assert_eq!(pace.take(), Duration::from_millis(80));
        pace.pause(Duration::from_secs(2));
        assert_eq!(pace.take(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn один_чат_не_чаще_раза_в_секунду() {
        let mut pace = Pace::new(&config(25, 1000));
        assert_eq!(pace.chat(7), Duration::ZERO);
        assert_eq!(pace.chat(8), Duration::ZERO);
        assert_eq!(pace.chat(7), Duration::from_secs(1));
        assert_eq!(pace.chat(7), Duration::from_secs(2));
        tokio::time::advance(Duration::from_millis(2500)).await;
        assert_eq!(pace.chat(7), Duration::from_millis(500));
        assert_eq!(pace.chat(8), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn пауза_по_просьбе_телеграма_держит_всех() {
        let mut pace = Pace::new(&config(25, 1000));
        pace.pause(Duration::from_secs(7));
        assert_eq!(pace.wait(), Duration::from_secs(7));
        pace.pause(Duration::from_secs(2));
        assert_eq!(pace.wait(), Duration::from_secs(7));
        tokio::time::advance(Duration::from_secs(7)).await;
        assert_eq!(pace.wait(), Duration::ZERO);
    }
}
