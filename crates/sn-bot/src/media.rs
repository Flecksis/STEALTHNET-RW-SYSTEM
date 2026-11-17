use crate::BotError;
use crate::handler::{BotCtx, Screen, Target};
use crate::telegram::{Keyboard, Message, PhotoInput, TelegramError};
use crate::ui;
use sn_domain::bot_content::{CAPTION_MAX, ImageSlot, strip_custom_emoji, visible_len};
use sn_services::bot_content::{BotContent, EMOJI_REJECTED_KEY};
use sn_store::{BotImage, Db};

pub const IMAGE_CACHE_PREFIX: &str = "telegram.image.";

pub fn picture(content: &BotContent, screen: &Screen) -> Option<(ImageSlot, String)> {
    if visible_len(&screen.text) > CAPTION_MAX {
        return None;
    }
    match screen.section {
        Some(section) => content.image_for(section),
        None => content
            .versions
            .get(&ImageSlot::Common)
            .map(|v| (ImageSlot::Common, v.clone())),
    }
}

pub async fn show(ctx: &BotCtx, target: Target, screen: &Screen) -> Result<(), BotError> {
    let content = ctx.services.bot_content();
    let mut keyboard = screen.keyboard.clone();
    if screen.menu {
        keyboard = keyboard.push(ui::menu(&content));
    }
    let image = picture(&content, screen);
    let custom = screen.text.contains("<tg-emoji") || keyboard.has_custom_emoji();
    match deliver(ctx, target, &screen.text, &keyboard, image.as_ref()).await {
        Err(BotError::Telegram(err)) if custom && rejected(&err) => {
            tracing::info!(error = %err, "Telegram не принял премиум-эмодзи, отправляем без них");
            remember_emoji(ctx, false).await;
            deliver(
                ctx,
                target,
                &strip_custom_emoji(&screen.text),
                &keyboard.plain(),
                image.as_ref(),
            )
            .await
        }
        Ok(()) => {
            if custom {
                remember_emoji(ctx, true).await;
            }
            Ok(())
        }
        other => other,
    }
}

fn rejected(err: &TelegramError) -> bool {
    matches!(err, TelegramError::Api { code: 400, .. })
}

fn stale_file(err: &TelegramError) -> bool {
    matches!(err, TelegramError::Api { code: 400, description, .. }
        if description.contains("file identifier")
            || description.contains("file reference")
            || description.contains("remote file"))
}

async fn deliver(
    ctx: &BotCtx,
    target: Target,
    text: &str,
    keyboard: &Keyboard,
    image: Option<&(ImageSlot, String)>,
) -> Result<(), BotError> {
    let Some((slot, version)) = image else {
        return deliver_text(ctx, target, text, keyboard).await;
    };
    if let Some(file_id) = cached_file(ctx, *slot, version).await {
        match send_picture(ctx, target, PhotoInput::FileId(&file_id), text, keyboard).await {
            Ok(_) => return Ok(()),
            Err(err) if stale_file(&err) => forget_file(ctx, *slot).await,
            Err(err) => return Err(err.into()),
        }
    }
    let Some(image) = load_image(ctx, *slot).await? else {
        return deliver_text(ctx, target, text, keyboard).await;
    };
    let sent = send_picture(
        ctx,
        target,
        PhotoInput::Upload {
            bytes: &image.bytes,
            content_type: &image.content_type,
        },
        text,
        keyboard,
    )
    .await?;
    if let Some(file_id) = sent
        .as_ref()
        .and_then(Message::largest_photo)
        .map(|p| p.file_id.clone())
    {
        remember_file(ctx, *slot, version, &file_id).await;
    }
    Ok(())
}

async fn send_picture(
    ctx: &BotCtx,
    target: Target,
    photo: PhotoInput<'_>,
    text: &str,
    keyboard: &Keyboard,
) -> Result<Option<Message>, TelegramError> {
    if let Some(message_id) = target.message_id {
        match ctx
            .api
            .edit_photo(target.chat_id, message_id, photo, text, Some(keyboard))
            .await
        {
            Ok(message) => return Ok(message),
            Err(err) if err.is_forbidden() || stale_file(&err) => return Err(err),
            Err(err) => {
                tracing::debug!(error = %err, "картинка не заменена, отправляем новое сообщение")
            }
        }
    }
    ctx.api
        .send_photo(target.chat_id, photo, text, Some(keyboard))
        .await
        .map(Some)
}

