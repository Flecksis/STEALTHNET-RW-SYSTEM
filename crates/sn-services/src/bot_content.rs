use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sn_domain::bot_content::{
    self as catalog, BUTTON_MAX, ButtonOverride, CAPTION_MAX, Content, ContentConfig, EntryKind,
    ImageMode, ImageSlot, ImagesConfig, Section, TEXT_MAX, Var,
};
use sn_domain::client::Locale;
use sn_domain::ids::AdminId;
use sn_domain::image::{self, ImageKind};
use sn_store::{BOT_IMAGE_MAX_BYTES, BotImage, Db};
use std::collections::BTreeMap;

pub const CONTENT_KEY: &str = "bot.content";
pub const IMAGES_KEY: &str = "bot.images";
pub const DOCUMENTS_KEY: &str = "bot.documents";
pub const CONSENT_KEY: &str = "bot.documents_consent";
pub const EMOJI_REJECTED_KEY: &str = "telegram.custom_emoji_rejected";
pub const DOCUMENTS_MAX: usize = 10;
pub const DOCUMENT_TITLE_MAX: usize = 80;
pub const DOCUMENT_URL_MAX: usize = 500;
pub const IMAGE_MIN_SIDE: u32 = 100;
pub const IMAGE_MAX_SIDES: u32 = 10_000;
pub const IMAGE_MAX_RATIO: u32 = 20;
const VERSION_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub title: String,
    pub url: String,
}

