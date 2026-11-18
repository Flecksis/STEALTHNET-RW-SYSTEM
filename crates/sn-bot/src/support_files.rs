use crate::BotError;
use crate::handler::{Screen, Turn};
use crate::session::Awaiting;
use crate::telegram::{
    Fetched, FileUpload, Keyboard, Message, TelegramApi, TelegramError, strip_tags,
};
use crate::text;
use crate::ui::{self, Text};
use serde::Deserialize;
use sn_domain::bot_content::Section;
use sn_domain::client::Client;
use sn_domain::ids::{ClientId, TicketFileId, TicketId};
use sn_modules::catalog::ticket_files::{self, MB, TELEGRAM_DOWNLOAD_LIMIT};
use sn_services::support::Delivered;
use sn_services::{Actor, ServiceError, Services};
use sn_store::Db;

pub const CAPTION_MAX: usize = 1_024;
const PHOTO_MAX: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingFile {
    pub file_id: String,
    pub name: String,
    pub size: Option<i64>,
    pub caption: String,
    pub group: Option<String>,
}

pub fn incoming(message: &Message) -> Option<IncomingFile> {
    let caption = message
        .caption
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    let group = message.media_group_id.clone();
    if let Some(document) = &message.document {
        return Some(IncomingFile {
            file_id: document.file_id.clone(),
            name: document.file_name.clone().unwrap_or_default(),
            size: document.file_size,
            caption,
            group,
        });
    }
    let photo = message.largest_photo()?;
    Some(IncomingFile {
        file_id: photo.file_id.clone(),
        name: "photo.jpg".into(),
        size: photo.file_size,
        caption,
        group,
    })
}

enum Refused {
    Big(i64),
    Type,
    Off,
    Failed,
    Other(ServiceError),
}

fn refused_screen(turn: &Turn<'_>, refused: Refused) -> Screen {
    let content = turn.content();
    let c = &*content;
    let text = match refused {
        Refused::Big(mb) => ui::text(c, "sup.file_big", &[("mb", Text(&mb.to_string()))]),
        Refused::Type => ui::text(c, "sup.file_type", &[]),
        Refused::Off => ui::text(c, "sup.file_off", &[]),
        Refused::Failed => ui::text(c, "sup.file_failed", &[]),
        Refused::Other(err) => text::user_error(&err),
    };
    Screen::notice(text).section(Section::Support)
}

async fn fetch(
    turn: &Turn<'_>,
    client_id: ClientId,
    file: &IncomingFile,
) -> Result<TicketFileId, Refused> {
    let support = turn.services().support();
    let limits = support.file_limits().ok_or(Refused::Off)?;
    let max = limits.max_bytes.min(TELEGRAM_DOWNLOAD_LIMIT);
    let max_mb = max / MB;
    if file.size.is_some_and(|size| size > max) {
        return Err(Refused::Big(max_mb));
    }
    if !ticket_files::allowed_name(&file.name) {
        return Err(Refused::Type);
    }
    let info = match turn.ctx.api.get_file(&file.file_id).await {
        Ok(info) => info,
        Err(TelegramError::Api { description, .. }) if description.contains("too big") => {
            return Err(Refused::Big(max_mb));
        }
        Err(err) => {
            tracing::warn!(error = %err, "файл из Telegram не получен");
            return Err(Refused::Failed);
        }
    };
    if info.file_size.is_some_and(|size| size > max) {
        return Err(Refused::Big(max_mb));
    }
    let path = info.file_path.ok_or(Refused::Failed)?;
    let bytes = match turn
        .ctx
        .api
        .download(&path, usize::try_from(max).unwrap_or(usize::MAX))
        .await
    {
        Ok(Fetched::Bytes(bytes)) => bytes,
        Ok(Fetched::TooBig) => return Err(Refused::Big(max_mb)),
        Err(err) => {
            tracing::warn!(error = %err, "файл из Telegram не скачан");
            return Err(Refused::Failed);
        }
    };
    let saved = support
        .upload(Actor::Client(client_id), &file.name, bytes)
        .await
        .map_err(|err| match err.code() {
            "support.file_type" | "support.file_empty" | "support.file_broken" => Refused::Type,
            "support.file_big" => Refused::Big(max_mb),
            "support.files_off" => Refused::Off,
            _ => Refused::Other(err),
        })?;
    Ok(saved.id)
}

