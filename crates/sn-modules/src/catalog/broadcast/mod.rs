mod engine;
mod routes;
mod store;
#[cfg(test)]
mod tests;

use crate::catalog::{options, promo, referral};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{Module, ScheduledTask};
use crate::sender::{BotSender, Delivery, OutButton, Outgoing, Photo};
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Duration as Span, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::bot_content::{CAPTION_MAX, TEXT_MAX, normalize_markup, visible_len};
use sn_domain::format::plural_ru;
use sn_domain::ids::{AdminId, ClientId, TariffId};
use sn_domain::image::{self, ImageKind};
use sn_domain::module::ModuleId;
use sn_domain::paging::{Page, PageRequest};
use sn_store::{ActorKind, AuditEntry, Db, StoreError};
use std::time::Duration;

pub use engine::{Engine, EngineConfig};
pub use store::{Broadcast, ImageMeta, ListItem, Problem, Progress, Reach};

pub const CLEANUP_TASK: &str = "cleanup";
pub const BUTTONS_MAX: usize = 6;
pub const BUTTON_TEXT_MAX: usize = 64;
pub const URL_MAX: usize = 512;
pub const RAW_MAX: usize = 16_384;
pub const DAYS_MAX: u16 = 90;
pub const SCHEDULE_DAYS_MAX: i64 = 90;
pub const IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;
pub const IMAGE_MIN_SIDE: u32 = 100;
pub const IMAGE_MAX_SIDES: u32 = 10_000;
pub const IMAGE_MAX_RATIO: u32 = 20;
pub const CALLBACK_PREFIX: &str = "n:";
pub const AUDIT_ENTITY: &str = "broadcast";
const ORPHAN_AFTER_HOURS: i64 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Draft,
    Scheduled,
    Sending,
    Done,
    Canceled,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Scheduled => "scheduled",
            Status::Sending => "sending",
            Status::Done => "done",
            Status::Canceled => "canceled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Status::Draft,
            Status::Scheduled,
            Status::Sending,
            Status::Done,
            Status::Canceled,
        ]
        .into_iter()
        .find(|s| s.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotSection {
    Menu,
    Subscriptions,
    Buy,
    Trial,
    Balance,
    Support,
    Promo,
    Referral,
    Options,
    Documents,
}

impl BotSection {
    pub const ALL: [BotSection; 10] = [
        BotSection::Menu,
        BotSection::Subscriptions,
        BotSection::Buy,
        BotSection::Trial,
        BotSection::Balance,
        BotSection::Support,
        BotSection::Promo,
        BotSection::Referral,
        BotSection::Options,
        BotSection::Documents,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            BotSection::Menu => "menu",
            BotSection::Subscriptions => "subscriptions",
            BotSection::Buy => "buy",
            BotSection::Trial => "trial",
            BotSection::Balance => "balance",
            BotSection::Support => "support",
            BotSection::Promo => "promo",
            BotSection::Referral => "referral",
            BotSection::Options => "options",
            BotSection::Documents => "documents",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            BotSection::Menu => "Главное меню",
            BotSection::Subscriptions => "Мои подписки",
            BotSection::Buy => "Купить подписку",
            BotSection::Trial => "Пробный период",
            BotSection::Balance => "Баланс",
            BotSection::Support => "Поддержка",
            BotSection::Promo => "Промокод",
            BotSection::Referral => "Пригласить друга",
            BotSection::Options => "Доп. опции",
            BotSection::Documents => "Документы",
        }
    }

    pub const fn module(self) -> Option<ModuleId> {
        match self {
            BotSection::Promo => Some(ModuleId::Promo),
            BotSection::Referral => Some(ModuleId::Referral),
            BotSection::Options => Some(ModuleId::Options),
            _ => None,
        }
    }

    pub fn data(self) -> String {
        match self {
            BotSection::Menu => "m".into(),
            BotSection::Subscriptions => "subs".into(),
            BotSection::Buy => "buy".into(),
            BotSection::Trial => "trial".into(),
            BotSection::Balance => "bal".into(),
            BotSection::Support => "sup".into(),
            BotSection::Promo => format!("x:{}", promo::BOT_ENTRY),
            BotSection::Referral => format!("x:{}", referral::BOT_ENTRY),
            BotSection::Options => format!("x:{}", options::BOT_ENTRY),
            BotSection::Documents => "docs".into(),
        }
    }

    pub fn callback(self) -> String {
        format!("{CALLBACK_PREFIX}{}", self.data())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Audience {
    All,
    Active,
    Expiring { days: u16 },
    Expired,
    NeverConnected,
    Trial,
    NoSubscription,
    Tariff { tariff_id: TariffId },
}

impl Audience {
    pub fn check(self) -> Result<Self, BroadcastError> {
        match self {
            Audience::Expiring { days } if days == 0 || days > DAYS_MAX => {
                Err(invalid(format!("Кому: число дней от 1 до {DAYS_MAX}")))
            }
            Audience::Tariff { tariff_id } if tariff_id.0 <= 0 => {
                Err(invalid("Кому: выберите тариф"))
            }
            other => Ok(other),
        }
    }

    pub fn describe(self, tariff: Option<&str>) -> String {
        match self {
            Audience::All => "Все клиенты с Telegram".into(),
            Audience::Active => "С активной подпиской".into(),
            Audience::Expiring { days } => {
                let n = u64::from(days);
                format!(
                    "Подписка закончится в течение {n} {}",
                    plural_ru(n, "дня", "дней", "дней")
                )
            }
            Audience::Expired => "Подписка закончилась".into(),
            Audience::NeverConnected => "Не подключались".into(),
            Audience::Trial => "На пробном периоде".into(),
            Audience::NoSubscription => "Ни разу не было подписки".into(),
            Audience::Tariff { .. } => match tariff {
                Some(name) => format!("Тариф «{name}»"),
                None => "Тариф удалён".into(),
            },
        }
    }

    pub const fn tariff(self) -> Option<TariffId> {
        match self {
            Audience::Tariff { tariff_id } => Some(tariff_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonSpec {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<BotSection>,
}

impl ButtonSpec {
    pub fn out(&self) -> OutButton {
        OutButton {
            text: self.text.clone(),
            url: self.url.clone(),
            callback: self.section.map(BotSection::callback),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Draft {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub image_id: Option<i64>,
    #[serde(default)]
    pub buttons: Vec<ButtonSpec>,
    pub audience: Audience,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
    pub text: String,
    pub image_id: Option<i64>,
    pub buttons: Vec<ButtonSpec>,
    pub audience: Audience,
}

#[derive(Debug, thiserror::Error)]
pub enum BroadcastError {
    #[error("{message}")]
    Rejected { code: &'static str, message: String },
    #[error(transparent)]
    Module(#[from] ModuleError),
}

impl From<StoreError> for BroadcastError {
    fn from(err: StoreError) -> Self {
        BroadcastError::Module(ModuleError::Store(err))
    }
}

fn rejected(code: &'static str, message: impl Into<String>) -> BroadcastError {
    BroadcastError::Rejected {
        code,
        message: message.into(),
    }
}

fn invalid(message: impl Into<String>) -> BroadcastError {
    rejected("broadcast.invalid", message)
}

pub fn clean_text(raw: &str, with_image: bool) -> Result<String, BroadcastError> {
    let raw = raw.replace("\r\n", "\n");
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid("Напишите текст сообщения"));
    }
    if trimmed.chars().count() > RAW_MAX {
        return Err(invalid(format!(
            "Текст с разметкой длиннее {RAW_MAX} знаков"
        )));
    }
    let html = normalize_markup(trimmed).map_err(|p| invalid(format!("Текст: {}", p.message())))?;
    if html.contains("href=\"{") {
        return Err(invalid(
            "Текст: у ссылки должен быть адрес вида <a href=\"https://…\">текст</a>",
        ));
    }
    let length = visible_len(&html);
    if with_image && length > CAPTION_MAX {
        return Err(invalid(format!(
            "С картинкой Telegram показывает до {CAPTION_MAX} знаков, а в тексте {length}. Сократите текст или уберите картинку"
        )));
    }
    if length > TEXT_MAX {
        return Err(invalid(format!(
            "Telegram показывает до {TEXT_MAX} знаков, а в тексте {length}"
        )));
    }
    Ok(html)
}

fn host_of(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or_default()
}

pub fn clean_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    if url.is_empty()
        || url.len() > URL_MAX
        || url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '<' | '>' | '\\'))
    {
        return None;
    }
    let url = if let Some(name) = url.strip_prefix('@') {
        format!("https://t.me/{name}")
    } else if url.starts_with("t.me/") {
        format!("https://{url}")
    } else {
        url.to_string()
    };
    let lower = url.to_ascii_lowercase();
    let host = if let Some(rest) = lower.strip_prefix("https://") {
        host_of(rest)
    } else if let Some(rest) = lower.strip_prefix("http://") {
        host_of(rest)
    } else if let Some(rest) = lower.strip_prefix("tg://") {
        host_of(rest)
    } else {
        return None;
    };
    let valid = !host.is_empty()
        && !host.starts_with('.')
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'));
    (valid && url.len() <= URL_MAX).then_some(url)
}

pub fn clean_buttons(raw: &[ButtonSpec]) -> Result<Vec<ButtonSpec>, BroadcastError> {
    if raw.len() > BUTTONS_MAX {
        return Err(invalid(format!("Кнопок не больше {BUTTONS_MAX}")));
    }
    raw.iter()
        .enumerate()
        .map(|(index, button)| {
            let n = index + 1;
            let text = button.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return Err(invalid(format!("Кнопка {n}: напишите надпись")));
            }
            if text.chars().count() > BUTTON_TEXT_MAX {
                return Err(invalid(format!(
                    "Кнопка {n}: надпись длиннее {BUTTON_TEXT_MAX} знаков"
                )));
            }
            let url = button
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty());
            match (url, button.section) {
                (Some(url), None) => {
                    let url = clean_url(url).ok_or_else(|| {
                        invalid(format!(
                            "Кнопка {n}: адрес должен начинаться с https://, http:// или tg://"
                        ))
                    })?;
                    Ok(ButtonSpec {
                        text,
                        url: Some(url),
                        section: None,
                    })
                }
                (None, Some(section)) => Ok(ButtonSpec {
                    text,
                    url: None,
                    section: Some(section),
                }),
                _ => Err(invalid(format!(
                    "Кнопка {n}: укажите адрес ссылки или раздел бота"
                ))),
            }
        })
        .collect()
}

pub fn check_image(bytes: &[u8]) -> Result<image::ImageInfo, BroadcastError> {
    let bad = |message: String| rejected("broadcast.image", message);
    if bytes.is_empty() {
        return Err(bad("Файл пустой".into()));
    }
    if bytes.len() > IMAGE_MAX_BYTES {
        return Err(bad("Файл больше 5 МБ".into()));
    }
    let info = image::inspect(bytes).ok_or_else(|| {
        bad("Нужна картинка JPEG или PNG: этот файл не открылся как картинка".into())
    })?;
    if !matches!(info.kind, ImageKind::Jpeg | ImageKind::Png) {
        return Err(bad(
            "Telegram показывает в сообщениях только JPEG и PNG".into()
        ));
    }
    let (short, long) = if info.width < info.height {
        (info.width, info.height)
    } else {
        (info.height, info.width)
    };
    if short < IMAGE_MIN_SIDE {
        return Err(bad(format!(
            "Картинка меньше {IMAGE_MIN_SIDE} точек по стороне"
        )));
    }
    if info.width + info.height > IMAGE_MAX_SIDES {
        return Err(bad(format!(
            "Ширина и высота вместе больше {IMAGE_MAX_SIDES} точек"
        )));
    }
    if long > short.saturating_mul(IMAGE_MAX_RATIO) {
        return Err(bad(
            "Картинка слишком вытянутая: одна сторона больше другой в 20 раз".into(),
        ));
    }
    Ok(info)
}

async fn cleaned(db: &mut impl Db, draft: &Draft) -> Result<Cleaned, BroadcastError> {
    if let Some(id) = draft.image_id
        && !store::image_exists(db, id).await?
    {
        return Err(rejected(
            "broadcast.image",
            "Картинка не найдена: загрузите её заново",
        ));
    }
    let audience = draft.audience.check()?;
    if let Some(tariff) = audience.tariff()
        && !store::tariff_alive(db, tariff).await?
    {
        return Err(invalid("Кому: такого тарифа нет"));
    }
    Ok(Cleaned {
        text: clean_text(&draft.text, draft.image_id.is_some())?,
        image_id: draft.image_id,
        buttons: clean_buttons(&draft.buttons)?,
        audience,
    })
}

fn audit(
    action: &str,
    admin: Option<AdminId>,
    id: Option<i64>,
    details: serde_json::Value,
) -> AuditEntry {
    AuditEntry {
        actor_kind: if admin.is_some() {
            ActorKind::Admin
        } else {
            ActorKind::System
        },
        actor_id: admin.map(|a| a.0),
        action: action.into(),
        entity_kind: id.map(|_| AUDIT_ENTITY.into()),
        entity_id: id,
        details,
        ip: None,
    }
}

pub async fn create(
    ctx: &ModuleContext,
    draft: &Draft,
    admin: Option<AdminId>,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let content = cleaned(&mut tx, draft).await?;
    let id = store::insert(&mut tx, &content, admin).await?;
    let view = store::view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn update(
    ctx: &ModuleContext,
    id: i64,
    draft: &Draft,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let status = store::lock(&mut tx, id).await?;
    if status != Status::Draft {
        return Err(not_draft(status));
    }
    let content = cleaned(&mut tx, draft).await?;
    store::update(&mut tx, id, &content).await?;
    let view = store::view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

fn not_draft(status: Status) -> BroadcastError {
    rejected(
        "broadcast.state",
        match status {
            Status::Scheduled => "Рассылка запланирована: отмените отправку, чтобы её изменить",
            Status::Sending => "Рассылка уже идёт",
            Status::Done => "Рассылка уже отправлена",
            Status::Canceled => "Рассылка остановлена",
            Status::Draft => "Рассылка — черновик",
        },
    )
}

pub async fn view(ctx: &ModuleContext, id: i64) -> Result<Broadcast, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    Ok(store::view(&mut conn, id).await?)
}

pub async fn list(
    ctx: &ModuleContext,
    page: PageRequest,
) -> Result<Page<ListItem>, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    Ok(store::list(&mut conn, page).await?)
}

pub async fn delete(ctx: &ModuleContext, id: i64) -> Result<(), BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let status = store::lock(&mut tx, id).await?;
    if status != Status::Draft {
        return Err(rejected("broadcast.state", "Удалить можно только черновик"));
    }
    store::delete(&mut tx, id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn copy(
    ctx: &ModuleContext,
    id: i64,
    admin: Option<AdminId>,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let source = store::view(&mut tx, id).await?;
    let content = Cleaned {
        text: source.text,
        image_id: source.image.map(|i| i.id),
        buttons: source.buttons,
        audience: source.audience,
    };
    let copied = store::insert(&mut tx, &content, admin).await?;
    let view = store::view(&mut tx, copied).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn reach(ctx: &ModuleContext, audience: Audience) -> Result<Reach, BroadcastError> {
    let audience = audience.check()?;
    let mut conn = ctx.store.conn().await?;
    let now = crate::catalog::db_now(&mut conn).await?;
    Ok(store::reach(&mut conn, audience, now).await?)
}

pub async fn start(
    ctx: &ModuleContext,
    id: i64,
    at: Option<DateTime<Utc>>,
    admin: Option<AdminId>,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let status = store::lock(&mut tx, id).await?;
    if status != Status::Draft {
        return Err(not_draft(status));
    }
    let now = crate::catalog::db_now(&mut tx).await?;
    let current = store::view(&mut tx, id).await?;
    current.audience.check()?;
    match at {
        Some(at) if at > now + Span::seconds(30) => {
            if at > now + Span::days(SCHEDULE_DAYS_MAX) {
                return Err(invalid(format!(
                    "Запланировать можно не дальше чем на {SCHEDULE_DAYS_MAX} дней вперёд"
                )));
            }
            store::schedule(&mut tx, id, at, admin).await?;
            tx.audit()
                .record(&audit(
                    "broadcast.scheduled",
                    admin,
                    Some(id),
                    serde_json::json!({ "send_at": at, "audience": current.audience }),
                ))
                .await?;
        }
        Some(_) => {
            return Err(invalid(
                "Время отправки уже прошло: выберите время позже или отправьте сейчас",
            ));
        }
        None => {
            let total = store::launch(&mut tx, id, current.audience, now, admin).await?;
            if total == 0 {
                return Err(rejected(
                    "broadcast.empty",
                    "Получателей нет: в этой аудитории никого с Telegram, кто не остановил бота",
                ));
            }
            tx.audit()
                .record(&audit(
                    "broadcast.started",
                    admin,
                    Some(id),
                    serde_json::json!({ "total": total, "audience": current.audience }),
                ))
                .await?;
        }
    }
    let view = store::view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn cancel(
    ctx: &ModuleContext,
    id: i64,
    admin: Option<AdminId>,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let status = store::lock(&mut tx, id).await?;
    let action = match status {
        Status::Scheduled => {
            store::unschedule(&mut tx, id).await?;
            "broadcast.unscheduled"
        }
        Status::Sending => {
            store::stop(&mut tx, id, None).await?;
            "broadcast.canceled"
        }
        other => {
            return Err(rejected(
                "broadcast.state",
                match other {
                    Status::Draft => "Рассылка ещё не отправлялась",
                    Status::Canceled => "Рассылка уже остановлена",
                    _ => "Рассылка уже закончилась",
                },
            ));
        }
    };
    tx.audit()
        .record(&audit(action, admin, Some(id), serde_json::json!({})))
        .await?;
    let view = store::view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestResult {
    Sent,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestReport {
    pub result: TestResult,
    pub message: String,
}

pub async fn send_test(
    ctx: &ModuleContext,
    draft: &Draft,
    client_id: ClientId,
    admin: Option<AdminId>,
) -> Result<TestReport, BroadcastError> {
    let sender = ctx.core.bot_sender().ok_or_else(|| {
        rejected(
            "broadcast.bot",
            "Бот не подключён: проверьте токен в настройках бота",
        )
    })?;
    let (content, chat, image) = {
        let mut conn = ctx.store.conn().await?;
        let content = cleaned(&mut conn, draft).await?;
        let chat = store::chat_of(&mut conn, client_id).await?.ok_or_else(|| {
            rejected(
                "broadcast.no_telegram",
                "У клиента нет Telegram: проверить можно только на клиенте из бота",
            )
        })?;
        let image = match content.image_id {
            Some(id) => Some(store::image(&mut conn, id).await?),
            None => None,
        };
        (content, chat, image)
    };
    let buttons: Vec<OutButton> = content.buttons.iter().map(ButtonSpec::out).collect();
    let bot = sender.bot_id();
    let delivery = deliver_once(&*sender, bot, chat, &content.text, image.as_ref(), &buttons).await;
    let mut conn = ctx.store.conn().await?;
    if let (
        Delivery::Sent {
            file_id: Some(file_id),
        },
        Some(image),
        Some(bot),
    ) = (&delivery, &image, bot)
    {
        store::remember_file(&mut conn, image.id, bot, file_id).await?;
    }
    let report = match delivery {
        Delivery::Sent { .. } => TestReport {
            result: TestResult::Sent,
            message: "Сообщение отправлено".into(),
        },
        Delivery::Blocked(_) => {
            conn.clients().set_bot_unreachable(client_id, true).await?;
            TestReport {
                result: TestResult::Blocked,
                message: "Клиент остановил бота: сообщение не дошло".into(),
            }
        }
        Delivery::Wait(pause) => {
            return Err(rejected(
                "broadcast.telegram",
                format!(
                    "Telegram просит подождать {} с и повторить",
                    pause.as_secs().max(1)
                ),
            ));
        }
        Delivery::NoBot(reason) => {
            return Err(rejected(
                "broadcast.bot",
                format!("Бот не подключён: {reason}"),
            ));
        }
        Delivery::StaleFile | Delivery::Failed { .. } => {
            let reason = match delivery {
                Delivery::Failed { error, .. } => error,
                _ => "картинка не принята".into(),
            };
            return Err(rejected(
                "broadcast.telegram",
                format!("Telegram не принял сообщение: {reason}"),
            ));
        }
    };
    conn.audit()
        .record(&audit(
            "broadcast.test",
            admin,
            None,
            serde_json::json!({ "client_id": client_id, "result": report.result }),
        ))
        .await?;
    Ok(report)
}

async fn deliver_once(
    sender: &dyn BotSender,
    bot: Option<i64>,
    chat: i64,
    text: &str,
    image: Option<&store::ImageData>,
    buttons: &[OutButton],
) -> Delivery {
    let cached = image.and_then(|i| i.file_for(bot));
    if let Some(file_id) = cached {
        let delivery = sender
            .send(
                chat,
                &Outgoing {
                    html: text,
                    photo: Some(Photo::Cached(file_id)),
                    buttons,
                },
            )
            .await;
        if delivery != Delivery::StaleFile {
            return delivery;
        }
    }
    sender
        .send(
            chat,
            &Outgoing {
                html: text,
                photo: image.map(|i| Photo::Upload {
                    bytes: &i.bytes,
                    content_type: &i.content_type,
                }),
                buttons,
            },
        )
        .await
}

pub async fn upload_image(
    ctx: &ModuleContext,
    bytes: &[u8],
    admin: Option<AdminId>,
) -> Result<ImageMeta, BroadcastError> {
    let info = check_image(bytes)?;
    let mut conn = ctx.store.conn().await?;
    Ok(store::insert_image(&mut conn, bytes, info, admin).await?)
}

pub async fn image_file(ctx: &ModuleContext, id: i64) -> Result<(String, Vec<u8>), BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    let image = store::image(&mut conn, id).await?;
    Ok((image.content_type, image.bytes))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionOption {
    pub key: BotSection,
    pub title: &'static str,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TariffOption {
    pub id: TariffId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub text: usize,
    pub caption: usize,
    pub raw: usize,
    pub buttons: usize,
    pub button_text: usize,
    pub url: usize,
    pub days: u16,
    pub schedule_days: i64,
    pub image_bytes: usize,
}

pub const LIMITS: Limits = Limits {
    text: TEXT_MAX,
    caption: CAPTION_MAX,
    raw: RAW_MAX,
    buttons: BUTTONS_MAX,
    button_text: BUTTON_TEXT_MAX,
    url: URL_MAX,
    days: DAYS_MAX,
    schedule_days: SCHEDULE_DAYS_MAX,
    image_bytes: IMAGE_MAX_BYTES,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Options {
    pub bot: bool,
    pub bot_username: Option<String>,
    pub sections: Vec<SectionOption>,
    pub tariffs: Vec<TariffOption>,
    pub limits: Limits,
}

pub async fn options(ctx: &ModuleContext) -> Result<Options, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    let tariffs = store::tariffs(&mut conn).await?;
    let enabled = store::enabled_modules(&mut conn).await?;
    let bot_username = conn
        .settings()
        .get(sn_domain::settings::keys::BOT_USERNAME)
        .await?
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|u| !u.trim().is_empty());
    let bot = ctx
        .core
        .bot_sender()
        .is_some_and(|sender| sender.bot_id().is_some());
    Ok(Options {
        bot,
        bot_username,
        sections: BotSection::ALL
            .into_iter()
            .map(|key| SectionOption {
                key,
                title: key.title(),
                available: key
                    .module()
                    .is_none_or(|m| enabled.iter().any(|e| e == m.as_str())),
            })
            .collect(),
        tariffs,
        limits: LIMITS,
    })
}

pub async fn cleanup(ctx: &ModuleContext) -> Result<u64, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    Ok(store::prune_images(&mut conn, ORPHAN_AFTER_HOURS).await?)
}

pub struct BroadcastModule;

#[async_trait]
impl Module for BroadcastModule {
    fn id(&self) -> ModuleId {
        ModuleId::Broadcast
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: CLEANUP_TASK,
            every: Duration::from_secs(6 * 3600),
        }]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        if task != CLEANUP_TASK {
            return Err(ModuleError::Invalid(format!("у модуля нет задачи {task}")));
        }
        let removed = cleanup(ctx).await?;
        if removed > 0 {
            tracing::info!(removed, "рассылки: удалены неиспользуемые картинки");
        }
        Ok(())
    }
}
