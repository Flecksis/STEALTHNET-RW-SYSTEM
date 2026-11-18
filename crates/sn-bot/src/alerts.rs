use crate::BotError;
use crate::telegram::{Keyboard, Message, TelegramApi, TelegramError};
use crate::text::{escape, lowercase_first};
use sn_domain::alerts::{AlertsConfig, START_PREFIX};
use sn_services::Services;
use sn_services::alerts::Alert;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const NODES_EVERY: Duration = Duration::from_secs(15);
const OUTBOX_MAX: usize = 200;

pub struct Keeper {
    services: Services,
    api: Arc<TelegramApi>,
    tick: Duration,
    sweep_every: u32,
    nodes_every: u32,
}

impl Keeper {
    pub fn new(services: Services, api: Arc<TelegramApi>, tick: Duration, sweep: Duration) -> Self {
        let ticks = (sweep.as_millis() / tick.as_millis().max(1)).max(1);
        let nodes = (NODES_EVERY.as_millis() / tick.as_millis().max(1)).max(1);
        Keeper {
            services,
            api,
            tick,
            sweep_every: u32::try_from(ticks).unwrap_or(u32::MAX),
            nodes_every: u32::try_from(nodes).unwrap_or(u32::MAX),
        }
    }

    pub async fn run(self, cancel: CancellationToken) {
        let mut ticks: u32 = 0;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(self.tick) => {}
            }
            if self.services.pause_gate().is_paused() {
                continue;
            }
            let sweep = ticks.is_multiple_of(self.sweep_every);
            if let Err(err) = self.step(sweep).await {
                tracing::warn!(error = %err, "уведомления администраторам не отправлены");
            }
            if !sweep
                && ticks.is_multiple_of(self.nodes_every)
                && let Err(err) = self.nodes_step().await
            {
                tracing::warn!(error = %err, "уведомления о нодах не отправлены");
            }
            ticks = ticks.wrapping_add(1);
        }
    }

    pub async fn step(&self, sweep: bool) -> Result<(), BotError> {
        let alerts = self.services.alerts();
        let config = alerts.config().await?;
        let Some(chat_id) = config.chat_id else {
            return Ok(());
        };
        let config = if sweep {
            self.refresh(chat_id).await?;
            alerts.config().await?
        } else {
            config
        };
        let Some(chat_id) = config.chat_id else {
            return Ok(());
        };
        if alerts.take_test().await? {
            self.test(&config, chat_id).await?;
        }
        if sweep {
            let fresh = alerts.collect().await?;
            self.deliver_all(&config, chat_id, fresh).await?;
        }
        Ok(())
    }

    pub async fn nodes_step(&self) -> Result<(), BotError> {
        let alerts = self.services.alerts();
        let config = alerts.config().await?;
        let Some(chat_id) = config.chat_id else {
            return Ok(());
        };
        let fresh = alerts.collect_nodes().await?;
        self.deliver_all(&config, chat_id, fresh).await
    }

    async fn deliver_all(
        &self,
        config: &AlertsConfig,
        chat_id: i64,
        fresh: Vec<Alert>,
    ) -> Result<(), BotError> {
        let alerts = self.services.alerts();
        let mut queue = alerts.pending().await?;
        let added = !fresh.is_empty();
        queue.extend(fresh);
        if queue.is_empty() {
            return Ok(());
        }
        if queue.len() > OUTBOX_MAX {
            let dropped = queue.len() - OUTBOX_MAX;
            queue.drain(..dropped);
            tracing::warn!(
                dropped,
                "Telegram долго не принимает уведомления администраторам, самые старые убраны из очереди"
            );
        }
        if added {
            alerts.keep_pending(&queue).await?;
        }
        while let Some(alert) = queue.first() {
            match self.deliver(config, chat_id, alert).await {
                Ok(()) => {}
                Err(err) if later(&err) => return Err(err),
                Err(err) => {
                    tracing::warn!(error = %err, kind = alert.kind.as_str(), "уведомление администраторам отклонено Telegram и пропущено");
                }
            }
            queue.remove(0);
            alerts.keep_pending(&queue).await?;
        }
        Ok(())
    }

    async fn refresh(&self, chat_id: i64) -> Result<(), BotError> {
        let alerts = self.services.alerts();
        match self.api.get_chat(chat_id).await {
            Ok(chat) => {
                alerts
                    .refresh_chat(chat_id, chat.title, chat.is_forum.unwrap_or(false))
                    .await?;
            }
            Err(err) => match err.migrated_to() {
                Some(next) => self.migrate(chat_id, next).await?,
                None if err.is_forbidden() || err.is_chat_missing() => {
                    tracing::info!("бота убрали из группы уведомлений, группа отключена");
                    alerts.drop_chat(chat_id).await?;
                }
                None => tracing::debug!(error = %err, "сведения о группе уведомлений не получены"),
            },
        }
        Ok(())
    }

    async fn migrate(&self, from: i64, to: i64) -> Result<(), BotError> {
        let alerts = self.services.alerts();
        let chat = self.api.get_chat(to).await.ok();
        let title = chat.as_ref().and_then(|c| c.title.clone());
        let forum = chat.and_then(|c| c.is_forum).unwrap_or(false);
        alerts.drop_chat(from).await?;
        alerts.adopt_chat(to, title, forum).await?;
        tracing::info!("группа уведомлений стала супергруппой, адрес обновлён");
        Ok(())
    }

    fn open_button(&self, path: &str) -> Option<Keyboard> {
        let settings = self.services.settings();
        let base = settings
            .public_url
            .as_deref()
            .map(|url| url.trim().trim_end_matches('/'))
            .filter(|url| url.starts_with("https://"))?;
        Some(Keyboard::new().link("Открыть", format!("{base}{path}")))
    }

    async fn deliver(
        &self,
        config: &AlertsConfig,
        chat_id: i64,
        alert: &Alert,
    ) -> Result<(), BotError> {
        let topic = config.topic_for(alert.kind);
        let keyboard = self.open_button(&alert.path);
        self.send(chat_id, topic.as_deref(), &alert.html, keyboard.as_ref())
            .await
    }

    async fn test(&self, config: &AlertsConfig, chat_id: i64) -> Result<(), BotError> {
        let mut places: BTreeMap<Option<String>, Vec<&'static str>> = BTreeMap::new();
        for kind in self.services.alerts().kinds() {
            if config.route(kind).on {
                places
                    .entry(config.topic_for(kind))
                    .or_default()
                    .push(kind.title_ru());
            }
        }
        if places.is_empty() {
            return self
                .send(
                    chat_id,
                    None,
                    "<b>Проверка уведомлений</b>\nВсе уведомления сейчас выключены.",
                    None,
                )
                .await;
        }
        for (topic, titles) in places {
            let listed: Vec<String> = titles.into_iter().map(lowercase_first).collect();
            let text = format!(
                "<b>Проверка уведомлений</b>\nСюда будут приходить: {}.",
                escape(&listed.join(", "))
            );
            self.send(chat_id, topic.as_deref(), &text, None).await?;
        }
        Ok(())
    }

    async fn thread(&self, chat_id: i64, name: &str) -> Result<Option<i64>, BotError> {
        let alerts = self.services.alerts();
        if let Some(id) = alerts.topics().await?.get(name) {
            return Ok(Some(*id));
        }
        match self.api.create_forum_topic(chat_id, name).await {
            Ok(topic) => {
                alerts.remember_topic(name, topic.message_thread_id).await?;
                Ok(Some(topic.message_thread_id))
            }
            Err(err) => {
                tracing::warn!(error = %err, topic = name, "тема для уведомлений не создана, пишем в общий чат");
                Ok(None)
            }
        }
    }

    async fn send(
        &self,
        chat_id: i64,
        topic: Option<&str>,
        text: &str,
        keyboard: Option<&Keyboard>,
    ) -> Result<(), BotError> {
        let thread = match topic {
            Some(name) => self.thread(chat_id, name).await?,
            None => None,
        };
        let first = self
            .api
            .send_to_thread(chat_id, thread, text, keyboard)
            .await;
        let err = match first {
            Ok(_) => return Ok(()),
            Err(err) => err,
        };
        match (topic, &err) {
            (Some(name), err) if thread.is_some() && err.is_thread_missing() => {
                self.services.alerts().forget_topic(name).await?;
                let thread = self.thread(chat_id, name).await?;
                self.api
                    .send_to_thread(chat_id, thread, text, keyboard)
                    .await
                    .map(|_| ())
                    .map_err(BotError::from)
            }
            (_, err) if err.migrated_to().is_some() => {
                if let Some(next) = err.migrated_to() {
                    self.migrate(chat_id, next).await?;
                }
                Ok(())
            }
            (_, err) if err.is_forbidden() || err.is_chat_missing() => {
                tracing::info!("бот не может писать в группу уведомлений, группа отключена");
                self.services.alerts().drop_chat(chat_id).await?;
                Ok(())
            }
            _ => Err(err.into()),
        }
    }
}