impl Document {
    pub fn valid(&self) -> bool {
        !self.title.trim().is_empty()
            && (self.url.starts_with("https://") || self.url.starts_with("http://"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotContent {
    pub content: Content,
    pub images: ImagesConfig,
    pub versions: BTreeMap<ImageSlot, String>,
    pub documents: Vec<Document>,
    pub consent: bool,
}

impl Default for BotContent {
    fn default() -> Self {
        BotContent {
            content: Content::default(),
            images: ImagesConfig::default(),
            versions: BTreeMap::new(),
            documents: Vec::new(),
            consent: true,
        }
    }
}

impl BotContent {
    pub fn in_locale(&self, locale: Locale) -> BotContent {
        BotContent {
            content: self.content.clone().in_locale(locale),
            ..self.clone()
        }
    }

    pub fn locale(&self) -> Locale {
        self.content.locale()
    }

    pub fn image_for(&self, section: Section) -> Option<(ImageSlot, String)> {
        let slot = self
            .images
            .resolve(section, |slot| self.versions.contains_key(&slot))?;
        self.versions.get(&slot).map(|v| (slot, v.clone()))
    }

    pub fn consent_documents(&self) -> &[Document] {
        if self.consent { &self.documents } else { &[] }
    }
}

fn parsed<T: serde::de::DeserializeOwned>(value: Option<Value>) -> Option<T> {
    value.and_then(|v| serde_json::from_value(v).ok())
}

pub async fn load(db: &mut impl Db) -> ServiceResult<BotContent> {
    let config: ContentConfig = parsed(db.settings().get(CONTENT_KEY).await?).unwrap_or_default();
    let images: ImagesConfig = parsed(db.settings().get(IMAGES_KEY).await?).unwrap_or_default();
    let documents: Vec<Document> = parsed::<Vec<Document>>(db.settings().get(DOCUMENTS_KEY).await?)
        .unwrap_or_default()
        .into_iter()
        .filter(Document::valid)
        .collect();
    let consent = parsed::<bool>(db.settings().get(CONSENT_KEY).await?).unwrap_or(true);
    let versions = db
        .bot_images()
        .versions()
        .await?
        .into_iter()
        .filter_map(|(slot, sha)| {
            ImageSlot::parse(&slot).map(|slot| (slot, sha.chars().take(VERSION_LEN).collect()))
        })
        .collect();
    Ok(BotContent {
        content: Content::new(config),
        images,
        versions,
        documents,
        consent,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionView {
    pub key: Section,
    pub title: &'static str,
    pub image: Option<ImageSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryView {
    pub key: &'static str,
    pub section: Section,
    pub kind: EntryKind,
    pub label: &'static str,
    pub default: &'static str,
    pub vars: &'static [Var],
    pub optional: bool,
    pub text: Option<String>,
    pub button: Option<ButtonOverride>,
    pub default_en: &'static str,
    pub text_en: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContentView {
    pub sections: Vec<SectionView>,
    pub entries: Vec<EntryView>,
    pub english: bool,
    pub menu_columns: u8,
    pub emoji_rejected_at: Option<DateTime<Utc>>,
    pub text_max: usize,
    pub caption_max: usize,
    pub button_max: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TranslationUpdate {
    #[serde(default)]
    pub texts: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub buttons: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ContentUpdate {
    #[serde(default)]
    pub texts: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub buttons: BTreeMap<String, Option<ButtonOverride>>,
    #[serde(default)]
    pub menu_columns: Option<u8>,
    #[serde(default)]
    pub en: Option<TranslationUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageSlotView {
    pub slot: ImageSlot,
    pub title: &'static str,
    pub mode: Option<ImageMode>,
    pub version: Option<String>,
    pub sections: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImagesView {
    pub slots: Vec<ImageSlotView>,
    pub max_bytes: usize,
    pub min_side: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentsView {
    pub consent: bool,
    pub items: Vec<Document>,
    pub max: usize,
    pub title_max: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DocumentsUpdate {
    pub consent: bool,
    #[serde(default)]
    pub items: Vec<Document>,
}

fn invalid(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::rejected(code, message)
}

fn slot_of(raw: &str) -> ServiceResult<ImageSlot> {
    ImageSlot::parse(raw).ok_or(ServiceError::NotFound("раздел для картинки"))
}

fn label_of(key: &str) -> String {
    match catalog::entry(key) {
        Some(entry) => format!("«{}» ({})", entry.label, entry.section.title()),
        None => format!("«{key}»"),
    }
}

pub fn check_image(bytes: &[u8]) -> ServiceResult<ImageKind> {
    if bytes.is_empty() {
        return Err(invalid("bot_image.invalid", "Файл пустой"));
    }
    if bytes.len() > BOT_IMAGE_MAX_BYTES {
        return Err(invalid("bot_image.invalid", "Файл больше 5 МБ"));
    }
    let info = image::inspect(bytes).ok_or_else(|| {
        invalid(
            "bot_image.invalid",
            "Нужна картинка JPEG или PNG: этот файл не открылся как картинка",
        )
    })?;
    if !matches!(info.kind, ImageKind::Jpeg | ImageKind::Png) {
        return Err(invalid(
            "bot_image.invalid",
            "Telegram показывает в сообщениях только JPEG и PNG",
        ));
    }
    let (short, long) = if info.width < info.height {
        (info.width, info.height)
    } else {
        (info.height, info.width)
    };
    if short < IMAGE_MIN_SIDE {
        return Err(invalid(
            "bot_image.invalid",
            format!("Картинка меньше {IMAGE_MIN_SIDE} точек по стороне"),
        ));
    }
    if info.width + info.height > IMAGE_MAX_SIDES {
        return Err(invalid(
            "bot_image.invalid",
            format!("Ширина и высота вместе больше {IMAGE_MAX_SIDES} точек"),
        ));
    }
    if long > short.saturating_mul(IMAGE_MAX_RATIO) {
        return Err(invalid(
            "bot_image.invalid",
            "Картинка слишком вытянутая: одна сторона больше другой в 20 раз",
        ));
    }
    Ok(info.kind)
}

fn clean_document(document: &Document) -> ServiceResult<Document> {
    let title: String = document
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        return Err(invalid("bot_documents.invalid", "У документа нет названия"));
    }
    if title.chars().count() > DOCUMENT_TITLE_MAX {
        return Err(invalid(
            "bot_documents.invalid",
            format!("Название «{title}» длиннее {DOCUMENT_TITLE_MAX} символов"),
        ));
    }
    let url = document.url.trim().to_string();
    let well_formed = url.starts_with("https://")
        && url.len() > "https://".len()
        && url.len() <= DOCUMENT_URL_MAX
        && !url
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '<' | '>' | '\''));
    if !well_formed {
        return Err(invalid(
            "bot_documents.invalid",
            format!("У документа «{title}» нужна ссылка вида https://…"),
        ));
    }
    Ok(Document { title, url })
}

fn apply_translation(
    next: &mut ContentConfig,
    locale: Locale,
    update: &TranslationUpdate,
) -> ServiceResult<()> {
    let Some(own) = next.translation_mut(locale) else {
        return Ok(());
    };
    for (key, value) in &update.texts {
        if catalog::entry(key).is_none_or(|e| e.kind != EntryKind::Text) {
            return Err(invalid("bot_content.invalid", format!("Текста {key} нет")));
        }
        let standard = catalog::default_text(key, locale).unwrap_or("");
        match value {
            None => {
                own.texts.remove(key);
            }
            Some(raw) => {
                let clean = catalog::clean_text(key, raw).map_err(|p| {
                    invalid(
                        "bot_content.invalid",
                        format!("{}, английский: {}", label_of(key), p.message),
                    )
                })?;
                if clean == standard {
                    own.texts.remove(key);
                } else {
                    own.texts.insert(key.clone(), clean);
                }
            }
        }
    }
    for (key, value) in &update.buttons {
        if catalog::entry(key).is_none_or(|e| e.kind != EntryKind::Button) {
            return Err(invalid("bot_content.invalid", format!("Кнопки {key} нет")));
        }
        let standard = catalog::default_text(key, locale).unwrap_or("");
        let clean = match value {
            None => None,
            Some(raw) => catalog::clean_label(key, raw).map_err(|p| {
                invalid(
                    "bot_content.invalid",
                    format!("{}, английский: {}", label_of(key), p.message),
                )
            })?,
        };
        match clean {
            Some(text) if text != standard => {
                own.buttons.insert(key.clone(), text);
            }
            _ => {
                own.buttons.remove(key);
            }
        }
    }
    Ok(())
}

pub struct BotContentOps<'a> {
    services: &'a Services,
}

impl<'a> BotContentOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        BotContentOps { services }
    }

    async fn saved(&self) -> ServiceResult<()> {
        self.services.reload().await?;
        self.services.notify_settings_changed().await
    }

    pub async fn content(&self) -> ServiceResult<ContentView> {
        let runtime = self.services.bot_content();
        let config = runtime.content.config();
        let rejected = {
            let mut conn = self.services.store().conn().await?;
            conn.bot_state().get(EMOJI_REJECTED_KEY).await?
        };
        let emoji_rejected_at = rejected
            .as_ref()
            .and_then(|v| v.get("at"))
            .and_then(Value::as_str)
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
            .map(|at| at.with_timezone(&Utc));
        Ok(ContentView {
            sections: Section::ALL
                .into_iter()
                .map(|key| SectionView {
                    key,
                    title: key.title(),
                    image: key.image(),
                })
                .collect(),
            entries: catalog::CATALOG
                .iter()
                .map(|entry| EntryView {
                    key: entry.key,
                    section: entry.section,
                    kind: entry.kind,
                    label: entry.label,
                    default: entry.default,
                    vars: entry.vars,
                    optional: entry.optional,
                    text: match entry.kind {
                        EntryKind::Text => config.texts.get(entry.key).cloned(),
                        EntryKind::Button => None,
                    },
                    button: match entry.kind {
                        EntryKind::Button => config.buttons.get(entry.key).cloned(),
                        EntryKind::Text => None,
                    },
                    default_en: catalog::default_text(entry.key, Locale::En).unwrap_or(""),
                    text_en: match entry.kind {
                        EntryKind::Text => config.en.texts.get(entry.key).cloned(),
                        EntryKind::Button => config.en.buttons.get(entry.key).cloned(),
                    },
                })
                .collect(),
            english: self.services.settings().languages.english,
            menu_columns: u8::try_from(runtime.content.menu_columns()).unwrap_or(1),
            emoji_rejected_at,
            text_max: TEXT_MAX,
            caption_max: CAPTION_MAX,
            button_max: BUTTON_MAX,
        })
    }

    pub async fn update_content(
        &self,
        update: &ContentUpdate,
        admin: AdminId,
    ) -> ServiceResult<ContentView> {
        let current = self.services.bot_content().content.config().clone();
        let mut next = current.clone();
        for (key, value) in &update.texts {
            let entry = catalog::entry(key)
                .filter(|e| e.kind == EntryKind::Text)
                .ok_or_else(|| invalid("bot_content.invalid", format!("Текста {key} нет")))?;
            match value {
                None => {
                    next.texts.remove(key);
                }
                Some(raw) => {
                    let clean = catalog::clean_text(key, raw).map_err(|p| {
                        invalid(
                            "bot_content.invalid",
                            format!("{}: {}", label_of(key), p.message),
                        )
                    })?;
                    if clean == entry.default {
                        next.texts.remove(key);
                    } else {
                        next.texts.insert(key.clone(), clean);
                    }
                }
            }
        }
        for (key, value) in &update.buttons {
            match value {
                None => {
                    if catalog::entry(key).is_none_or(|e| e.kind != EntryKind::Button) {
                        return Err(invalid("bot_content.invalid", format!("Кнопки {key} нет")));
                    }
                    next.buttons.remove(key);
                }
                Some(raw) => {
                    let clean = catalog::clean_button(key, raw).map_err(|p| {
                        invalid(
                            "bot_content.invalid",
                            format!("{}: {}", label_of(key), p.message),
                        )
                    })?;
                    if clean.is_empty() {
                        next.buttons.remove(key);
                    } else {
                        next.buttons.insert(key.clone(), clean);
                    }
                }
            }
        }
        if let Some(en) = &update.en {
            apply_translation(&mut next, Locale::En, en)?;
        }
        if let Some(columns) = update.menu_columns {
            if !matches!(columns, 1 | 2) {
                return Err(invalid(
                    "bot_content.invalid",
                    "В главном меню может быть 1 или 2 кнопки в ряд",
                ));
            }
            next.menu_columns = (columns == 2).then_some(2);
        }
        if next == current {
            return self.content().await;
        }
        let changed_texts: Vec<&String> = update
            .texts
            .keys()
            .filter(|k| next.texts.get(*k) != current.texts.get(*k))
            .collect();
        let changed_buttons: Vec<&String> = update
            .buttons
            .keys()
            .filter(|k| next.buttons.get(*k) != current.buttons.get(*k))
            .collect();
        let (en_texts, en_buttons): (Vec<&String>, Vec<&String>) = match &update.en {
            Some(en) => (
                en.texts
                    .keys()
                    .filter(|k| next.en.texts.get(*k) != current.en.texts.get(*k))
                    .collect(),
                en.buttons
                    .keys()
                    .filter(|k| next.en.buttons.get(*k) != current.en.buttons.get(*k))
                    .collect(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(CONTENT_KEY, &next, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.bot_content",
                "settings",
                None,
                json!({
                    "texts": changed_texts,
                    "buttons": changed_buttons,
                    "en_texts": en_texts,
                    "en_buttons": en_buttons,
                    "menu_columns": next.menu_columns.unwrap_or(1),
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        self.content().await
    }

    pub fn images(&self) -> ImagesView {
        let runtime = self.services.bot_content();
        ImagesView {
            slots: ImageSlot::ALL
                .into_iter()
                .map(|slot| ImageSlotView {
                    slot,
                    title: slot.title(),
                    mode: (slot != ImageSlot::Common).then(|| runtime.images.mode(slot)),
                    version: runtime.versions.get(&slot).cloned(),
                    sections: Section::ALL
                        .into_iter()
                        .filter(|section| match slot {
                            ImageSlot::Common => section.image().is_some(),
                            other => section.image() == Some(other),
                        })
                        .map(Section::title)
                        .collect(),
                })
                .collect(),
            max_bytes: BOT_IMAGE_MAX_BYTES,
            min_side: IMAGE_MIN_SIDE,
        }
    }

    async fn audit_image(
        &self,
        db: &mut impl Db,
        admin: AdminId,
        slot: ImageSlot,
        details: Value,
    ) -> ServiceResult<()> {
        let mut details = details;
        details["slot"] = json!(slot.as_str());
        db.audit()
            .record(&Actor::Admin(admin).audit("settings.bot_image", "settings", None, details))
            .await?;
        Ok(())
    }

    pub async fn upload_image(
        &self,
        raw_slot: &str,
        bytes: &[u8],
        admin: AdminId,
    ) -> ServiceResult<ImagesView> {
        let slot = slot_of(raw_slot)?;
        let kind = check_image(bytes)?;
        let digest = hex::encode(Sha256::digest(bytes));
        let mut images = self.services.bot_content().images.clone();
        let mut tx = self.services.store().begin().await?;
        tx.bot_images()
            .put(
                slot.as_str(),
                &BotImage {
                    content_type: kind.content_type().to_string(),
                    bytes: bytes.to_vec(),
                    sha256: digest.clone(),
                },
                Some(admin),
            )
            .await?;
        if slot != ImageSlot::Common && images.mode(slot) == ImageMode::Common {
            images.sections.insert(slot, ImageMode::Own);
            tx.settings()
                .set_as(IMAGES_KEY, &images, Some(admin))
                .await?;
        }
        self.audit_image(
            &mut tx,
            admin,
            slot,
            json!({ "action": "uploaded", "size": bytes.len(), "sha256": digest }),
        )
        .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.images())
    }

    pub async fn delete_image(&self, raw_slot: &str, admin: AdminId) -> ServiceResult<ImagesView> {
        let slot = slot_of(raw_slot)?;
        let mut images = self.services.bot_content().images.clone();
        let mut tx = self.services.store().begin().await?;
        let removed = tx.bot_images().delete(slot.as_str()).await?;
        if !removed {
            tx.commit().await?;
            return Ok(self.images());
        }
        if images.mode(slot) == ImageMode::Own {
            images.sections.remove(&slot);
            tx.settings()
                .set_as(IMAGES_KEY, &images, Some(admin))
                .await?;
        }
        self.audit_image(&mut tx, admin, slot, json!({ "action": "removed" }))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.images())
    }

    pub async fn set_image_mode(
        &self,
        raw_slot: &str,
        mode: ImageMode,
        admin: AdminId,
    ) -> ServiceResult<ImagesView> {
        let slot = slot_of(raw_slot)?;
        if slot == ImageSlot::Common {
            return Err(invalid(
                "bot_image.invalid",
                "У общей картинки нет выбора: её либо загружают, либо удаляют",
            ));
        }
        let mut images = self.services.bot_content().images.clone();
        if images.mode(slot) == mode {
            return Ok(self.images());
        }
        if mode == ImageMode::Common {
            images.sections.remove(&slot);
        } else {
            images.sections.insert(slot, mode);
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(IMAGES_KEY, &images, Some(admin))
            .await?;
        self.audit_image(
            &mut tx,
            admin,
            slot,
            json!({ "action": "mode", "mode": mode }),
        )
        .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.images())
    }

    pub async fn image(&self, raw_slot: &str) -> ServiceResult<Option<BotImage>> {
        let slot = slot_of(raw_slot)?;
        let mut conn = self.services.store().conn().await?;
        Ok(conn.bot_images().get(slot.as_str()).await?)
    }

    pub fn documents(&self) -> DocumentsView {
        let runtime = self.services.bot_content();
        DocumentsView {
            consent: runtime.consent,
            items: runtime.documents.clone(),
            max: DOCUMENTS_MAX,
            title_max: DOCUMENT_TITLE_MAX,
        }
    }

    pub async fn update_documents(
        &self,
        update: &DocumentsUpdate,
        admin: AdminId,
    ) -> ServiceResult<DocumentsView> {
        if update.items.len() > DOCUMENTS_MAX {
            return Err(invalid(
                "bot_documents.invalid",
                format!("Документов может быть не больше {DOCUMENTS_MAX}"),
            ));
        }
        let items = update
            .items
            .iter()
            .map(clean_document)
            .collect::<ServiceResult<Vec<_>>>()?;
        let current = self.documents();
        if items == current.items && update.consent == current.consent {
            return Ok(current);
        }
        let mut tx = self.services.store().begin().await?;
        tx.settings()
            .set_as(DOCUMENTS_KEY, &items, Some(admin))
            .await?;
        tx.settings()
            .set_as(CONSENT_KEY, &update.consent, Some(admin))
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "settings.documents",
                "settings",
                None,
                json!({
                    "count": items.len(),
                    "titles": items.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(),
                    "consent": update.consent,
                }),
            ))
            .await?;
        tx.commit().await?;
        self.saved().await?;
        Ok(self.documents())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn документы_проверяются_и_приводятся_к_виду() {
        let clean = clean_document(&Document {
            title: "  Пользовательское   соглашение ".into(),
            url: " https://example.com/terms ".into(),
        })
        .unwrap();
        assert_eq!(clean.title, "Пользовательское соглашение");
        assert_eq!(clean.url, "https://example.com/terms");
        for url in [
            "http://example.com",
            "https://",
            "https://exa mple.com",
            "javascript:alert(1)",
            "https://example.com/\"x",
        ] {
            assert!(
                clean_document(&Document {
                    title: "Оферта".into(),
                    url: url.into(),
                })
                .is_err(),
                "{url}"
            );
        }
        assert!(
            clean_document(&Document {
                title: " ".into(),
                url: "https://example.com".into(),
            })
            .is_err()
        );
    }

    #[test]
    fn картинка_для_раздела_с_версией() {
        let mut content = BotContent::default();
        assert_eq!(content.image_for(Section::Menu), None);
        content
            .versions
            .insert(ImageSlot::Common, "aaaaaaaaaaaaaaaa".into());
        assert_eq!(
            content.image_for(Section::Menu),
            Some((ImageSlot::Common, "aaaaaaaaaaaaaaaa".into()))
        );
        assert_eq!(content.image_for(Section::Login), None);
        content
            .images
            .sections
            .insert(ImageSlot::Menu, ImageMode::Own);
        content
            .versions
            .insert(ImageSlot::Menu, "bbbbbbbbbbbbbbbb".into());
        assert_eq!(
            content.image_for(Section::Menu),
            Some((ImageSlot::Menu, "bbbbbbbbbbbbbbbb".into()))
        );
        assert!(content.consent);
        content.documents.push(Document {
            title: "Оферта".into(),
            url: "https://example.com/offer".into(),
        });
        assert_eq!(content.consent_documents().len(), 1);
        content.consent = false;
        assert!(content.consent_documents().is_empty());
    }
}