pub async fn receive(
    turn: &mut Turn<'_>,
    client: &Client,
    file: IncomingFile,
) -> Result<Option<Screen>, BotError> {
    let awaiting = turn.session.awaiting == Some(Awaiting::SupportMessage);
    let preferred = if awaiting {
        turn.session.awaiting = None;
        turn.session.support_ticket.take().map(TicketId)
    } else {
        match turn
            .services()
            .support()
            .telegram_ticket(client.id, None)
            .await
        {
            Ok(Some(ticket)) => Some(ticket),
            Ok(None) => return Ok(None),
            Err(err) => return Ok(Some(refused_screen(turn, Refused::Other(err)))),
        }
    };
    let saved = match fetch(turn, client.id, &file).await {
        Ok(id) => id,
        Err(refused) => return Ok(Some(refused_screen(turn, refused))),
    };
    let delivered = turn
        .services()
        .support()
        .from_telegram_with_files(client.id, &file.caption, &[saved], preferred)
        .await;
    let (key, id) = match delivered {
        Ok(Delivered::Opened(id)) => ("sup.sent", id),
        Ok(Delivered::Added(id)) => ("sup.added", id),
        Err(err) => return Ok(Some(refused_screen(turn, Refused::Other(err)))),
    };
    let repeat = file.group.is_some() && turn.session.media_group == file.group;
    turn.session.media_group = file.group.clone();
    if repeat {
        return Ok(Some(Screen::default()));
    }
    let content = turn.content();
    Ok(Some(
        Screen::notice(ui::text(&content, key, &[("id", Text(&id.to_string()))]))
            .section(Section::Support),
    ))
}

#[derive(Debug, Clone, Deserialize)]
struct ReplyFile {
    id: TicketFileId,
}

pub fn reply_files(params: &serde_json::Value) -> Vec<TicketFileId> {
    params
        .get("files")
        .cloned()
        .and_then(|v| serde_json::from_value::<Vec<ReplyFile>>(v).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.id)
        .collect()
}

struct Outgoing {
    id: TicketFileId,
    name: String,
    mime: String,
    bytes: Vec<u8>,
}

impl Outgoing {
    fn photo(&self) -> bool {
        matches!(
            self.mime.as_str(),
            "image/jpeg" | "image/png" | "image/webp"
        ) && self.bytes.len() <= PHOTO_MAX
    }
}

async fn send_one(
    api: &TelegramApi,
    chat_id: i64,
    file: &Outgoing,
    caption: &str,
    keyboard: Option<&Keyboard>,
) -> Result<(), TelegramError> {
    let upload = FileUpload {
        bytes: &file.bytes,
        name: &file.name,
        mime: &file.mime,
        photo: file.photo(),
    };
    match api.send_document(chat_id, upload, caption, keyboard).await {
        Err(TelegramError::Api { code: 400, .. }) if upload.photo => api
            .send_document(
                chat_id,
                FileUpload {
                    photo: false,
                    ..upload
                },
                caption,
                keyboard,
            )
            .await
            .map(|_| ()),
        other => other.map(|_| ()),
    }
}

pub async fn send_reply(
    services: &Services,
    api: &TelegramApi,
    chat_id: i64,
    text: &str,
    keyboard: &Keyboard,
    files: &[TicketFileId],
) -> Result<(), TelegramError> {
    let mut outgoing = Vec::new();
    {
        let mut conn = services
            .store()
            .conn()
            .await
            .map_err(|e| TelegramError::Transport(e.to_string()))?;
        for id in files {
            let stored = conn
                .ticket_files()
                .content(*id)
                .await
                .map_err(|e| TelegramError::Transport(e.to_string()))?;
            let Some(stored) = stored else { continue };
            let sent = conn
                .ticket_files()
                .sent_to_telegram(*id)
                .await
                .map_err(|e| TelegramError::Transport(e.to_string()))?;
            if let (Some(bytes), false) = (stored.bytes, sent) {
                outgoing.push(Outgoing {
                    id: *id,
                    name: stored.file.name,
                    mime: stored.file.mime,
                    bytes,
                });
            }
        }
    }
    let mark = |id: TicketFileId| async move {
        if let Ok(mut conn) = services.store().conn().await
            && let Err(err) = conn.ticket_files().mark_sent_to_telegram(id).await
        {
            tracing::warn!(error = %err, "отметка об отправке файла не сохранилась");
        }
    };
    if files.len() == 1 && outgoing.len() == 1 && strip_tags(text).chars().count() <= CAPTION_MAX {
        let file = &outgoing[0];
        send_one(api, chat_id, file, text, Some(keyboard)).await?;
        mark(file.id).await;
        return Ok(());
    }
    for file in &outgoing {
        match send_one(api, chat_id, file, "", None).await {
            Ok(()) => mark(file.id).await,
            Err(err) if err.is_retryable() || err.is_forbidden() => return Err(err),
            Err(err) => {
                tracing::warn!(error = %err, file = %file.id, "Telegram не принял файл из ответа поддержки");
                mark(file.id).await;
            }
        }
    }
    api.send_message(chat_id, text, Some(keyboard))
        .await
        .map(|_| ())
}