pub async fn bind_from_group(
    services: &Services,
    api: &TelegramApi,
    message: &Message,
    args: &str,
) -> Result<(), BotError> {
    let Some(code) = args.trim().strip_prefix(START_PREFIX) else {
        return Ok(());
    };
    let chat = &message.chat;
    let forum = chat.is_forum.unwrap_or(false);
    let bound = services
        .alerts()
        .bind_group(code, chat.id, chat.title.clone(), forum)
        .await?;
    let brand = escape(&services.settings().brand_name);
    let text = match (bound, forum) {
        (true, true) => format!(
            "Готово: сюда будут приходить уведомления для администраторов {brand}. Темы для них бот создаст сам при первом уведомлении."
        ),
        (true, false) => format!(
            "Готово: сюда будут приходить уведомления для администраторов {brand}. Чтобы раскладывать их по темам, включите в группе темы и дайте боту право управлять ими."
        ),
        (false, _) => {
            "Ссылка для подключения устарела. Нажмите «Подключить группу» в админке ещё раз."
                .to_string()
        }
    };
    if let Err(err) = api.send_to_thread(chat.id, None, &text, None).await {
        report(&err);
    }
    Ok(())
}

fn later(err: &BotError) -> bool {
    !matches!(err, BotError::Telegram(TelegramError::Api { code, .. }) if *code != 429 && *code < 500)
}

fn report(err: &TelegramError) {
    tracing::debug!(error = %err, "ответ в группу уведомлений не отправлен");
}