async fn deliver_text(
    ctx: &BotCtx,
    target: Target,
    text: &str,
    keyboard: &Keyboard,
) -> Result<(), BotError> {
    if let Some(message_id) = target.message_id
        && !target.photo
    {
        match ctx
            .api
            .edit_message(target.chat_id, message_id, text, Some(keyboard))
            .await
        {
            Ok(()) => return Ok(()),
            Err(err) if err.is_forbidden() => return Err(err.into()),
            Err(err) => {
                tracing::debug!(error = %err, "сообщение не отредактировано, отправляем новое")
            }
        }
    }
    ctx.api
        .send_message(target.chat_id, text, Some(keyboard))
        .await?;
    if let Some(message_id) = target.message_id.filter(|_| target.photo)
        && let Err(err) = ctx.api.delete_message(target.chat_id, message_id).await
    {
        tracing::debug!(error = %err, "сообщение с картинкой не удалено");
    }
    Ok(())
}

fn bot_id(ctx: &BotCtx) -> Option<i64> {
    ctx.services
        .bot_token()?
        .split(':')
        .next()?
        .parse::<i64>()
        .ok()
}

fn cache_key(slot: ImageSlot) -> String {
    format!("{IMAGE_CACHE_PREFIX}{}", slot.as_str())
}

async fn cached_file(ctx: &BotCtx, slot: ImageSlot, version: &str) -> Option<String> {
    let bot = bot_id(ctx)?;
    let mut conn = ctx.services.store().conn().await.ok()?;
    let value = conn.bot_state().get(&cache_key(slot)).await.ok()??;
    let matches = value.get("bot").and_then(serde_json::Value::as_i64) == Some(bot)
        && value.get("version").and_then(serde_json::Value::as_str) == Some(version);
    if !matches {
        return None;
    }
    value
        .get("file_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

async fn remember_file(ctx: &BotCtx, slot: ImageSlot, version: &str, file_id: &str) {
    let Some(bot) = bot_id(ctx) else {
        return;
    };
    let value = serde_json::json!({ "bot": bot, "version": version, "file_id": file_id });
    match ctx.services.store().conn().await {
        Ok(mut conn) => {
            if let Err(err) = conn.bot_state().set(&cache_key(slot), &value).await {
                tracing::debug!(error = %err, "код картинки не сохранён");
            }
        }
        Err(err) => tracing::debug!(error = %err, "код картинки не сохранён"),
    }
}

async fn forget_file(ctx: &BotCtx, slot: ImageSlot) {
    if let Ok(mut conn) = ctx.services.store().conn().await
        && let Err(err) = conn
            .bot_state()
            .set(&cache_key(slot), &serde_json::json!({}))
            .await
    {
        tracing::debug!(error = %err, "код картинки не сброшен");
    }
}

async fn load_image(ctx: &BotCtx, slot: ImageSlot) -> Result<Option<BotImage>, BotError> {
    let mut conn = ctx.services.store().conn().await?;
    Ok(conn.bot_images().get(slot.as_str()).await?)
}

async fn remember_emoji(ctx: &BotCtx, accepted: bool) {
    if !ctx.services.bot_runtime().note_custom_emoji(accepted) {
        return;
    }
    let value = if accepted {
        serde_json::json!({})
    } else {
        serde_json::json!({ "at": ctx.services.now().to_rfc3339() })
    };
    match ctx.services.store().conn().await {
        Ok(mut conn) => {
            if let Err(err) = conn.bot_state().set(EMOJI_REJECTED_KEY, &value).await {
                tracing::debug!(error = %err, "отметка о премиум-эмодзи не сохранена");
            }
        }
        Err(err) => tracing::debug!(error = %err, "отметка о премиум-эмодзи не сохранена"),
    }
}
